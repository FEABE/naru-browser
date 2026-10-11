//! One owned bookmark Store and one short-lived worker. AppKit only reads the
//! cached metadata; all file access finishes independently of UI callbacks.

use crate::bookmarks::{Bookmark, MAX_TITLE_CHARS, Mutation, Store};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

pub const MAX_ROWS: usize = 30;

pub struct Snapshot {
    pub ready: bool,
    pub busy: bool,
    pub writable: bool,
    pub total: usize,
    pub rows: Vec<Bookmark>,
    pub notice: Option<String>,
    pub path: String,
}

#[derive(Debug)]
pub struct Draft {
    pub id: Option<u64>,
    pub url: String,
    pub title: String,
}

#[derive(Debug)]
pub enum Outcome {
    Prepared(Draft),
    Changed {
        id: u64,
        changed: bool,
        durability_warning: Option<String>,
    },
}

#[derive(Default)]
struct Cache {
    ready: bool,
    writable: bool,
    items: Arc<[Bookmark]>,
    notice: Option<String>,
    path: String,
}

impl Cache {
    fn from_store(store: &Store) -> Self {
        Self {
            ready: true,
            writable: store.writable(),
            // One bounded cache refresh after an operation, not on browser events.
            items: Arc::from(store.items()),
            notice: store.notice().map(str::to_owned),
            path: store.path().to_string_lossy().into_owned(),
        }
    }

    fn snapshot(&self, query: &str, busy: bool) -> Snapshot {
        let query = crate::tab_search::bounded_query(query).to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        let rows = self
            .items
            .iter()
            .filter(|item| {
                if words.is_empty() {
                    return true;
                }
                let title = item.title.to_lowercase();
                let url = item.url.to_lowercase();
                words
                    .iter()
                    .all(|word| title.contains(word) || url.contains(word))
            })
            .take(MAX_ROWS)
            .cloned()
            .collect();
        Snapshot {
            ready: self.ready,
            busy,
            writable: self.writable && !busy,
            total: self.items.len(),
            rows,
            notice: self.notice.clone(),
            path: self.path.clone(),
        }
    }

    fn draft(&self, url: &str, page_title: &str) -> Result<Draft, String> {
        if !self.ready {
            return Err("북마크를 불러오는 중입니다.".into());
        }
        let url = crate::address::normalize_url(url).map_err(|error| error.to_string())?;
        if let Some(item) = self.items.iter().find(|item| item.url == url) {
            return Ok(Draft {
                id: Some(item.id),
                url,
                title: item.title.clone(),
            });
        }
        let title: String = page_title
            .trim()
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_TITLE_CHARS)
            .collect();
        let title = if title.is_empty() {
            url.chars().take(MAX_TITLE_CHARS).collect()
        } else {
            title
        };
        Ok(Draft {
            id: None,
            url,
            title,
        })
    }
}

enum Operation {
    Add { url: String, title: String },
    Rename { id: u64, title: String },
    Remove { id: u64 },
}

enum Work {
    Load(PathBuf),
    Change(Store, Operation),
}

struct Finished {
    store: Option<Store>,
    result: Result<Option<Mutation>, String>,
}

fn perform(work: Work) -> Finished {
    match work {
        Work::Load(path) => Finished {
            store: Some(Store::load(&path)),
            result: Ok(None),
        },
        Work::Change(mut store, operation) => {
            let result = match operation {
                Operation::Add { url, title } => store.add(&url, &title),
                Operation::Rename { id, title } => store.rename(id, &title),
                Operation::Remove { id } => store.remove(id),
            }
            .map(Some)
            .map_err(|error| error.to_string());
            Finished {
                store: Some(store),
                result,
            }
        }
    }
}

struct Job {
    token: u64,
    request: Option<u64>,
    receiver: mpsc::Receiver<Finished>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Runtime {
    started: bool,
    closing: Arc<AtomicBool>,
    next_token: u64,
    store: Option<Store>,
    cache: Cache,
    job: Option<Job>,
}

impl Runtime {
    fn initialize(&mut self, path: PathBuf) {
        if self.started || self.closing.load(Ordering::Acquire) {
            return;
        }
        self.started = true;
        self.cache.path = path
            .join(crate::bookmarks::FILE_NAME)
            .to_string_lossy()
            .into_owned();
        if let Err(error) = self.start(Work::Load(path), None) {
            self.cache.notice = Some(error);
        }
    }

    fn mutate(&mut self, request: u64, operation: Operation) -> Result<(), String> {
        if self.closing.load(Ordering::Acquire) {
            return Err("브라우저가 종료 중입니다.".into());
        }
        if self.job.is_some() {
            return Err("북마크 저장이 진행 중입니다. 잠시 후 다시 시도해 주세요.".into());
        }
        let store = self
            .store
            .take()
            .ok_or_else(|| "북마크 저장소가 아직 준비되지 않았습니다.".to_owned())?;
        self.start(Work::Change(store, operation), Some(request))
    }

    fn start(&mut self, work: Work, request: Option<u64>) -> Result<(), String> {
        let Some(token) = self.next_token.checked_add(1) else {
            if let Work::Change(store, _) = work {
                self.store = Some(store);
            }
            return Err("북마크 작업 번호를 만들 수 없습니다.".into());
        };
        self.next_token = token;
        let closing = self.closing.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        // Retain ownership if creating the short-lived thread itself fails.
        let slot = Arc::new(Mutex::new(Some(work)));
        let worker_slot = slot.clone();
        let spawned = thread::Builder::new().name("naru-bookmarks".into()).spawn(move || {
            let finished = catch_unwind(AssertUnwindSafe(|| {
                let work = worker_slot.lock().expect("bookmark work lock").take().expect("one bookmark worker");
                perform(work)
            })).unwrap_or_else(|_| Finished {
                store: None,
                result: Err("북마크 작업이 중단됐습니다. 저장 결과가 불명확하므로 자동 재시도하지 말고 앱을 다시 실행해 확인해 주세요.".into()),
            });
            // The one-slot send does not depend on the UI consuming the result.
            let _ = sender.send(finished);
            if !closing.load(Ordering::Acquire) { post_completion(token); }
        });
        match spawned {
            Ok(handle) => {
                self.job = Some(Job {
                    token,
                    request,
                    receiver,
                    handle: Some(handle),
                });
                Ok(())
            }
            Err(error) => {
                if let Some(Work::Change(store, _)) =
                    slot.lock().ok().and_then(|mut slot| slot.take())
                {
                    self.store = Some(store);
                }
                Err(format!("북마크 작업을 시작할 수 없습니다: {error}"))
            }
        }
    }

    fn complete(&mut self, token: u64) -> Option<(u64, Result<Outcome, String>)> {
        if self.closing.load(Ordering::Acquire)
            || self.job.as_ref().is_none_or(|job| job.token != token)
        {
            return None;
        }
        let finished = match self.job.as_ref()?.receiver.try_recv() {
            Ok(finished) => finished,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Finished {
                store: None,
                result: Err(
                    "북마크 작업의 완료 결과를 받지 못했습니다. 재실행 후 파일을 확인해 주세요."
                        .into(),
                ),
            },
        };
        let mut job = self.job.take()?;
        // All file work and the result send already finished. The remaining
        // worker code only posts this task and exits; it never waits for the UI.
        if let Some(handle) = job.handle.take() {
            let _ = handle.join();
        }
        if let Some(store) = finished.store {
            self.cache = Cache::from_store(&store);
            self.store = Some(store);
        } else {
            self.cache.ready = false;
            self.cache.writable = false;
        }
        let outcome = match finished.result {
            Ok(Some(result)) => {
                if result.durability_warning.is_some() {
                    self.cache.notice = result.durability_warning.clone();
                }
                Ok(Outcome::Changed {
                    id: result.id,
                    changed: result.changed,
                    durability_warning: result.durability_warning,
                })
            }
            Ok(None) => return None,
            Err(error) => {
                self.cache.notice = Some(error.clone());
                Err(error)
            }
        };
        job.request.map(|request| (request, outcome))
    }

    fn shutdown(&mut self) {
        self.closing.store(true, Ordering::Release);
        if let Some(mut job) = self.job.take() {
            if let Some(handle) = job.handle.take() {
                let _ = handle.join();
            }
            // Dropping the result only happens after the disk operation has
            // finished. No CEF task must run for shutdown to preserve a save.
            if let Ok(finished) = job.receiver.try_recv() {
                match finished.result {
                    Err(error) => eprintln!("{error}"),
                    Ok(Some(Mutation {
                        durability_warning: Some(warning),
                        ..
                    })) => eprintln!("{warning}"),
                    _ => {}
                }
            }
        }
        self.store = None;
        self.cache = Cache::default();
    }
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default()); }

#[cfg(not(test))]
use cef::*;

#[cfg(not(test))]
wrap_task! {
    struct BookmarkCompletionTask { token: u64 }
    impl Task { fn execute(&self) { complete(self.token); } }
}

fn post_completion(token: u64) {
    #[cfg(not(test))]
    {
        let mut task = BookmarkCompletionTask::new(token);
        if post_task(ThreadId::UI, Some(&mut task)) == 0 {
            eprintln!("Bookmark completion could not be posted; shutdown will join the writer.");
        }
    }
    #[cfg(test)]
    let _ = token;
}

#[cfg(not(test))]
fn complete(token: u64) {
    let result = RUNTIME.with(|runtime| runtime.borrow_mut().complete(token));
    crate::browser::notify_panel();
    if let Some((request, result)) = result {
        deliver(request, result);
    }
}

fn deliver(request: u64, result: Result<Outcome, String>) {
    #[cfg(not(test))]
    crate::control_panel::bookmark_completed(request, result);
    #[cfg(test)]
    let _ = (request, result);
}

/// Call after CEF initialization and before entering its message loop.
pub fn initialize(data_dir: PathBuf) {
    RUNTIME.with(|runtime| runtime.borrow_mut().initialize(data_dir));
    crate::browser::notify_panel();
}

pub fn snapshot(query: &str) -> Snapshot {
    RUNTIME.with(|runtime| {
        let runtime = runtime.borrow();
        runtime.cache.snapshot(query, runtime.job.is_some())
    })
}

/// Capture the draft now; subsequent tab navigation cannot change its URL.
pub fn prepare(request: u64, tab: i32) {
    let result = crate::browser::bookmark_page(tab)
        .and_then(|(url, title)| {
            RUNTIME.with(|runtime| {
                let runtime = runtime.borrow();
                if runtime.job.is_some() {
                    return Err("북마크 작업이 진행 중입니다.".into());
                }
                runtime.cache.draft(&url, &title)
            })
        })
        .map(Outcome::Prepared);
    deliver(request, result);
}

fn mutate(request: u64, operation: Operation) {
    let result = RUNTIME.with(|runtime| runtime.borrow_mut().mutate(request, operation));
    crate::browser::notify_panel();
    if let Err(error) = result {
        deliver(request, Err(error));
    }
}

pub fn add(request: u64, url: String, title: String) {
    mutate(request, Operation::Add { url, title });
}
pub fn rename(request: u64, id: u64, title: String) {
    mutate(request, Operation::Rename { id, title });
}
pub fn remove(request: u64, id: u64) {
    mutate(request, Operation::Remove { id });
}

pub fn open(id: u64) -> Result<(), String> {
    let url = RUNTIME
        .with(|runtime| {
            runtime
                .borrow()
                .cache
                .items
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.url.clone())
        })
        .ok_or_else(|| "북마크를 찾을 수 없습니다. 목록을 다시 확인해 주세요.".to_owned())?;
    crate::browser::open_bookmark_url(&url)
}

/// Call before CEF shutdown. A write already started always reaches completion.
pub fn shutdown() {
    RUNTIME.with(|runtime| runtime.borrow_mut().shutdown());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::DirBuilderExt, sync::atomic::AtomicU64};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            loop {
                let path = std::env::temp_dir().join(format!(
                    "naru-bookmark-runtime-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("{error}"),
                }
            }
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn finish(runtime: &mut Runtime) -> Option<(u64, Result<Outcome, String>)> {
        let job = runtime.job.as_mut().unwrap();
        job.handle.take().unwrap().join().unwrap();
        let token = job.token;
        runtime.complete(token)
    }

    #[test]
    fn cached_search_is_bounded_and_draft_keeps_stored_name() {
        let cache = Cache {
            ready: true,
            writable: true,
            items: (1..=80)
                .map(|id| Bookmark {
                    id,
                    title: format!("저장 이름 {id}"),
                    url: format!("https://example.com/{id}"),
                })
                .collect::<Vec<_>>()
                .into(),
            ..Default::default()
        };
        let snapshot = cache.snapshot("저장 example", false);
        assert_eq!(snapshot.total, 80);
        assert_eq!(snapshot.rows.len(), MAX_ROWS);
        assert!(cache.snapshot("없음", false).rows.is_empty());
        let draft = cache
            .draft("https://example.com/1", "변경된 페이지 제목")
            .unwrap();
        assert_eq!(draft.id, Some(1));
        assert_eq!(draft.title, "저장 이름 1");
        assert!(cache.draft("about:blank", "새 탭").is_err());
    }

    #[test]
    fn sole_writer_rejects_overlap_and_ignores_stale_completion() {
        let directory = Directory::new();
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone());
        assert!(runtime.cache.snapshot("", true).busy);
        assert!(finish(&mut runtime).is_none());
        assert!(runtime.cache.ready);
        runtime
            .mutate(
                10,
                Operation::Add {
                    url: "https://example.com".into(),
                    title: "고정 이름".into(),
                },
            )
            .unwrap();
        let token = runtime.job.as_ref().unwrap().token;
        assert!(runtime.mutate(11, Operation::Remove { id: 1 }).is_err());
        assert!(runtime.complete(token - 1).is_none());
        assert_eq!(runtime.job.as_ref().unwrap().token, token);
        let (request, outcome) = finish(&mut runtime).unwrap();
        assert_eq!(request, 10);
        assert!(matches!(
            outcome.unwrap(),
            Outcome::Changed {
                id: 1,
                changed: true,
                ..
            }
        ));
        assert_eq!(runtime.cache.items[0].title, "고정 이름");
        assert!(runtime.complete(token).is_none());
        runtime.shutdown();
    }

    #[test]
    fn failed_mutation_returns_store_and_shutdown_waits_for_next_save() {
        let directory = Directory::new();
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone());
        finish(&mut runtime);
        runtime
            .mutate(
                1,
                Operation::Add {
                    url: "https://example.com".into(),
                    title: " ".into(),
                },
            )
            .unwrap();
        assert!(finish(&mut runtime).unwrap().1.is_err());
        assert!(runtime.cache.items.is_empty());
        assert!(runtime.store.is_some());
        runtime
            .mutate(
                2,
                Operation::Add {
                    url: "https://example.com".into(),
                    title: "종료 전 저장".into(),
                },
            )
            .unwrap();
        runtime.shutdown();
        let store = Store::load(&directory.0);
        assert!(store.writable());
        assert_eq!(store.items()[0].title, "종료 전 저장");
        assert!(runtime.mutate(3, Operation::Remove { id: 1 }).is_err());
    }
}
