//! One short-lived file worker, a bounded metadata cache and a coalescing queue.
//! Clearing increments a generation before accepting any further observations.

use crate::history::{HistoryEntry, Mutation, RETENTION_MS, Store, Update};
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
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_ROWS: usize = 40;
const MAX_PENDING_BYTES: usize = 256 * 1024;

pub struct Snapshot {
    pub ready: bool,
    pub busy: bool,
    pub writable: bool,
    pub clearing: bool,
    pub total: usize,
    pub rows: Vec<HistoryEntry>,
    pub notice: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn recent(entry: &HistoryEntry, now: u64) -> bool {
    now.saturating_sub(entry.visited_at_ms) <= RETENTION_MS
}

#[derive(Default)]
struct Cache {
    ready: bool,
    writable: bool,
    items: Arc<[HistoryEntry]>,
    notice: Option<String>,
}

impl Cache {
    fn from_store(store: &Store) -> Self {
        Self {
            ready: true,
            writable: store.writable(),
            items: Arc::from(store.items()),
            notice: store.notice().map(str::to_owned),
        }
    }

    fn snapshot(&self, query: &str, busy: bool, clearing: bool, now: u64) -> Snapshot {
        let query = crate::tab_search::bounded_query(query).to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        let total = if clearing {
            0
        } else {
            self.items.iter().filter(|item| recent(item, now)).count()
        };
        let rows = if clearing {
            Vec::new()
        } else {
            self.items
                .iter()
                .filter(|item| recent(item, now))
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
                .collect()
        };
        Snapshot {
            ready: self.ready,
            busy,
            writable: self.writable,
            clearing,
            total,
            rows,
            notice: if clearing {
                Some("방문 기록을 지우는 중입니다.".into())
            } else {
                self.notice.clone()
            },
        }
    }
}

fn update_url(update: &Update) -> &str {
    match update {
        Update::Visit { url, .. } | Update::Title { url, .. } => url,
    }
}

fn update_bytes(update: &Update) -> usize {
    match update {
        Update::Visit { url, title, .. } | Update::Title { url, title } => url.len() + title.len(),
    }
}

enum Work {
    Load(PathBuf, u64),
    Change(Store, Vec<Update>, u64),
    Clear(Store, u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Load,
    Change,
    Clear,
}

impl Work {
    fn kind(&self) -> Kind {
        match self {
            Self::Load(..) => Kind::Load,
            Self::Change(..) => Kind::Change,
            Self::Clear(..) => Kind::Clear,
        }
    }
}

struct Finished {
    store: Option<Store>,
    result: Result<Option<Mutation>, String>,
}

fn perform(work: Work) -> Finished {
    match work {
        Work::Load(path, now) => {
            let mut store = Store::load(&path, now);
            // Expired disk records must not survive indefinitely in an idle app.
            // The store writes only if pruning changed a valid original file.
            let result = if store.writable() {
                store
                    .apply_updates(&[], now)
                    .map(Some)
                    .map_err(|error| error.to_string())
            } else {
                Ok(None)
            };
            Finished {
                store: Some(store),
                result,
            }
        }
        Work::Change(mut store, updates, now) => {
            let result = store
                .apply_updates(&updates, now)
                .map(Some)
                .map_err(|error| error.to_string());
            Finished {
                store: Some(store),
                result,
            }
        }
        Work::Clear(mut store, now) => {
            let result = store
                .clear(now)
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
    kind: Kind,
    receiver: mpsc::Receiver<Finished>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Runtime {
    started: bool,
    closing: Arc<AtomicBool>,
    generation: u64,
    next_token: u64,
    store: Option<Store>,
    cache: Cache,
    job: Option<Job>,
    pending: Vec<Update>,
    pending_bytes: usize,
    clear_requested: bool,
    faulted: bool,
    queue_notice: Option<String>,
}

impl Runtime {
    fn initialize(&mut self, path: PathBuf, now: u64) {
        if self.started || self.closing.load(Ordering::Acquire) {
            return;
        }
        self.started = true;
        self.generation = 1;
        self.start(Work::Load(path, now));
    }

    fn observation_generation(&self) -> Option<u64> {
        (self.started
            && !self.faulted
            && !self.closing.load(Ordering::Acquire)
            && (!self.cache.ready || self.cache.writable))
            .then_some(self.generation)
    }

    fn fail(&mut self, error: String) {
        self.faulted = true;
        self.cache.ready = true;
        self.cache.writable = false;
        self.cache.notice = Some(error);
        self.pending.clear();
        self.pending_bytes = 0;
        self.clear_requested = false;
    }

    fn start(&mut self, work: Work) {
        let Some(token) = self.next_token.checked_add(1) else {
            self.recover_unstarted(work);
            self.fail("방문 기록 작업 번호를 만들 수 없습니다.".into());
            return;
        };
        self.next_token = token;
        let kind = work.kind();
        let closing = self.closing.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let slot = Arc::new(Mutex::new(Some(work)));
        let worker_slot = slot.clone();
        let spawned = thread::Builder::new()
            .name("naru-history".into())
            .spawn(move || {
                let finished = catch_unwind(AssertUnwindSafe(|| {
                    let work = worker_slot
                        .lock()
                        .expect("history work lock")
                        .take()
                        .expect("one history worker");
                    perform(work)
                }))
                .unwrap_or_else(|_| Finished {
                    store: None,
                    result: Err(
                        "방문 기록 저장이 중단됐습니다. 다시 실행해 파일 상태를 확인해 주세요."
                            .into(),
                    ),
                });
                let _ = sender.send(finished);
                if !closing.load(Ordering::Acquire) {
                    post_completion(token);
                }
            });
        match spawned {
            Ok(handle) => {
                self.job = Some(Job {
                    token,
                    kind,
                    receiver,
                    handle: Some(handle),
                })
            }
            Err(error) => {
                if let Some(work) = slot.lock().ok().and_then(|mut slot| slot.take()) {
                    self.recover_unstarted(work);
                }
                self.fail(format!("방문 기록 저장을 시작할 수 없습니다: {error}"));
            }
        }
    }

    fn recover_unstarted(&mut self, work: Work) {
        if let Work::Change(store, _, _) | Work::Clear(store, _) = work {
            self.store = Some(store);
        }
    }

    fn flush(&mut self, now: u64) {
        if self.job.is_some() || self.faulted {
            return;
        }
        if !self.clear_requested && self.pending.is_empty() {
            return;
        }
        let Some(store) = self.store.take() else {
            return;
        };
        if self.clear_requested {
            self.start(Work::Clear(store, now));
        } else {
            self.pending_bytes = 0;
            let updates = std::mem::take(&mut self.pending);
            self.start(Work::Change(store, updates, now));
        }
    }

    fn enqueue(&mut self, generation: u64, update: Update, now: u64) -> bool {
        if self.observation_generation() != Some(generation) {
            return false;
        }
        if let Some(index) = self
            .pending
            .iter()
            .position(|old| update_url(old) == update_url(&update))
        {
            self.pending_bytes -= update_bytes(&self.pending[index]);
            match (&mut self.pending[index], update) {
                (Update::Visit { title, .. }, Update::Title { title: new, .. }) => *title = new,
                (old, new) => *old = new,
            }
            self.pending_bytes += update_bytes(&self.pending[index]);
        } else {
            let bytes = update_bytes(&update);
            if bytes > MAX_PENDING_BYTES {
                return false;
            }
            if matches!(update, Update::Title { .. })
                && (self.pending.len() == crate::history::MAX_UPDATES
                    || self.pending_bytes + bytes > MAX_PENDING_BYTES)
            {
                return false;
            }
            while self.pending.len() >= crate::history::MAX_UPDATES
                || self.pending_bytes + bytes > MAX_PENDING_BYTES
            {
                self.pending_bytes -= update_bytes(&self.pending.remove(0));
                self.queue_notice =
                    Some("방문 기록 요청이 많아 오래된 대기 항목을 제외했습니다.".into());
            }
            self.pending_bytes += bytes;
            self.pending.push(update);
        }
        // A larger title can grow a coalesced entry past the byte budget.
        while self.pending_bytes > MAX_PENDING_BYTES {
            self.pending_bytes -= update_bytes(&self.pending.remove(0));
            self.queue_notice =
                Some("방문 기록 요청이 많아 오래된 대기 항목을 제외했습니다.".into());
        }
        self.flush(now);
        true
    }

    fn clear_all(&mut self, now: u64) -> Result<(), String> {
        if !self.cache.ready
            || !self.cache.writable
            || self.faulted
            || self.closing.load(Ordering::Acquire)
        {
            return Err("지금은 방문 기록을 지울 수 없습니다. 저장 상태를 확인해 주세요.".into());
        }
        if self.clear_requested {
            return Err("방문 기록을 지우는 중입니다.".into());
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("방문 기록 세대를 갱신할 수 없습니다.")?;
        self.pending.clear();
        self.pending_bytes = 0;
        self.clear_requested = true;
        self.queue_notice = None;
        self.flush(now);
        if self.faulted {
            return Err(self.cache.notice.clone().unwrap_or_default());
        }
        Ok(())
    }

    fn complete(&mut self, token: u64, now: u64) -> bool {
        if self.job.as_ref().is_none_or(|job| job.token != token) {
            return false;
        }
        let finished = match self.job.as_ref().unwrap().receiver.try_recv() {
            Ok(finished) => finished,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Finished {
                store: None,
                result: Err(
                    "방문 기록 저장 결과를 받지 못했습니다. 다시 실행해 확인해 주세요.".into(),
                ),
            },
        };
        let mut job = self.job.take().unwrap();
        if let Some(handle) = job.handle.take() {
            let _ = handle.join();
        }
        if let Some(store) = finished.store {
            self.cache = Cache::from_store(&store);
            self.store = Some(store);
        } else {
            self.cache = Cache::default();
        }
        match finished.result {
            Err(error) => self.fail(error),
            Ok(mutation) => {
                if let Some(warning) = mutation.and_then(|value| value.durability_warning) {
                    self.cache.notice = Some(warning);
                }
                if job.kind == Kind::Clear {
                    self.clear_requested = false;
                }
                if !self.cache.writable {
                    self.pending.clear();
                    self.pending_bytes = 0;
                    self.clear_requested = false;
                } else {
                    self.flush(now);
                }
            }
        }
        true
    }

    fn poll(&mut self, now: u64) -> bool {
        self.job
            .as_ref()
            .map(|job| job.token)
            .is_some_and(|token| self.complete(token, now))
    }

    fn url(&self, id: u64, generation: u64, now: u64) -> Option<String> {
        if self.clear_requested
            || self.generation != generation
            || self.closing.load(Ordering::Acquire)
        {
            return None;
        }
        self.cache
            .items
            .iter()
            .find(|item| item.id == id && recent(item, now))
            .map(|item| item.url.clone())
    }

    fn shutdown(&mut self, now: u64) {
        self.closing.store(true, Ordering::Release);
        // The producer is closed. At most the in-flight job, one clear and one
        // bounded batch remain. Workers never wait for a CEF completion task.
        loop {
            self.flush(now);
            let Some(job) = self.job.as_mut() else {
                break;
            };
            let token = job.token;
            if let Some(handle) = job.handle.take() {
                let _ = handle.join();
            }
            self.complete(token, now);
        }
        if let Some(notice) = &self.cache.notice {
            eprintln!("방문 기록: {notice}");
        }
        self.store = None;
        self.cache = Cache::default();
        self.pending.clear();
        self.pending_bytes = 0;
    }
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default()); }

#[cfg(not(test))]
use cef::*;

#[cfg(not(test))]
wrap_task! {
    struct HistoryCompletionTask { token: u64 }
    impl Task { fn execute(&self) {
        let changed = RUNTIME.with(|runtime| runtime.borrow_mut().complete(self.token, now_ms()));
        if changed { crate::browser::notify_panel(); }
    } }
}

fn post_completion(token: u64) {
    #[cfg(not(test))]
    {
        let mut task = HistoryCompletionTask::new(token);
        if post_task(ThreadId::UI, Some(&mut task)) == 0 {
            eprintln!(
                "History completion could not be posted; the next access or shutdown will collect it."
            );
        }
    }
    #[cfg(test)]
    let _ = token;
}

fn poll() {
    let changed = RUNTIME.with(|runtime| runtime.borrow_mut().poll(now_ms()));
    if changed {
        crate::browser::notify_panel();
    }
}

pub fn initialize(data_dir: PathBuf) {
    RUNTIME.with(|runtime| runtime.borrow_mut().initialize(data_dir, now_ms()));
}

pub fn generation() -> Option<u64> {
    poll();
    RUNTIME.with(|runtime| runtime.borrow().observation_generation())
}

pub fn snapshot(query: &str) -> Snapshot {
    poll();
    RUNTIME.with(|runtime| {
        let runtime = runtime.borrow();
        let mut snapshot = runtime.cache.snapshot(
            query,
            runtime.job.is_some() || runtime.clear_requested,
            runtime.clear_requested,
            now_ms(),
        );
        if snapshot.notice.is_none() {
            snapshot.notice.clone_from(&runtime.queue_notice);
        }
        snapshot
    })
}

fn normalized(url: &str, title: &str) -> Option<(String, String)> {
    let url = crate::address::normalize_url(url).ok()?;
    let title: String = title
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(crate::history::MAX_TITLE_CHARS)
        .collect();
    let title = if title.is_empty() {
        url.chars().take(crate::history::MAX_TITLE_CHARS).collect()
    } else {
        title
    };
    Some((url, title))
}

pub fn record(generation: u64, url: String, title: String) -> bool {
    let Some((url, title)) = normalized(&url, &title) else {
        return false;
    };
    poll();
    let now = now_ms();
    RUNTIME.with(|runtime| {
        runtime.borrow_mut().enqueue(
            generation,
            Update::Visit {
                url,
                title,
                visited_at_ms: now,
            },
            now,
        )
    })
}

pub fn update_title(generation: u64, url: String, title: String) -> bool {
    let Some((url, title)) = normalized(&url, &title) else {
        return false;
    };
    poll();
    RUNTIME.with(|runtime| {
        runtime
            .borrow_mut()
            .enqueue(generation, Update::Title { url, title }, now_ms())
    })
}

pub fn clear_all() -> Result<(), String> {
    poll();
    let result = RUNTIME.with(|runtime| runtime.borrow_mut().clear_all(now_ms()));
    crate::browser::notify_panel();
    result
}

pub fn open(id: u64) -> Result<(), String> {
    poll();
    let (generation, url) = RUNTIME
        .with(|runtime| {
            let runtime = runtime.borrow();
            runtime
                .url(id, runtime.generation, now_ms())
                .map(|url| (runtime.generation, url))
        })
        .ok_or_else(|| "방문 기록이 삭제되거나 만료됐습니다. 다시 검색해 주세요.".to_owned())?;
    crate::browser::open_history_url(&url, || {
        RUNTIME.with(|runtime| {
            runtime.borrow().url(id, generation, now_ms()).as_deref() == Some(url.as_str())
        })
    })
}

pub fn shutdown() {
    RUNTIME.with(|runtime| runtime.borrow_mut().shutdown(now_ms()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::atomic::AtomicU64};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
    const NOW: u64 = 1_800_000_000_000;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "naru-history-runtime-{}-{}-{}",
                std::process::id(),
                now_ms(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn visit(url: &str, title: &str, time: u64) -> Update {
        Update::Visit {
            url: url.into(),
            title: title.into(),
            visited_at_ms: time,
        }
    }
    fn finish_one(runtime: &mut Runtime, now: u64) {
        let job = runtime.job.as_mut().expect("one pending job");
        if let Some(handle) = job.handle.take() {
            handle.join().unwrap();
        }
        // Unit workers do not post CEF tasks. Access must recover the result.
        assert!(runtime.poll(now));
    }
    fn drain(runtime: &mut Runtime, now: u64) {
        for _ in 0..5 {
            if runtime.job.is_none() {
                return;
            }
            finish_one(runtime, now);
        }
        panic!("history queue did not drain within the bounded job count");
    }

    #[test]
    fn loading_coalesces_bounded_events_and_publishes_only_after_storage() {
        let directory = Directory::new();
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone(), NOW);
        let generation = runtime.observation_generation().unwrap();
        assert!(runtime.enqueue(
            generation,
            visit("https://example.test/a", "first", NOW),
            NOW
        ));
        assert!(runtime.enqueue(
            generation,
            Update::Title {
                url: "https://example.test/a".into(),
                title: "latest title".into()
            },
            NOW
        ));
        assert_eq!(runtime.pending.len(), 1);
        assert!(runtime.cache.items.is_empty());
        finish_one(&mut runtime, NOW);
        assert!(runtime.cache.ready);
        assert!(runtime.cache.items.is_empty());
        assert!(runtime.pending.is_empty());
        drain(&mut runtime, NOW);
        assert_eq!(runtime.cache.items.len(), 1);
        assert_eq!(runtime.cache.items[0].title, "latest title");
        let token = runtime.next_token;
        assert!(!runtime.complete(token, NOW));
        assert_eq!(
            Store::load(directory.path(), NOW).items(),
            &*runtime.cache.items
        );
        runtime.shutdown(NOW);
    }

    #[test]
    fn clear_orders_inflight_writes_and_rejects_old_visits_titles_and_ids() {
        let directory = Directory::new();
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone(), NOW);
        drain(&mut runtime, NOW);
        let old = runtime.observation_generation().unwrap();
        runtime.enqueue(old, visit("https://example.test/old", "old", NOW), NOW);
        drain(&mut runtime, NOW);
        let old_id = runtime.cache.items[0].id;
        assert!(runtime.url(old_id, old, NOW).is_some());
        runtime.enqueue(
            old,
            visit("https://example.test/inflight", "in flight", NOW + 1),
            NOW + 1,
        );
        runtime.enqueue(
            old,
            visit("https://example.test/queued", "queued", NOW + 2),
            NOW + 2,
        );
        runtime.clear_all(NOW + 2).unwrap();
        assert!(runtime.clear_requested);
        assert!(runtime.pending.is_empty());
        assert!(runtime.url(old_id, old, NOW + 2).is_none());
        let clearing = runtime
            .cache
            .snapshot("", true, runtime.clear_requested, NOW);
        assert!(clearing.clearing && clearing.busy && clearing.rows.is_empty());
        assert!(!runtime.enqueue(old, visit("https://example.test/late", "late", NOW), NOW));
        assert!(!runtime.enqueue(
            old,
            Update::Title {
                url: "https://example.test/old".into(),
                title: "late title".into()
            },
            NOW
        ));
        assert!(runtime.clear_all(NOW).is_err());
        let new = runtime.observation_generation().unwrap();
        runtime.enqueue(
            new,
            visit("https://example.test/new", "new", NOW + 3),
            NOW + 3,
        );
        finish_one(&mut runtime, NOW + 3);
        assert!(runtime.clear_requested);
        assert!(
            runtime
                .job
                .as_ref()
                .is_some_and(|job| job.kind == Kind::Clear)
        );
        finish_one(&mut runtime, NOW + 3);
        assert!(!runtime.clear_requested);
        assert!(runtime.cache.items.is_empty());
        drain(&mut runtime, NOW + 3);
        assert_eq!(runtime.cache.items.len(), 1);
        assert_eq!(runtime.cache.items[0].url, "https://example.test/new");
        assert_ne!(runtime.cache.items[0].id, old_id);
        assert!(runtime.url(old_id, new, NOW + 3).is_none());
        runtime.shutdown(NOW + 3);
        let store = Store::load(directory.path(), NOW + 3);
        assert_eq!(store.items().len(), 1);
        assert_eq!(store.items()[0].title, "new");
    }

    #[test]
    fn queue_bounds_are_enforced_and_shutdown_drains_accepted_records() {
        let directory = Directory::new();
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone(), NOW);
        let generation = runtime.observation_generation().unwrap();
        for id in 0..100 {
            let url = format!("https://example.test/{id}/{}", "x".repeat(7_900));
            assert!(runtime.enqueue(generation, visit(&url, "bounded", NOW + id), NOW + id));
            assert!(runtime.pending.len() <= crate::history::MAX_UPDATES);
            assert!(runtime.pending_bytes <= MAX_PENDING_BYTES);
        }
        assert!(runtime.queue_notice.is_some());
        let expected = runtime.pending.len();
        assert!(expected > 0 && expected < 100);
        runtime.shutdown(NOW + 100);
        let store = Store::load(directory.path(), NOW + 100);
        assert_eq!(store.items().len(), expected);
        assert!(store.items().iter().any(|item| item.url.contains("/99/")));
        assert!(runtime.observation_generation().is_none());
        assert!(runtime.job.is_none());
    }

    #[test]
    fn expired_records_disappear_from_lookup_search_and_disk_on_load() {
        let directory = Directory::new();
        let mut store = Store::load(directory.path(), NOW);
        store
            .record("https://example.test/expired", "old", NOW)
            .unwrap();
        let old_id = store.items()[0].id;
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone(), NOW + RETENTION_MS + 1);
        drain(&mut runtime, NOW + RETENTION_MS + 1);
        assert!(runtime.cache.items.is_empty());
        assert!(
            runtime
                .url(old_id, runtime.generation, NOW + RETENTION_MS + 1)
                .is_none()
        );
        assert!(Store::load(directory.path(), NOW).items().is_empty());
        runtime.shutdown(NOW + RETENTION_MS + 1);
    }

    #[test]
    fn external_change_during_clear_is_reported_without_claiming_deletion() {
        let directory = Directory::new();
        let mut runtime = Runtime::default();
        runtime.initialize(directory.0.clone(), NOW);
        drain(&mut runtime, NOW);
        let generation = runtime.observation_generation().unwrap();
        runtime.enqueue(
            generation,
            visit("https://example.test/keep", "keep", NOW),
            NOW,
        );
        drain(&mut runtime, NOW);
        let file = directory.path().join(crate::history::FILE_NAME);
        let bytes = fs::read(&file).unwrap();
        let external = [bytes.as_slice(), b"\n"].concat();
        fs::write(&file, &external).unwrap();
        runtime.clear_all(NOW).unwrap();
        drain(&mut runtime, NOW);
        assert!(runtime.faulted);
        assert!(!runtime.clear_requested);
        assert!(!runtime.cache.writable);
        assert!(runtime.cache.notice.is_some());
        assert_eq!(runtime.cache.items.len(), 1);
        assert_eq!(fs::read(file).unwrap(), external);
        assert!(runtime.observation_generation().is_none());
        runtime.shutdown(NOW);
    }
}
