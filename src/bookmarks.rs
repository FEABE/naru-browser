//! Bounded, single-writer bookmark storage. Move the owned Store to a worker
//! while loading or changing it; this module creates no threads or UI objects.

use crate::address::{self, AddressError};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{
    collections::BTreeSet,
    ffi::CStr,
    fmt,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub const MAX_ITEMS: usize = 1_000;
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TITLE_CHARS: usize = 256;
pub const FILE_NAME: &str = "bookmarks-v1.json";
const PRIMARY: &CStr = c"bookmarks-v1.json";
const BACKUP: &CStr = c"bookmarks-v1.json.bak";
const PENDING: &CStr = c".bookmarks-v1.json.pending";
const BACKUP_PENDING: &CStr = c".bookmarks-v1.json.bak.pending";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bookmark {
    pub id: u64,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    version: u32,
    next_id: u64,
    #[serde(deserialize_with = "read_items")]
    items: Vec<Bookmark>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            items: Vec::new(),
        }
    }
}

impl Document {
    fn validate(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::InvalidData(
                "지원하지 않는 북마크 파일 버전입니다.".into(),
            ));
        }
        if self.items.len() > MAX_ITEMS {
            return Err(Error::Limit(
                "북마크는 최대 1,000개까지 저장할 수 있습니다.",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut urls = BTreeSet::new();
        for item in &self.items {
            if item.id == 0 || item.id >= self.next_id || !ids.insert(item.id) {
                return Err(Error::InvalidData(
                    "북마크 ID가 잘못됐거나 중복됐습니다.".into(),
                ));
            }
            if title(&item.title)? != item.title || address::normalize_url(&item.url)? != item.url {
                return Err(Error::InvalidData(
                    "북마크 이름이나 주소가 저장 형식에 맞지 않습니다.".into(),
                ));
            }
            if !urls.insert(item.url.as_str()) {
                return Err(Error::InvalidData("북마크 주소가 중복됐습니다.".into()));
            }
        }
        if self.next_id == 0 {
            return Err(Error::InvalidData("다음 북마크 ID가 잘못됐습니다.".into()));
        }
        Ok(())
    }
}

fn read_items<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Bookmark>, D::Error> {
    struct Items;
    impl<'de> de::Visitor<'de> for Items {
        type Value = Vec<Bookmark>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("at most 1,000 bookmarks")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == MAX_ITEMS {
                    return Err(de::Error::custom("bookmark item limit exceeded"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    deserializer.deserialize_seq(Items)
}

#[derive(Debug)]
pub enum Error {
    InvalidTitle,
    InvalidUrl(AddressError),
    NotFound,
    Limit(&'static str),
    ReadOnly(String),
    InvalidData(String),
    UnsafePath,
    ExternalChange,
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTitle => {
                formatter.write_str("북마크 이름은 제어 문자 없이 1~256자로 입력해 주세요.")
            }
            Self::InvalidUrl(error) => write!(formatter, "{error}"),
            Self::NotFound => formatter.write_str("북마크를 찾을 수 없습니다."),
            Self::Limit(message) => formatter.write_str(message),
            Self::ReadOnly(reason) => write!(formatter, "북마크는 읽기 전용입니다. {reason}"),
            Self::InvalidData(reason) => formatter.write_str(reason),
            Self::UnsafePath => {
                formatter.write_str("북마크 경로의 파일 종류, 소유자 또는 권한을 확인해 주세요.")
            }
            Self::ExternalChange => {
                formatter.write_str("북마크 파일이 다른 곳에서 변경됐습니다. 다시 읽어 주세요.")
            }
            Self::Io(error) => write!(formatter, "북마크 파일 처리에 실패했습니다: {error}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<AddressError> for Error {
    fn from(error: AddressError) -> Self {
        Self::InvalidUrl(error)
    }
}

#[derive(Debug)]
pub struct Mutation {
    pub id: u64,
    pub changed: bool,
    /// The rename committed. Do not retry the mutation as if it had failed.
    pub durability_warning: Option<String>,
}

/// One Store belongs to the app, not to individual spaces or CLI clients.
/// `load` reads an existing private data_dir; it never creates or repairs files.
#[derive(Debug)]
pub struct Store {
    directory_path: PathBuf,
    path: PathBuf,
    directory: Option<File>,
    document: Document,
    primary_stamp: Option<Stamp>,
    backup_stamp: Option<Stamp>,
    read_only: Option<String>,
    #[cfg(test)]
    fail_at: Option<Stage>,
}

impl Store {
    pub fn load(data_dir: &Path) -> Self {
        let mut store = Self {
            directory_path: data_dir.to_owned(),
            path: data_dir.join(FILE_NAME),
            directory: None,
            document: Document::default(),
            primary_stamp: None,
            backup_stamp: None,
            read_only: None,
            #[cfg(test)]
            fail_at: None,
        };
        let directory = match open_directory(data_dir) {
            Ok(directory) => directory,
            Err(error) => {
                store.read_only = Some(error.to_string());
                return store;
            }
        };
        let primary = read_document(&directory, PRIMARY);
        let backup = read_document(&directory, BACKUP);
        let pending = ensure_no_pending(&directory);
        match primary {
            Ok(Some(loaded)) => {
                store.document = loaded.document;
                store.primary_stamp = Some(loaded.stamp);
                match backup {
                    Ok(loaded) => store.backup_stamp = loaded.map(|loaded| loaded.stamp),
                    Err(error) => {
                        store.read_only = Some(format!("백업 원본을 보존했습니다. {error}"))
                    }
                }
            }
            Ok(None) => match backup {
                Ok(Some(loaded)) => {
                    store.document = loaded.document;
                    store.backup_stamp = Some(loaded.stamp);
                    store.read_only = Some(
                        "본문이 없어 정상 백업을 읽기 전용으로 표시합니다. 복구가 필요합니다."
                            .into(),
                    );
                }
                Ok(None) => {}
                Err(error) => store.read_only = Some(format!("백업 원본을 보존했습니다. {error}")),
            },
            Err(error) => {
                if let Ok(Some(loaded)) = backup {
                    store.document = loaded.document;
                    store.backup_stamp = Some(loaded.stamp);
                }
                store.read_only = Some(format!(
                    "본문 원본을 보존했습니다. 정상 백업이 있으면 읽기 전용으로 표시합니다. {error}"
                ));
            }
        }
        if let Err(error) = pending {
            if store.read_only.is_none() {
                store.read_only = Some(error.to_string());
            }
        }
        store.directory = Some(directory);
        store
    }

    pub fn items(&self) -> &[Bookmark] {
        &self.document.items
    }
    pub fn writable(&self) -> bool {
        self.read_only.is_none()
    }
    pub fn notice(&self) -> Option<&str> {
        self.read_only.as_deref()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn find_by_url(&self, url: &str) -> Result<Option<&Bookmark>, Error> {
        let url = address::normalize_url(url)?;
        Ok(self.document.items.iter().find(|item| item.url == url))
    }

    /// A duplicate keeps its ID and stored title; the UI can open its editor.
    pub fn add(&mut self, url: &str, name: &str) -> Result<Mutation, Error> {
        self.require_writable()?;
        let url = address::normalize_url(url)?;
        let name = title(name)?;
        if let Some(item) = self.document.items.iter().find(|item| item.url == url) {
            return Ok(Mutation {
                id: item.id,
                changed: false,
                durability_warning: None,
            });
        }
        if self.document.items.len() >= MAX_ITEMS {
            return Err(Error::Limit(
                "북마크는 최대 1,000개까지 저장할 수 있습니다.",
            ));
        }
        let id = self.document.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or(Error::Limit("새 북마크 ID를 만들 수 없습니다."))?;
        let mut candidate = self.document.clone();
        candidate.next_id = next_id;
        candidate.items.push(Bookmark {
            id,
            title: name,
            url,
        });
        self.save(candidate, id)
    }

    pub fn rename(&mut self, id: u64, name: &str) -> Result<Mutation, Error> {
        self.require_writable()?;
        let name = title(name)?;
        let index = self
            .document
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or(Error::NotFound)?;
        if self.document.items[index].title == name {
            return Ok(Mutation {
                id,
                changed: false,
                durability_warning: None,
            });
        }
        let mut candidate = self.document.clone();
        candidate.items[index].title = name;
        self.save(candidate, id)
    }

    pub fn remove(&mut self, id: u64) -> Result<Mutation, Error> {
        self.require_writable()?;
        let index = self
            .document
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or(Error::NotFound)?;
        let mut candidate = self.document.clone();
        candidate.items.remove(index);
        self.save(candidate, id)
    }

    fn require_writable(&self) -> Result<(), Error> {
        match &self.read_only {
            Some(reason) => Err(Error::ReadOnly(reason.clone())),
            None => Ok(()),
        }
    }

    fn save(&mut self, candidate: Document, id: u64) -> Result<Mutation, Error> {
        let result = self.persist(candidate);
        if let Err(error @ (Error::ExternalChange | Error::UnsafePath | Error::InvalidData(_))) =
            &result
        {
            self.read_only = Some(error.to_string());
        }
        result.map(|durability_warning| Mutation {
            id,
            changed: true,
            durability_warning,
        })
    }

    fn persist(&mut self, candidate: Document) -> Result<Option<String>, Error> {
        candidate.validate()?;
        let bytes = encode(&candidate)?;
        let directory = self.directory.as_ref().ok_or(Error::UnsafePath)?;
        verify_directory_path(directory, &self.directory_path)?;
        ensure_no_pending(directory)?;
        let current = read_document(directory, PRIMARY)?;
        if current.as_ref().map(|loaded| loaded.stamp) != self.primary_stamp
            || current
                .as_ref()
                .is_some_and(|loaded| loaded.document != self.document)
        {
            return Err(Error::ExternalChange);
        }
        let backup = read_document(directory, BACKUP)?;
        if backup.as_ref().map(|loaded| loaded.stamp) != self.backup_stamp {
            return Err(Error::ExternalChange);
        }
        drop(backup);
        self.checkpoint(Stage::Write)?;
        let mut pending = PendingFile::new(directory, PENDING)?;
        pending.file.write_all(&bytes)?;
        pending.file.flush()?;
        self.checkpoint(Stage::FileSync)?;
        pending.file.sync_all()?;

        if let Some(current) = &current {
            let mut backup_pending = PendingFile::new(directory, BACKUP_PENDING)?;
            backup_pending.file.write_all(&current.bytes)?;
            backup_pending.file.flush()?;
            backup_pending.file.sync_all()?;
            self.checkpoint(Stage::BackupRename)?;
            require_stamp(directory, BACKUP, self.backup_stamp)?;
            let stamp = backup_pending.replace(BACKUP)?;
            self.backup_stamp = Some(stamp);
            self.checkpoint(Stage::BackupSync)?;
            directory.sync_all()?;
        }
        self.checkpoint(Stage::PrimaryRename)?;
        verify_directory_path(directory, &self.directory_path)?;
        require_stamp(directory, PRIMARY, self.primary_stamp)?;
        let stamp = pending.replace(PRIMARY)?;
        // No fallible operation can turn this committed mutation into Err.
        self.primary_stamp = Some(stamp);
        self.document = candidate;
        let sync = self
            .checkpoint(Stage::DirectorySync)
            .and_then(|_| directory.sync_all().map_err(Error::from));
        Ok(sync.err().map(|error| {
            format!("북마크 파일에 반영됐으나 영속 동기화 확인에 실패했습니다. {error}")
        }))
    }

    fn checkpoint(&self, stage: Stage) -> Result<(), Error> {
        #[cfg(test)]
        if self.fail_at == Some(stage) {
            return Err(Error::Io(io::Error::other(
                "injected bookmark storage failure",
            )));
        }
        let _ = stage;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Write,
    FileSync,
    BackupRename,
    BackupSync,
    PrimaryRename,
    DirectorySync,
}

fn title(value: &str) -> Result<String, Error> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_TITLE_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(Error::InvalidTitle);
    }
    Ok(value.to_owned())
}

fn encode(document: &Document) -> Result<Vec<u8>, Error> {
    #[derive(Default)]
    struct BoundedBuffer {
        bytes: Vec<u8>,
        full: bool,
    }
    impl Write for BoundedBuffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_FILE_BYTES.saturating_sub(self.bytes.len()) {
                self.full = true;
                return Err(io::Error::other("bookmark file limit exceeded"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = BoundedBuffer::default();
    let encoded = serde_json::to_writer(&mut buffer, document);
    if buffer.full {
        return Err(Error::Limit(
            "북마크 파일은 최대 2MiB까지 저장할 수 있습니다.",
        ));
    }
    encoded.map_err(|error| Error::InvalidData(error.to_string()))?;
    Ok(buffer.bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl Stamp {
    fn new(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.len(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
}

struct Loaded {
    document: Document,
    bytes: Vec<u8>,
    stamp: Stamp,
}

fn private_file(metadata: &Metadata) -> bool {
    metadata.file_type().is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o077 == 0
        && metadata.nlink() == 1
}

fn open_directory(path: &Path) -> Result<File, Error> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    verify_directory_path(&directory, path)?;
    Ok(directory)
}

fn verify_directory_path(directory: &File, path: &Path) -> Result<(), Error> {
    let opened = directory.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if !current.file_type().is_dir()
        || current.uid() != unsafe { libc::geteuid() }
        || current.mode() & 0o077 != 0
        || opened.dev() != current.dev()
        || opened.ino() != current.ino()
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}

fn open_child(directory: &File, name: &CStr, flags: i32) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600 as libc::c_uint,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn read_document(directory: &File, name: &CStr) -> Result<Option<Loaded>, Error> {
    let file = match open_child(directory, name, libc::O_RDONLY) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
    };
    let metadata = file.metadata()?;
    if !private_file(&metadata) {
        return Err(Error::UnsafePath);
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(Error::Limit(
            "북마크 파일이 2MiB를 초과합니다. 원본을 보존했습니다.",
        ));
    }
    let stamp = Stamp::new(&metadata);
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Limit("북마크 파일이 2MiB를 초과합니다."));
    }
    if stamp != Stamp::new(&file.metadata()?) {
        return Err(Error::ExternalChange);
    }
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| Error::InvalidData(format!("북마크 JSON을 읽을 수 없습니다: {error}")))?;
    document.validate()?;
    Ok(Some(Loaded {
        document,
        bytes,
        stamp,
    }))
}

fn stamp_at(directory: &File, name: &CStr) -> Result<Option<Stamp>, Error> {
    match open_child(directory, name, libc::O_RDONLY) {
        Ok(file) => {
            let metadata = file.metadata()?;
            if !private_file(&metadata) {
                return Err(Error::UnsafePath);
            }
            Ok(Some(Stamp::new(&metadata)))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn require_stamp(directory: &File, name: &CStr, expected: Option<Stamp>) -> Result<(), Error> {
    if stamp_at(directory, name)? != expected {
        Err(Error::ExternalChange)
    } else {
        Ok(())
    }
}

fn ensure_no_pending(directory: &File) -> Result<(), Error> {
    for name in [PENDING, BACKUP_PENDING] {
        if stamp_at(directory, name)?.is_some() {
            return Err(Error::InvalidData(
                "이전 저장의 임시 파일을 보존했습니다. 정리 또는 복구가 필요합니다.".into(),
            ));
        }
    }
    Ok(())
}

struct PendingFile<'a> {
    directory: &'a File,
    name: &'static CStr,
    file: File,
    moved: bool,
}

impl<'a> PendingFile<'a> {
    fn new(directory: &'a File, name: &'static CStr) -> Result<Self, Error> {
        let file = open_child(
            directory,
            name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        Ok(Self {
            directory,
            name,
            file,
            moved: false,
        })
    }

    fn replace(&mut self, destination: &CStr) -> Result<Stamp, Error> {
        let metadata = self.file.metadata()?;
        if !private_file(&metadata) {
            return Err(Error::UnsafePath);
        }
        let stamp = Stamp::new(&metadata);
        require_stamp(self.directory, self.name, Some(stamp))?;
        let result = unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                self.directory.as_raw_fd(),
                destination.as_ptr(),
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error().into());
        }
        self.moved = true;
        // rename may change ctime. The handle still identifies the committed file.
        Ok(Stamp::new(&self.file.metadata().unwrap_or(metadata)))
    }
}

impl Drop for PendingFile<'_> {
    fn drop(&mut self) {
        if self.moved {
            return;
        }
        let Ok(created) = self.file.metadata() else {
            return;
        };
        let Ok(Some(current)) = stamp_at(self.directory, self.name) else {
            return;
        };
        if created.dev() == current.dev && created.ino() == current.ino {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            loop {
                let path = std::env::temp_dir().join(format!(
                    "naru-bookmarks-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("{error}"),
                }
            }
        }
        fn file(&self, name: &CStr) -> PathBuf {
            self.0.join(name.to_str().unwrap())
        }
        fn write(&self, name: &CStr, bytes: &[u8]) {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(self.file(name))
                .unwrap();
            file.write_all(bytes).unwrap();
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn document(count: usize) -> Document {
        Document {
            version: 1,
            next_id: count as u64 + 1,
            items: (1..=count)
                .map(|id| Bookmark {
                    id: id as u64,
                    title: format!("이름 {id}"),
                    url: format!("https://example.com/{id}"),
                })
                .collect(),
        }
    }

    #[test]
    fn add_rename_duplicate_and_delete_survive_fresh_loads() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0);
        assert!(store.writable());
        let first = store.add("HTTPS://example.com/a b", " 처음 ").unwrap();
        assert_eq!(first.id, 1);
        assert!(first.changed);
        assert!(
            store
                .add("https://example.com/a%20b", "다른 이름")
                .is_ok_and(|result| !result.changed && result.id == 1)
        );
        assert_eq!(store.items()[0].title, "처음");
        store.rename(1, "바꾼 이름").unwrap();
        drop(store);
        let mut store = Store::load(&directory.0);
        assert!(store.writable(), "{:?}", store.notice());
        assert_eq!(store.items()[0].title, "바꾼 이름");
        store.remove(1).unwrap();
        drop(store);
        let mut store = Store::load(&directory.0);
        assert!(store.writable(), "{:?}", store.notice());
        assert!(store.items().is_empty());
        assert!(directory.file(BACKUP).exists());
        assert_eq!(store.add("https://example.com/next", "다음").unwrap().id, 2);
        assert_eq!(
            fs::metadata(directory.file(PRIMARY)).unwrap().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn corrupt_primary_and_missing_primary_only_expose_backup_read_only() {
        let directory = Directory::new();
        let bytes = encode(&document(1)).unwrap();
        directory.write(BACKUP, &bytes);
        for primary in [
            None,
            Some(b"{broken".as_slice()),
            Some(br#"{"version":2,"nextId":1,"items":[]}"#.as_slice()),
        ] {
            if let Some(primary) = primary {
                directory.write(PRIMARY, primary);
            }
            let mut store = Store::load(&directory.0);
            assert!(!store.writable());
            assert_eq!(store.items().len(), 1);
            assert!(matches!(store.remove(1), Err(Error::ReadOnly(_))));
            assert_eq!(fs::read(directory.file(BACKUP)).unwrap(), bytes);
            if let Some(primary) = primary {
                assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), primary);
            } else {
                assert!(!directory.file(PRIMARY).exists());
            }
        }
    }

    #[test]
    fn malformed_backup_or_stale_pending_preserves_primary_and_blocks_writes() {
        for bad_name in [BACKUP, PENDING, BACKUP_PENDING] {
            let directory = Directory::new();
            let primary = encode(&document(1)).unwrap();
            directory.write(PRIMARY, &primary);
            directory.write(bad_name, b"keep this file");
            let mut store = Store::load(&directory.0);
            assert!(!store.writable());
            assert_eq!(store.items().len(), 1);
            assert!(store.rename(1, "새 이름").is_err());
            assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), primary);
            assert_eq!(
                fs::read(directory.file(bad_name)).unwrap(),
                b"keep this file"
            );
        }
    }

    #[test]
    fn invalid_structure_and_oversized_input_are_preserved() {
        let directory = Directory::new();
        let mut duplicate = document(2);
        duplicate.items[1].id = 1;
        let too_many = document(MAX_ITEMS + 1);
        for bytes in [
            serde_json::to_vec(&duplicate).unwrap(),
            serde_json::to_vec(&too_many).unwrap(),
            vec![b' '; MAX_FILE_BYTES + 1],
        ] {
            directory.write(PRIMARY, &bytes);
            let store = Store::load(&directory.0);
            assert!(!store.writable());
            assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), bytes);
        }
    }

    #[test]
    fn failed_writes_keep_the_previous_document_and_clean_owned_temps() {
        for stage in [
            Stage::Write,
            Stage::FileSync,
            Stage::BackupRename,
            Stage::BackupSync,
            Stage::PrimaryRename,
        ] {
            let directory = Directory::new();
            let mut store = Store::load(&directory.0);
            store.add("https://example.com", "원본").unwrap();
            let original = fs::read(directory.file(PRIMARY)).unwrap();
            store.fail_at = Some(stage);
            assert!(store.rename(1, "후보").is_err(), "{stage:?}");
            assert_eq!(store.items()[0].title, "원본");
            assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), original);
            assert!(!directory.file(PENDING).exists());
            assert!(!directory.file(BACKUP_PENDING).exists());
            store.fail_at = None;
            store.rename(1, "재시도").unwrap();
            assert_eq!(Store::load(&directory.0).items()[0].title, "재시도");
        }
    }

    #[test]
    fn directory_sync_failure_returns_committed_change_and_warning() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0);
        store.fail_at = Some(Stage::DirectorySync);
        let result = store.add("https://example.com", "반영됨").unwrap();
        assert!(result.changed);
        assert!(result.durability_warning.is_some());
        assert_eq!(store.items()[0].title, "반영됨");
        assert_eq!(Store::load(&directory.0).items()[0].title, "반영됨");
    }

    #[test]
    fn external_changes_and_symlinks_are_never_replaced() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0);
        directory.write(PRIMARY, &encode(&document(1)).unwrap());
        let bytes = fs::read(directory.file(PRIMARY)).unwrap();
        assert!(matches!(
            store.add("https://example.com/new", "새 항목"),
            Err(Error::ExternalChange)
        ));
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), bytes);
        let target = directory.0.join("target");
        fs::rename(directory.file(PRIMARY), &target).unwrap();
        std::os::unix::fs::symlink(&target, directory.file(PRIMARY)).unwrap();
        assert!(!Store::load(&directory.0).writable());
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert!(
            fs::symlink_metadata(directory.file(PRIMARY))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn restrictive_directory_and_file_permissions_are_required() {
        let directory = Directory::new();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!Store::load(&directory.0).writable());
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
        directory.write(PRIMARY, &encode(&document(1)).unwrap());
        fs::set_permissions(directory.file(PRIMARY), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!Store::load(&directory.0).writable());
    }

    #[test]
    fn model_limits_and_invalid_values_do_not_change_the_file() {
        let directory = Directory::new();
        directory.write(PRIMARY, &encode(&document(MAX_ITEMS)).unwrap());
        let mut store = Store::load(&directory.0);
        assert!(store.writable());
        let original = fs::read(directory.file(PRIMARY)).unwrap();
        assert!(matches!(
            store.add("https://example.com/new", "새 항목"),
            Err(Error::Limit(_))
        ));
        assert!(matches!(store.rename(1, "  "), Err(Error::InvalidTitle)));
        assert!(matches!(
            store.rename(1, &"한".repeat(257)),
            Err(Error::InvalidTitle)
        ));
        assert!(store.add("site:example.com", "검색어").is_err());
        assert!(store.remove(u64::MAX).is_err());
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), original);
        let mut large = document(300);
        for item in &mut large.items {
            item.url = format!("https://example.com/{}/{}", item.id, "a".repeat(7_500));
        }
        assert!(matches!(encode(&large), Err(Error::Limit(_))));
        let mut exhausted = document(0);
        exhausted.next_id = u64::MAX;
        directory.write(PRIMARY, &encode(&exhausted).unwrap());
        let mut store = Store::load(&directory.0);
        assert!(matches!(
            store.add("https://example.com", "이름"),
            Err(Error::Limit(_))
        ));
    }
}
