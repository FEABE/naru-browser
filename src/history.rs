//! Bounded, single-writer visit history; no threads, UI objects, or backups.
//! Move Store to a worker for disk access. Wall-clock retention uses saturating
//! age: a clock rollback/future timestamp has age zero until the clock catches up.

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
pub const MAX_UPDATES: usize = 64;
pub const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
pub const FILE_NAME: &str = "history-v1.json";
const PRIMARY: &CStr = c"history-v1.json";
const PENDING: &CStr = c".history-v1.json.pending";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryEntry {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub visited_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    Visit {
        url: String,
        title: String,
        visited_at_ms: u64,
    },
    Title {
        url: String,
        title: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    version: u32,
    next_id: u64,
    #[serde(deserialize_with = "read_items")]
    items: Vec<HistoryEntry>,
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
        if self.version != 1 || self.next_id == 0 {
            return Err(Error::InvalidData(
                "방문 기록 버전이나 다음 ID가 잘못됐습니다.".into(),
            ));
        }
        if self.items.len() > MAX_ITEMS {
            return Err(Error::Limit("방문 기록 항목 수가 1,000개를 초과합니다."));
        }
        let mut ids = BTreeSet::new();
        let mut urls = BTreeSet::new();
        for item in &self.items {
            if item.id == 0 || item.id >= self.next_id || !ids.insert(item.id) {
                return Err(Error::InvalidData(
                    "방문 기록 ID가 잘못됐거나 중복됐습니다.".into(),
                ));
            }
            if address::normalize_url(&item.url)? != item.url
                || clean_title(&item.title, &item.url) != item.title
            {
                return Err(Error::InvalidData(
                    "방문 기록 제목이나 주소가 저장 형식에 맞지 않습니다.".into(),
                ));
            }
            if !urls.insert(item.url.as_str()) {
                return Err(Error::InvalidData("방문 기록 주소가 중복됐습니다.".into()));
            }
        }
        Ok(())
    }

    fn retain_recent(&mut self, now_ms: u64) {
        self.items
            .retain(|item| !expired(item.visited_at_ms, now_ms));
        self.items.sort_unstable_by(|a, b| {
            b.visited_at_ms
                .cmp(&a.visited_at_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        self.items.truncate(MAX_ITEMS);
    }

    fn fit_bytes(&mut self) -> Result<(), Error> {
        // Count serialization without allocating an oversized output or encoding
        // the entire document once per discarded item.
        let mut size = encoded_len(self)?;
        while size > MAX_FILE_BYTES {
            let oldest = self
                .items
                .pop()
                .ok_or(Error::Limit("방문 기록 파일이 2MiB를 초과합니다."))?;
            size -= encoded_len(&oldest)? + usize::from(!self.items.is_empty());
        }
        Ok(())
    }
}

fn expired(visited_at_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(visited_at_ms) > RETENTION_MS
}

fn read_items<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<HistoryEntry>, D::Error> {
    struct Items;
    impl<'de> de::Visitor<'de> for Items {
        type Value = Vec<HistoryEntry>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("at most 1,000 history entries")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == MAX_ITEMS {
                    return Err(de::Error::custom("history item limit exceeded"));
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
    InvalidUrl(AddressError),
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
            Self::InvalidUrl(error) => write!(formatter, "{error}"),
            Self::Limit(message) => formatter.write_str(message),
            Self::ReadOnly(reason) => write!(formatter, "방문 기록은 읽기 전용입니다. {reason}"),
            Self::InvalidData(reason) => formatter.write_str(reason),
            Self::UnsafePath => {
                formatter.write_str("방문 기록 경로의 파일 종류, 소유자 또는 권한을 확인해 주세요.")
            }
            Self::ExternalChange => {
                formatter.write_str("방문 기록 파일이 다른 곳에서 변경됐습니다. 다시 읽어 주세요.")
            }
            Self::Io(error) => write!(formatter, "방문 기록 파일 처리에 실패했습니다: {error}"),
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
    pub changed: bool,
    /// Rename committed; this warning must not be treated as a failed mutation.
    pub durability_warning: Option<String>,
}

#[derive(Debug)]
pub struct Store {
    directory_path: PathBuf,
    path: PathBuf,
    directory: Option<File>,
    document: Document,
    primary_stamp: Option<Stamp>,
    needs_cleanup: bool,
    read_only: Option<String>,
    #[cfg(test)]
    fail_at: Option<Stage>,
}

impl Store {
    /// Read only: absent, damaged, and interrupted files are never created,
    /// repaired, promoted, or deleted here. Expired valid entries are hidden.
    pub fn load(data_dir: &Path, now_ms: u64) -> Self {
        let mut store = Self {
            directory_path: data_dir.to_owned(),
            path: data_dir.join(FILE_NAME),
            directory: None,
            document: Document::default(),
            primary_stamp: None,
            needs_cleanup: false,
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
        match read_document(&directory, PRIMARY) {
            Ok(Some(loaded)) => {
                store.primary_stamp = Some(loaded.stamp);
                let mut retained = loaded.document.clone();
                retained.retain_recent(now_ms);
                store.needs_cleanup = retained != loaded.document;
                store.document = retained;
            }
            Ok(None) => {}
            Err(error) => store.read_only = Some(format!("방문 기록 원본을 보존했습니다. {error}")),
        }
        if let Err(error) = ensure_no_pending(&directory) {
            if store.read_only.is_none() {
                store.read_only = Some(error.to_string());
            }
        }
        store.directory = Some(directory);
        store
    }

    pub fn items(&self) -> &[HistoryEntry] {
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

    pub fn record(&mut self, url: &str, title: &str, now_ms: u64) -> Result<Mutation, Error> {
        self.apply_updates(
            &[Update::Visit {
                url: url.into(),
                title: title.into(),
                visited_at_ms: now_ms,
            }],
            now_ms,
        )
    }

    pub fn update_title(&mut self, url: &str, title: &str, now_ms: u64) -> Result<Mutation, Error> {
        self.apply_updates(
            &[Update::Title {
                url: url.into(),
                title: title.into(),
            }],
            now_ms,
        )
    }

    /// Apply a bounded arrival-ordered batch in one serialization/rename. Visit
    /// timestamps are captured at event receipt, not moved forward on flush.
    pub fn apply_updates(&mut self, updates: &[Update], now_ms: u64) -> Result<Mutation, Error> {
        self.require_writable()?;
        if updates.len() > MAX_UPDATES {
            return Err(Error::Limit("방문 기록 한 번의 갱신은 최대 64개입니다."));
        }
        let mut candidate = self.document.clone();
        candidate.retain_recent(now_ms);
        for update in updates {
            let (url, name) = match update {
                Update::Visit { url, title, .. } | Update::Title { url, title } => (url, title),
            };
            let url = address::normalize_url(url)?;
            let name = clean_title(name, &url);
            match update {
                Update::Visit { visited_at_ms, .. } => {
                    if expired(*visited_at_ms, now_ms) {
                        continue;
                    }
                    if let Some(item) = candidate.items.iter_mut().find(|item| item.url == url) {
                        item.title = name;
                        item.visited_at_ms = *visited_at_ms;
                    } else {
                        let id = candidate.next_id;
                        candidate.next_id = id
                            .checked_add(1)
                            .ok_or(Error::Limit("새 방문 기록 ID를 만들 수 없습니다."))?;
                        candidate.items.push(HistoryEntry {
                            id,
                            url,
                            title: name,
                            visited_at_ms: *visited_at_ms,
                        });
                    }
                }
                Update::Title { .. } => {
                    if let Some(item) = candidate.items.iter_mut().find(|item| item.url == url) {
                        item.title = name;
                    }
                }
            }
        }
        candidate.retain_recent(now_ms);
        candidate.fit_bytes()?;
        if candidate == self.document && !self.needs_cleanup {
            return Ok(Mutation {
                changed: false,
                durability_warning: None,
            });
        }
        self.save(candidate, true)
    }

    /// Always verify and replace the on-disk file, even for an empty cache.
    /// Otherwise an external write or expired on-disk rows could survive a
    /// reported successful clear. IDs are never recycled, including after clear.
    pub fn clear(&mut self, _now_ms: u64) -> Result<Mutation, Error> {
        self.require_writable()?;
        let mut candidate = self.document.clone();
        candidate.items.clear();
        let changed =
            !self.document.items.is_empty() || self.needs_cleanup || self.primary_stamp.is_none();
        self.save(candidate, changed)
    }

    fn require_writable(&self) -> Result<(), Error> {
        match &self.read_only {
            Some(reason) => Err(Error::ReadOnly(reason.clone())),
            None => Ok(()),
        }
    }

    fn save(&mut self, candidate: Document, changed: bool) -> Result<Mutation, Error> {
        let result = self.persist(candidate);
        if let Err(
            error @ (Error::ExternalChange
            | Error::UnsafePath
            | Error::InvalidUrl(_)
            | Error::InvalidData(_)
            | Error::Limit(_)),
        ) = &result
        {
            self.read_only = Some(error.to_string());
        }
        result.map(|durability_warning| Mutation {
            changed,
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
        // Cache retention can differ from the loaded file; compare file identity
        // and nanosecond modification/change stamps, not the pruned document.
        if current.as_ref().map(|loaded| loaded.stamp) != self.primary_stamp {
            return Err(Error::ExternalChange);
        }
        drop(current);
        self.checkpoint(Stage::Write)?;
        let mut pending = PendingFile::new(directory, PENDING)?;
        pending.file.write_all(&bytes)?;
        pending.file.flush()?;
        self.checkpoint(Stage::FileSync)?;
        pending.file.sync_all()?;
        self.checkpoint(Stage::PrimaryRename)?;
        verify_directory_path(directory, &self.directory_path)?;
        require_stamp(directory, PRIMARY, self.primary_stamp)?;
        let stamp = pending.replace(PRIMARY)?;
        // From this point the mutation is committed; directory sync is a warning.
        self.primary_stamp = Some(stamp);
        self.document = candidate;
        self.needs_cleanup = false;
        let sync = self
            .checkpoint(Stage::DirectorySync)
            .and_then(|_| directory.sync_all().map_err(Error::from));
        Ok(sync.err().map(|error| {
            format!("방문 기록 파일에 반영됐으나 영속 동기화 확인에 실패했습니다. {error}")
        }))
    }

    fn checkpoint(&self, stage: Stage) -> Result<(), Error> {
        #[cfg(test)]
        if self.fail_at == Some(stage) {
            return Err(Error::Io(io::Error::other(
                "injected history storage failure",
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
    PrimaryRename,
    DirectorySync,
}

fn clean_title(value: &str, url: &str) -> String {
    let value: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_TITLE_CHARS)
        .collect();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        url.chars().take(MAX_TITLE_CHARS).collect()
    } else {
        trimmed.to_owned()
    }
}

fn encoded_len(value: &impl Serialize) -> Result<usize, Error> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| io::Error::other("history size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)
        .map_err(|error| Error::InvalidData(error.to_string()))?;
    Ok(counter.0)
}

fn encode(document: &Document) -> Result<Vec<u8>, Error> {
    let size = encoded_len(document)?;
    if size > MAX_FILE_BYTES {
        return Err(Error::Limit(
            "방문 기록 파일은 최대 2MiB까지 저장할 수 있습니다.",
        ));
    }
    serde_json::to_vec(document).map_err(|error| Error::InvalidData(error.to_string()))
}

// Same directory-handle, no-follow, private-file and atomic-pending discipline
// as bookmarks. Kept local because history must never create a backup copy.
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
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Err(Error::UnsafePath),
        Err(error) => return Err(Error::Io(error)),
    };
    let metadata = file.metadata()?;
    if !private_file(&metadata) {
        return Err(Error::UnsafePath);
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(Error::Limit(
            "방문 기록 파일이 2MiB를 초과합니다. 원본을 보존했습니다.",
        ));
    }
    let stamp = Stamp::new(&metadata);
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Limit("방문 기록 파일이 2MiB를 초과합니다."));
    }
    if stamp != Stamp::new(&file.metadata()?) {
        return Err(Error::ExternalChange);
    }
    let document: Document = serde_json::from_slice(&bytes).map_err(|error| {
        // Parser messages can quote arbitrary field names from the file.
        Error::InvalidData(format!(
            "방문 기록 JSON을 읽을 수 없습니다(행 {}, 열 {}).",
            error.line(),
            error.column()
        ))
    })?;
    document.validate()?;
    Ok(Some(Loaded { document, stamp }))
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
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => Err(Error::UnsafePath),
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
    for name in [PENDING] {
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
    const NOW: u64 = RETENTION_MS * 2;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            loop {
                let path = std::env::temp_dir().join(format!(
                    "naru-history-{}-{}",
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
                .rev()
                .map(|id| HistoryEntry {
                    id: id as u64,
                    url: format!("https://example.test/{id}"),
                    title: format!("방문 {id}"),
                    visited_at_ms: NOW + id as u64,
                })
                .collect(),
        }
    }
    fn visit(url: &str, title: &str, time: u64) -> Update {
        Update::Visit {
            url: url.into(),
            title: title.into(),
            visited_at_ms: time,
        }
    }

    #[test]
    fn visits_normalize_deduplicate_and_keep_stable_ids_across_reload() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0, NOW);
        assert!(store.writable());
        assert!(fs::read_dir(&directory.0).unwrap().next().is_none());
        store
            .record("HTTPS://example.test/a b?q=one#section", "  A\n\0B  ", NOW)
            .unwrap();
        store
            .record("https://example.test/second", "Second", NOW + 1)
            .unwrap();
        store
            .record("https://example.test/a%20b?q=one#section", "Again", NOW + 2)
            .unwrap();
        assert_eq!(store.items().len(), 2);
        assert_eq!(store.items()[0].id, 1);
        assert_eq!(store.items()[0].visited_at_ms, NOW + 2);
        assert_eq!(store.items()[0].title, "Again");
        assert_eq!(
            store.items()[0].url,
            "https://example.test/a%20b?q=one#section"
        );
        let loaded = Store::load(&directory.0, NOW + 2);
        assert!(loaded.writable(), "{:?}", loaded.notice());
        assert_eq!(loaded.items(), store.items());
        assert_eq!(
            fs::metadata(directory.file(PRIMARY)).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn title_cleanup_is_bounded_and_title_events_never_create_visits() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0, NOW);
        store
            .record("https://example.test/one", "  A\n\0B  ", NOW)
            .unwrap();
        assert_eq!(store.items()[0].title, "AB");
        store
            .update_title("https://example.test/one", &"한".repeat(300), NOW + 100)
            .unwrap();
        assert_eq!(store.items()[0].title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(store.items()[0].visited_at_ms, NOW);
        let original = fs::read(directory.file(PRIMARY)).unwrap();
        assert!(
            !store
                .update_title("https://example.test/missing", "Late title", NOW)
                .unwrap()
                .changed
        );
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), original);
        store
            .update_title("https://example.test/one", "\n\r\0  ", NOW)
            .unwrap();
        assert_eq!(store.items()[0].title, "https://example.test/one");
        store.clear(NOW).unwrap();
        assert!(
            !store
                .update_title("https://example.test/one", "Late after clear", NOW)
                .unwrap()
                .changed
        );
        assert!(Store::load(&directory.0, NOW).items().is_empty());
    }

    #[test]
    fn clear_commits_empty_file_preserves_next_id_and_leaves_no_backup() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0, NOW);
        store
            .record("https://example.test/old", "Old", NOW)
            .unwrap();
        assert!(store.clear(NOW + 1).unwrap().changed);
        let mut loaded = Store::load(&directory.0, NOW + 1);
        assert!(loaded.writable());
        assert!(loaded.items().is_empty());
        assert!(!loaded.clear(NOW + 1).unwrap().changed);
        loaded
            .record("https://example.test/new", "New", NOW + 2)
            .unwrap();
        assert_eq!(loaded.items()[0].id, 2);
        let names: Vec<_> = fs::read_dir(&directory.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(FILE_NAME)]);
    }

    #[test]
    fn retention_is_read_only_on_load_then_pruned_on_next_mutation() {
        let directory = Directory::new();
        let mut stored = document(3);
        stored.items[0].visited_at_ms = NOW + 100; // Clock rollback: age zero.
        stored.items[1].visited_at_ms = NOW - RETENTION_MS; // Boundary retained.
        stored.items[2].visited_at_ms = NOW - RETENTION_MS - 1;
        let original = encode(&stored).unwrap();
        directory.write(PRIMARY, &original);
        let mut store = Store::load(&directory.0, NOW);
        assert_eq!(
            store
                .items()
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), original);
        assert!(
            store
                .update_title("https://example.test/missing", "No resurrection", NOW)
                .unwrap()
                .changed
        );
        let loaded = Store::load(&directory.0, NOW);
        assert!(loaded.writable());
        assert_eq!(loaded.items(), store.items());
        assert_ne!(fs::read(directory.file(PRIMARY)).unwrap(), original);
        assert!(
            store
                .apply_updates(&[], NOW + RETENTION_MS + 101)
                .unwrap()
                .changed
        );
        assert!(store.items().is_empty());
    }

    #[test]
    fn clear_removes_expired_disk_rows_even_when_cache_is_empty() {
        let directory = Directory::new();
        directory.write(PRIMARY, &encode(&document(1)).unwrap());
        let mut store = Store::load(&directory.0, NOW + RETENTION_MS + 2);
        assert!(store.items().is_empty());
        assert!(store.clear(NOW + RETENTION_MS + 2).unwrap().changed);
        let file: Document =
            serde_json::from_slice(&fs::read(directory.file(PRIMARY)).unwrap()).unwrap();
        assert!(file.items.is_empty());
        assert_eq!(file.next_id, 2);
    }

    #[test]
    fn batch_uses_event_times_and_rejects_invalid_batch_without_partial_changes() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0, NOW);
        let updates = [
            visit("https://example.test/one", "First", NOW - 2),
            visit("https://example.test/two", "Second", NOW - 1),
            Update::Title {
                url: "https://example.test/one".into(),
                title: "Final".into(),
            },
            visit(
                "https://example.test/expired",
                "Old",
                NOW - RETENTION_MS - 1,
            ),
        ];
        store.apply_updates(&updates, NOW).unwrap();
        assert_eq!(store.items().len(), 2);
        assert_eq!(store.items()[0].visited_at_ms, NOW - 1);
        assert_eq!(store.items()[1].title, "Final");
        assert_eq!(store.items()[1].visited_at_ms, NOW - 2);
        assert_eq!(store.document.next_id, 3);
        let original = fs::read(directory.file(PRIMARY)).unwrap();
        assert!(
            store
                .apply_updates(
                    &[
                        visit("https://example.test/valid", "Would be valid", NOW),
                        visit("https://user:password@example.test/", "Invalid", NOW),
                    ],
                    NOW
                )
                .is_err()
        );
        assert!(
            store
                .apply_updates(
                    &vec![visit("https://example.test/one", "Too many", NOW); MAX_UPDATES + 1],
                    NOW
                )
                .is_err()
        );
        assert_eq!(store.items().len(), 2);
        assert_eq!(store.document.next_id, 3);
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), original);
        assert!(store.writable());
    }

    #[test]
    fn count_limit_discards_oldest_but_not_recently_revisited_entries() {
        let directory = Directory::new();
        directory.write(PRIMARY, &encode(&document(MAX_ITEMS)).unwrap());
        let mut store = Store::load(&directory.0, NOW + 2000);
        store
            .apply_updates(
                &[
                    visit("https://example.test/1", "Revisited", NOW + 2001),
                    visit("https://example.test/new", "New", NOW + 2002),
                ],
                NOW + 2002,
            )
            .unwrap();
        assert_eq!(store.items().len(), MAX_ITEMS);
        assert_eq!(store.items()[0].id, 1001);
        assert_eq!(store.items()[1].id, 1);
        assert!(!store.items().iter().any(|entry| entry.id == 2));
        assert_eq!(Store::load(&directory.0, NOW + 2002).items(), store.items());
    }

    #[test]
    fn serialized_byte_limit_prunes_oldest_with_exact_utf8_json_size() {
        let directory = Directory::new();
        let mut stored = document(260);
        for item in &mut stored.items {
            item.url = format!("https://example.test/{}/{}", item.id, "a".repeat(7500));
            item.title = "한".repeat(20);
        }
        directory.write(PRIMARY, &encode(&stored).unwrap());
        let mut store = Store::load(&directory.0, NOW + 1000);
        let updates: Vec<_> = (0..MAX_UPDATES)
            .map(|id| {
                visit(
                    &format!("https://example.test/new-{id}/{}", "b".repeat(7500)),
                    &"\"한\\".repeat(80),
                    NOW + 1001 + id as u64,
                )
            })
            .collect();
        store.apply_updates(&updates, NOW + 2000).unwrap();
        let bytes = fs::read(directory.file(PRIMARY)).unwrap();
        assert!(bytes.len() <= MAX_FILE_BYTES);
        assert!(store.items().len() < 260 + MAX_UPDATES);
        assert_eq!(store.items()[0].visited_at_ms, NOW + 1064);
        assert!(!store.items().iter().any(|entry| entry.id == 1));
        assert_eq!(encoded_len(&store.document).unwrap(), bytes.len());
        assert_eq!(Store::load(&directory.0, NOW + 2000).items(), store.items());
    }

    #[test]
    fn corrupt_unknown_oversized_and_duplicate_data_is_preserved_read_only() {
        let mut duplicate_id = document(2);
        duplicate_id.items[1].id = duplicate_id.items[0].id;
        let mut duplicate_url = document(2);
        duplicate_url.items[1].url = duplicate_url.items[0].url.clone();
        for bytes in [
            b"{broken".to_vec(),
            br#"{"version":2,"nextId":1,"items":[]}"#.to_vec(),
            br#"{"version":1,"nextId":1,"items":[],"unexpected":true}"#.to_vec(),
            serde_json::to_vec(&duplicate_id).unwrap(),
            serde_json::to_vec(&duplicate_url).unwrap(),
            serde_json::to_vec(&document(MAX_ITEMS + 1)).unwrap(),
            vec![b' '; MAX_FILE_BYTES + 1],
        ] {
            let directory = Directory::new();
            directory.write(PRIMARY, &bytes);
            let mut store = Store::load(&directory.0, NOW);
            assert!(!store.writable());
            assert!(store.items().is_empty());
            assert!(store.clear(NOW).is_err());
            assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), bytes);
        }
    }

    #[test]
    fn stale_pending_is_never_promoted_removed_or_overwritten() {
        for with_primary in [false, true] {
            let directory = Directory::new();
            if with_primary {
                directory.write(PRIMARY, &encode(&document(1)).unwrap());
            }
            directory.write(PENDING, b"keep interrupted data");
            let mut store = Store::load(&directory.0, NOW);
            assert!(!store.writable());
            assert_eq!(store.items().len(), usize::from(with_primary));
            assert!(store.clear(NOW).is_err());
            assert_eq!(
                fs::read(directory.file(PENDING)).unwrap(),
                b"keep interrupted data"
            );
            assert_eq!(directory.file(PRIMARY).exists(), with_primary);
        }
        let directory = Directory::new();
        let mut store = Store::load(&directory.0, NOW);
        directory.write(PENDING, b"arrived after load");
        assert!(
            store
                .record("https://example.test/one", "One", NOW)
                .is_err()
        );
        assert!(!store.writable());
        assert_eq!(
            fs::read(directory.file(PENDING)).unwrap(),
            b"arrived after load"
        );
    }

    #[test]
    fn failed_write_and_clear_preserve_cache_file_and_remove_only_owned_pending() {
        for stage in [Stage::Write, Stage::FileSync, Stage::PrimaryRename] {
            for clearing in [false, true] {
                let directory = Directory::new();
                let mut store = Store::load(&directory.0, NOW);
                store
                    .record("https://example.test/original", "Original", NOW)
                    .unwrap();
                let original = fs::read(directory.file(PRIMARY)).unwrap();
                let items = store.items().to_vec();
                store.fail_at = Some(stage);
                let result = if clearing {
                    store.clear(NOW)
                } else {
                    store.record("https://example.test/new", "Candidate", NOW + 1)
                };
                assert!(result.is_err(), "{stage:?}, clearing={clearing}");
                assert_eq!(store.items(), items);
                assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), original);
                assert!(!directory.file(PENDING).exists());
                store.fail_at = None;
                store.clear(NOW).unwrap();
                assert!(Store::load(&directory.0, NOW).items().is_empty());
            }
        }
    }

    #[test]
    fn directory_sync_failure_reports_committed_clear_and_record_with_warning() {
        let directory = Directory::new();
        let mut store = Store::load(&directory.0, NOW);
        store.fail_at = Some(Stage::DirectorySync);
        let result = store
            .record("https://example.test/one", "One", NOW)
            .unwrap();
        assert!(result.changed && result.durability_warning.is_some());
        assert_eq!(Store::load(&directory.0, NOW).items(), store.items());
        let result = store.clear(NOW).unwrap();
        assert!(result.changed && result.durability_warning.is_some());
        assert!(Store::load(&directory.0, NOW).items().is_empty());
    }

    #[test]
    fn external_changes_including_empty_cache_clear_are_never_replaced() {
        for clearing in [false, true] {
            let directory = Directory::new();
            let mut store = Store::load(&directory.0, NOW);
            let external = encode(&document(1)).unwrap();
            directory.write(PRIMARY, &external);
            let result = if clearing {
                store.clear(NOW)
            } else {
                store.record("https://example.test/new", "New", NOW)
            };
            assert!(matches!(result, Err(Error::ExternalChange)));
            assert!(!store.writable());
            assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), external);
        }
        let directory = Directory::new();
        directory.write(PRIMARY, &encode(&document(1)).unwrap());
        let mut store = Store::load(&directory.0, NOW);
        let external = encode(&document(2)).unwrap();
        directory.write(PRIMARY, &external);
        assert!(matches!(store.clear(NOW), Err(Error::ExternalChange)));
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), external);
    }

    #[test]
    fn symlink_hardlink_and_public_permissions_block_writes_without_touching_targets() {
        let directory = Directory::new();
        let target = directory.0.join("target");
        directory.write(c"target", &encode(&document(1)).unwrap());
        let original = fs::read(&target).unwrap();
        std::os::unix::fs::symlink(&target, directory.file(PRIMARY)).unwrap();
        assert!(!Store::load(&directory.0, NOW).writable());
        fs::remove_file(directory.file(PRIMARY)).unwrap();
        fs::hard_link(&target, directory.file(PRIMARY)).unwrap();
        assert!(!Store::load(&directory.0, NOW).writable());
        fs::remove_file(directory.file(PRIMARY)).unwrap();
        directory.write(PRIMARY, &original);
        fs::set_permissions(directory.file(PRIMARY), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!Store::load(&directory.0, NOW).writable());
        fs::set_permissions(directory.file(PRIMARY), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!Store::load(&directory.0, NOW).writable());
        assert_eq!(fs::read(target).unwrap(), original);
    }

    #[test]
    fn replacing_data_directory_or_exhausting_ids_never_discards_history() {
        let root = Directory::new();
        let path = root.0.join("profile");
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let mut store = Store::load(&path, NOW);
        store
            .record("https://example.test/one", "Original", NOW)
            .unwrap();
        fs::rename(&path, root.0.join("moved")).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        assert!(matches!(store.clear(NOW), Err(Error::UnsafePath)));
        assert!(fs::read_dir(&path).unwrap().next().is_none());
        assert_eq!(Store::load(&root.0.join("moved"), NOW).items().len(), 1);
        let directory = Directory::new();
        let mut full = document(1);
        full.next_id = u64::MAX;
        let bytes = encode(&full).unwrap();
        directory.write(PRIMARY, &bytes);
        let mut store = Store::load(&directory.0, NOW);
        assert!(matches!(
            store.record("https://example.test/new", "New", NOW),
            Err(Error::Limit(_))
        ));
        assert_eq!(fs::read(directory.file(PRIMARY)).unwrap(), bytes);
        store
            .update_title("https://example.test/1", "Still editable", NOW)
            .unwrap();
        assert_eq!(store.items()[0].title, "Still editable");
    }
}
