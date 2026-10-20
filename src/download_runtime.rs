//! CEF download callbacks stay on the UI thread; rows contain copied metadata only.

use crate::downloads::{
    MAX_ACTIVE, MAX_PATH_BYTES, MAX_RECORDS, Model, Progress, Snapshot, SourceTarget, Status,
};
use cef::*;
use std::{cell::RefCell, collections::BTreeMap, path::Path};

#[derive(Default)]
struct Callbacks {
    before: Option<BeforeDownloadCallback>,
    item: Option<DownloadItemCallback>,
    // Only the user's accepted Save As path, never DownloadItem::full_path.
    approved_path: Option<String>,
    cancel_requested: bool,
    cancel_sent: bool,
}

impl Callbacks {
    fn cancel(self) {
        if let Some(callback) = &self.item {
            callback.cancel();
        }
        // Dropping an unexecuted BeforeDownloadCallback also cancels it.
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dialog {
    id: u32,
    sequence: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct Retry {
    scope: Option<crate::protocol::TaskScope>,
    path: String,
}

#[derive(Clone)]
struct RequestAttempt {
    target: SourceTarget,
    url: String,
    get: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct RestartRequest {
    source_id: u32,
    target: SourceTarget,
    scope: Option<crate::protocol::TaskScope>,
    url: String,
    sequence: u64,
}

struct PendingRestart {
    request: RestartRequest,
    cef_id: Option<u32>,
    canceled: bool,
}

impl PendingRestart {
    fn canceled_update(&self, id: u32) -> bool {
        self.canceled && self.cef_id == Some(id)
    }
}

#[derive(Default)]
struct Runtime {
    model: Model,
    callbacks: BTreeMap<u32, Callbacks>,
    dialog: Option<Dialog>,
    next_dialog: u64,
    notice: Option<String>,
    shutting_down: bool,
    requests: Vec<RequestAttempt>,
    restart: Option<PendingRestart>,
    next_restart: u64,
}

impl Runtime {
    fn has_capacity(&self) -> bool {
        let reserved = usize::from(self.restart.is_some());
        self.model.activity().0 + reserved < MAX_ACTIVE
            && self.callbacks.len() + reserved < MAX_ACTIVE
    }

    fn restart_entry(&self, id: u32) -> Result<&crate::downloads::Entry, String> {
        if self.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        if self.dialog.is_some() {
            return Err("저장 위치 선택 창을 먼저 닫아 주세요.".into());
        }
        if self.restart.is_some() {
            return Err("앞선 다시 받기 요청의 응답을 기다리고 있습니다.".into());
        }
        if !self.has_capacity() {
            return Err("다운로드는 대기와 진행을 합쳐 최대 4개까지 받을 수 있습니다.".into());
        }
        let entry = self
            .model
            .entry(id)
            .ok_or_else(|| "다시 받을 다운로드가 목록에서 지워졌습니다.".to_owned())?;
        if entry.restart_url().is_none() {
            return Err("이 파일은 원본 페이지에서 다시 받아 주세요.".into());
        }
        let target = entry
            .source_target
            .as_ref()
            .ok_or_else(|| "다운로드를 요청한 탭을 확인할 수 없습니다.".to_owned())?;
        if self
            .requests
            .iter()
            .any(|request| request.target.tab == target.tab)
        {
            return Err("원본 탭의 앞선 다운로드 요청을 먼저 기다려 주세요.".into());
        }
        Ok(entry)
    }

    fn restart_request(&self, id: u32) -> Result<RestartRequest, String> {
        let entry = self.restart_entry(id)?;
        Ok(RestartRequest {
            source_id: id,
            target: entry.source_target.clone().unwrap(),
            scope: entry.scope.clone(),
            url: entry.restart_url().unwrap().to_owned(),
            sequence: self
                .next_restart
                .checked_add(1)
                .ok_or_else(|| "다시 받기 요청을 더 만들 수 없습니다.".to_owned())?,
        })
    }

    fn consume_request(&mut self, tab: i32, url: &str, target: &SourceTarget) -> bool {
        let matching = self
            .requests
            .iter()
            .enumerate()
            .filter(|(_, request)| request.target.tab == tab && request.url == url)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return false;
        }
        if matching.len() > 1 {
            for index in &matching {
                self.requests[*index].get = false;
            }
        }
        let request = self.requests.remove(matching[0]);
        matching.len() == 1 && request.get && &request.target == target
    }

    fn completed_file_path(&self, id: u32) -> Result<&str, String> {
        if self.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        if self.dialog.is_some() {
            return Err("저장 위치 선택 창을 먼저 닫아 주세요.".into());
        }
        self.model
            .entry(id)
            .and_then(|entry| entry.completed_file_path())
            .ok_or_else(|| "저장 완료된 파일의 위치를 확인할 수 없습니다.".into())
    }

    fn update(&mut self, id: u32, progress: Progress) -> bool {
        let approved = self.callbacks.get(&id).and_then(|handles| {
            // An outstanding Save As callback has not accepted a destination.
            handles
                .before
                .is_none()
                .then_some(handles.approved_path.as_deref())
                .flatten()
        });
        self.model.update_with_approval(id, progress, approved)
    }

    fn source_target(&self, id: u32) -> Result<SourceTarget, String> {
        if self.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        if self.dialog.is_some() {
            return Err("저장 위치 선택 창을 먼저 닫아 주세요.".into());
        }
        self.model
            .entry(id)
            .and_then(|entry| entry.source_target.clone())
            .ok_or_else(|| "다운로드를 요청한 탭을 확인할 수 없습니다.".into())
    }

    fn retry(&self, id: u32, tab: i32) -> Result<Option<Retry>, &'static str> {
        if self.shutting_down {
            return Err("브라우저를 종료하고 있습니다.");
        }
        let Some(entry) = self.model.entry(id) else {
            return if self.callbacks.contains_key(&id) {
                Err("다운로드의 저장 요청이 만료되었습니다.")
            } else {
                Ok(None)
            };
        };
        if entry.tab != tab {
            return Err("다운로드를 요청한 탭이 변경되었습니다.");
        }
        let handles = self
            .callbacks
            .get(&id)
            .filter(|handles| !handles.cancel_requested && handles.before.is_none())
            .filter(|_| entry.status == Status::Downloading)
            .ok_or("현재 상태에서는 다운로드를 다시 시작할 수 없습니다.")?;
        let path = handles
            .approved_path
            .as_ref()
            .ok_or("사용자가 선택한 저장 경로가 없습니다.")?;
        Ok(Some(Retry {
            scope: entry.scope.clone(),
            path: path.clone(),
        }))
    }

    fn dismiss(&mut self, dialog: Dialog) -> Option<crate::downloads::Entry> {
        if self.dialog != Some(dialog) {
            return None;
        }
        // Cancellation and metadata removal never release an open native sheet.
        self.dialog = None;
        self.model
            .entry(dialog.id)
            .filter(|entry| entry.status == Status::Choosing)
            .cloned()
    }
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default()); }

pub fn snapshot() -> Snapshot {
    let (mut snapshot, targets) = RUNTIME.with(|slot| {
        let state = slot.borrow();
        let mut snapshot = state.model.snapshot();
        snapshot.notice = state.notice.clone();
        for row in &mut snapshot.rows {
            row.can_reveal_file &= !state.shutting_down && state.dialog.is_none();
            row.can_restart &= state.restart_entry(row.id).is_ok();
            if state
                .restart
                .as_ref()
                .is_some_and(|pending| pending.request.source_id == row.id)
            {
                row.detail = if state.restart.as_ref().unwrap().canceled {
                    "다시 받기 응답 대기 중, 원본 탭을 닫으면 정리됩니다.".into()
                } else {
                    "다시 받기 요청 중".into()
                };
            }
        }
        snapshot.active_count += usize::from(state.restart.is_some());
        if state.dialog.is_some() {
            for row in &mut snapshot.rows {
                row.can_choose = false;
            }
            if snapshot.notice.is_none() {
                snapshot.notice = Some("저장 위치 선택 창을 먼저 닫아 주세요.".into());
            }
        }
        let targets = snapshot
            .rows
            .iter()
            .map(|row| state.source_target(row.id).ok())
            .collect::<Vec<_>>();
        (snapshot, targets)
    });
    // Browser state validation is metadata-only and runs outside RUNTIME.
    // At most MAX_RECORDS targets are copied, only for a requested snapshot.
    for (row, target) in snapshot.rows.iter_mut().zip(targets) {
        row.can_reveal = target
            .as_ref()
            .is_some_and(|target| crate::browser::validate_download_source(target).is_ok());
        row.can_restart &= target
            .as_ref()
            .is_some_and(|target| crate::browser::validate_download_restart_source(target).is_ok());
    }
    snapshot
}

pub(crate) fn source_reveal_allowed() -> bool {
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        !state.shutting_down && state.dialog.is_none()
    })
}

pub(crate) fn reveal_source(id: u32) -> Result<i32, String> {
    let target = RUNTIME.with(|slot| slot.borrow().source_target(id))?;
    // Selection is explicit, does not restart a file, and may reenter either
    // runtime when revealing a previously hidden Chromium permission prompt.
    crate::browser::reveal_download_source(&target)
}

pub(crate) fn reveal_file(id: u32) -> Result<(), String> {
    let path = RUNTIME.with(|slot| slot.borrow().completed_file_path(id).map(str::to_owned))?;
    // A completed file outlives its source tab/task. No browser selection or
    // ownership change is needed, and native dispatch holds no runtime borrow.
    #[cfg(target_os = "macos")]
    return crate::mac::reveal_file(&path);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("파일 위치 표시는 macOS에서 지원합니다.".into())
    }
}

pub fn activity() -> (usize, usize) {
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        let (active, pending) = state.model.activity();
        let reserved = usize::from(state.restart.is_some());
        (active + reserved, pending + reserved)
    })
}

fn notice(message: &str) {
    RUNTIME.with(|slot| {
        slot.borrow_mut().notice = Some(message.chars().take(512).collect());
    });
    crate::browser::notify_panel();
}

pub(crate) fn can_accept(tab: i32, url: Option<&CefString>) -> bool {
    let url = request_url(url);
    let result = crate::browser::download_context(tab).and_then(|_| {
        RUNTIME.with(|slot| {
            let state = slot.borrow();
            if !state.shutting_down
                && state.has_capacity()
                && !state.restart.as_ref().is_some_and(|pending| {
                    pending.request.target.tab == tab && url.as_ref() == Some(&pending.request.url)
                })
            {
                Ok(())
            } else {
                Err("다운로드는 대기와 진행을 합쳐 최대 4개까지 받을 수 있습니다.".into())
            }
        })
    });
    if let Err(error) = result {
        notice(&error);
        return false;
    }
    true
}

fn bounded_text(value: &CefString, limit: usize) -> String {
    char::decode_utf16(value.as_slice().unwrap_or_default().iter().copied())
        .map(|value| value.unwrap_or(char::REPLACEMENT_CHARACTER))
        .take(limit)
        .collect()
}

fn bounded_path(value: &CefString) -> Result<String, String> {
    let mut path = String::new();
    for character in char::decode_utf16(value.as_slice().unwrap_or_default().iter().copied()) {
        let character =
            character.map_err(|_| "파일 경로의 문자 인코딩을 읽을 수 없습니다.".to_owned())?;
        if path.len() + character.len_utf8() > MAX_PATH_BYTES {
            return Err("파일 경로가 너무 깁니다.".into());
        }
        path.push(character);
    }
    Ok(path)
}

fn request_url(value: Option<&CefString>) -> Option<String> {
    value
        .and_then(|value| bounded_path(value).ok())
        .filter(|url| crate::downloads::restart_url_allowed(url))
}

/// CanDownload has the method but no download ID. Keep every matching GET/POST
/// candidate until OnBeforeDownload; overlapping candidates are never evidence
/// that a later URL-only request can safely reproduce the original request.
pub(crate) fn record_request(
    tab: i32,
    url: Option<&CefString>,
    method: Option<&CefString>,
) -> bool {
    let Ok(context) = crate::browser::download_context(tab) else {
        return false;
    };
    let target = context.source_target;
    let Some(url) = request_url(url) else {
        crate::browser::block_download_restart_provenance(&target);
        return true;
    };
    if crate::browser::validate_download_restart_source(&target).is_err() {
        return true;
    }
    let candidates = RUNTIME.with(|slot| {
        slot.borrow()
            .requests
            .iter()
            .map(|request| request.target.clone())
            .collect::<Vec<_>>()
    });
    let stale = candidates
        .into_iter()
        .filter(|target| crate::browser::validate_download_restart_source(target).is_err())
        .collect::<Vec<_>>();
    let blocked = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state
            .requests
            .retain(|request| !stale.contains(&request.target));
        if state.requests.len() == MAX_RECORDS {
            let mut blocked = state
                .requests
                .drain(..)
                .map(|request| request.target)
                .collect::<Vec<_>>();
            blocked.push(target);
            return blocked;
        }
        state.requests.push(RequestAttempt {
            target,
            url,
            get: method.is_some_and(|method| method.as_slice() == Some(&[71, 69, 84])),
        });
        Vec::new()
    });
    // Overflow removes retry evidence for these documents, not their ordinary
    // download permission. The per-tab generation stamp is constant-size.
    for target in blocked {
        crate::browser::block_download_restart_provenance(&target);
    }
    true
}

pub(crate) fn restart(id: u32) -> Result<(), String> {
    let request = RUNTIME.with(|slot| slot.borrow().restart_request(id))?;
    let host = crate::browser::restart_download_host(&request.target, request.scope.as_ref())?;
    let url = CefString::from(request.url.as_str());
    let pointer: *const cef::sys::cef_string_t = (&url).into();
    if pointer.is_null() {
        return Err("다시 받을 파일의 주소를 전달하지 못했습니다.".into());
    }
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.restart_request(id)? != request {
            return Err("다운로드 요청이 변경되었습니다.".to_owned());
        }
        state.next_restart = request.sequence;
        state.restart = Some(PendingRestart {
            request: request.clone(),
            cef_id: None,
            canceled: false,
        });
        state.notice =
            Some("다시 받기를 요청했습니다. 새 항목에서 저장 위치를 선택해 주세요.".into());
        Ok(())
    })?;
    let mut timeout = RestartTimeout::new(request.sequence);
    let ready = post_delayed_task(ThreadId::UI, Some(&mut timeout), 30_000) != 0;
    let validation =
        crate::browser::validate_download_restart_source(&request.target).and_then(|()| {
            crate::browser::validate_download(request.target.tab, request.scope.as_ref())
        });
    if !ready || validation.is_err() {
        let error = validation
            .err()
            .unwrap_or_else(|| "다시 받기 요청을 준비하지 못했습니다.".into());
        let removed = RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            state.notice = Some(error.clone());
            state.restart.take()
        });
        drop(removed);
        return Err(error);
    }
    // StartDownload returns void and skips CanDownload. Keep the reservation
    // until an actual callback establishes the new ID. Never reuse a save path.
    host.start_download(Some(&url));
    crate::browser::notify_panel();
    Ok(())
}

wrap_task! {
    struct RestartTimeout { sequence: u64 }
    impl Task {
        fn execute(&self) {
            RUNTIME.with(|slot| {
                let mut state = slot.borrow_mut();
                let Some(pending) = state.restart.as_mut()
                    .filter(|pending| pending.request.sequence == self.sequence) else { return };
                pending.canceled = true;
                state.notice = Some("다시 받기 요청이 응답하지 않습니다. 원본 탭을 닫으면 요청이 정리됩니다.".into());
            });
            crate::browser::notify_panel();
        }
    }
}

pub(crate) fn before_download(
    tab: i32,
    item: &DownloadItem,
    suggested_name: Option<&CefString>,
    callback: &BeforeDownloadCallback,
) -> bool {
    if item.is_valid() == 0 {
        return false;
    }
    let id = item.id();
    let (status, retry) = RUNTIME.with(|slot| {
        let state = slot.borrow();
        (
            state.model.entry(id).map(|entry| entry.status),
            state.retry(id, tab),
        )
    });
    eprintln!("download event=before id={id} tab={tab} status={status:?}");
    match retry {
        Ok(Some(retry)) => {
            let result = crate::browser::validate_download(tab, retry.scope.as_ref())
                .and_then(|()| continue_download(callback, &retry.path));
            if let Err(error) = result {
                let _ = cancel(id);
                notice(&error);
                return false;
            }
            return true;
        }
        Err(error) => {
            notice(error);
            return false;
        }
        Ok(None) => {}
    }
    let context = match crate::browser::download_context(tab) {
        Ok(context) => context,
        Err(error) => {
            notice(&error);
            return false;
        }
    };
    let original_url = request_url(Some(&CefString::from(&item.original_url())));
    let restarted = RUNTIME.with(|slot| {
        let state = slot.borrow();
        state
            .restart
            .as_ref()
            .filter(|pending| {
                pending.request.target.tab == tab
                    && (pending.cef_id == Some(id)
                        || original_url.as_ref() == Some(&pending.request.url))
            })
            .map(|pending| {
                (
                    pending.request.clone(),
                    pending.canceled,
                    pending.cef_id.is_none_or(|current| current == id),
                )
            })
    });
    if let Some((request, canceled, same_id)) = &restarted {
        if *canceled
            || !same_id
            || original_url.as_ref() != Some(&request.url)
            || crate::browser::validate_download_restart_source(&request.target).is_err()
            || crate::browser::validate_download(tab, request.scope.as_ref()).is_err()
        {
            // Destroy an unexecuted callback outside the runtime borrow. The
            // pending slot remains a tombstone until a terminal update/close.
            RUNTIME.with(|slot| {
                if let Some(pending) = slot.borrow_mut().restart.as_mut() {
                    pending.canceled = true;
                    pending.cef_id.get_or_insert(id);
                }
            });
            notice("다시 받기 요청의 원본 탭이나 제어권이 변경되었습니다.");
            return true;
        }
    }
    let name = suggested_name
        .map(|name| bounded_text(name, 256))
        .unwrap_or_default();
    // Retain/release CEF references outside RefCell borrows, including rejection.
    let mut before = Some(callback.clone());
    let result = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".to_owned());
        }
        if (restarted.is_none() && !state.has_capacity()) || state.callbacks.len() >= MAX_ACTIVE {
            return Err("다운로드는 대기와 진행을 합쳐 최대 4개까지 받을 수 있습니다.".to_owned());
        }
        let replayable = if let Some((request, _, _)) = &restarted {
            state
                .restart
                .as_ref()
                .is_some_and(|pending| pending.request == *request && !pending.canceled)
        } else {
            original_url
                .as_deref()
                .is_some_and(|url| state.consume_request(tab, url, &context.source_target))
        };
        if restarted.is_some() && !replayable {
            return Err("다시 받기 요청이 만료되었습니다.".to_owned());
        }
        state.model.insert(
            id,
            tab,
            context.scope,
            &name,
            &context.source,
            Some(context.source_target),
        )?;
        if let Some((request, _, _)) = &restarted {
            state.model.consume_restart_url(request.source_id);
        }
        if replayable {
            state
                .model
                .set_restart_url(id, original_url.clone().unwrap());
        }
        state.callbacks.insert(
            id,
            Callbacks {
                before: before.take(),
                ..Default::default()
            },
        );
        state.notice = None;
        Ok(if restarted.is_some() {
            state.restart.take()
        } else {
            None
        })
    });
    drop(before);
    match result {
        Ok(previous) => drop(previous),
        Err(error) => {
            notice(&error);
            return false;
        }
    }
    crate::browser::notify_panel();
    true
}

fn update_restart(item: &DownloadItem, callback: Option<&DownloadItemCallback>) -> bool {
    let id = item.id();
    // IDs are unique, not ordered. A URL-only OnUpdated before OnBeforeDownload
    // can belong to an evicted old record, so it cannot establish a new ID.
    let canceled = RUNTIME.with(|slot| {
        slot.borrow()
            .restart
            .as_ref()
            .is_some_and(|pending| pending.canceled_update(id))
    });
    if !canceled {
        return false;
    }
    let terminal = item.is_complete() != 0 || item.is_canceled() != 0 || item.is_interrupted() != 0;
    if terminal {
        RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            if state
                .restart
                .as_ref()
                .is_some_and(|pending| pending.cef_id == Some(id))
            {
                state.restart = None;
            }
        });
    } else if let Some(callback) = callback {
        callback.cancel();
    }
    crate::browser::notify_panel();
    true
}

pub(crate) fn updated(
    tab: Option<i32>,
    item: &DownloadItem,
    callback: Option<&DownloadItemCallback>,
) {
    if item.is_valid() == 0 {
        return;
    }
    if update_restart(item, callback) {
        return;
    }
    let id = item.id();
    let entry = RUNTIME.with(|slot| slot.borrow().model.entry(id).cloned());
    let Some(entry) =
        entry.filter(|entry| tab.is_none_or(|tab| entry.tab == tab) && !entry.status.is_terminal())
    else {
        // Updates may precede OnBeforeDownload or follow cancellation/eviction.
        // Only OnBeforeDownload can create a row.
        return;
    };
    if entry.status != Status::Canceling
        && let Err(error) = crate::browser::validate_download(entry.tab, entry.scope.as_ref())
    {
        let _ = cancel(id);
        notice(&error);
        // A running transfer must still accept its terminal CEF result after
        // ownership was revoked, including a completion racing with cancel.
    }
    let status = if item.is_complete() != 0 {
        Status::Complete
    } else if item.is_canceled() != 0 {
        Status::Canceled
    } else if item.is_interrupted() != 0 {
        Status::Interrupted
    } else {
        Status::Downloading
    };
    let path = bounded_path(&CefString::from(&item.full_path()))
        .ok()
        .filter(|path| !path.is_empty());
    let progress = Progress {
        path,
        status,
        received: item.received_bytes(),
        total: item.total_bytes(),
        percent: item.percent_complete(),
        reason: (status == Status::Interrupted)
            .then(|| format!("중단 코드 {}", item.interrupt_reason().get_raw() as i32)),
    };
    let mut incoming = callback.cloned();
    let (changed, removed, replaced, cancel_now) = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if !state.update(id, progress) {
            return (false, None, None, None);
        }
        if status.is_terminal() {
            (true, state.callbacks.remove(&id), None, None)
        } else {
            let mut cancel_now = None;
            let replaced = state.callbacks.get_mut(&id).and_then(|handles| {
                if handles.cancel_requested {
                    if !handles.cancel_sent && incoming.is_some() {
                        handles.cancel_sent = true;
                        cancel_now = incoming.take();
                    }
                    None
                } else {
                    incoming
                        .take()
                        .and_then(|callback| handles.item.replace(callback))
                }
            });
            (true, None, replaced, cancel_now)
        }
    });
    drop((removed, replaced, incoming));
    if let Some(callback) = cancel_now {
        callback.cancel();
    }
    if changed {
        crate::browser::notify_panel();
    }
}

pub fn choose_path(id: u32) -> Result<(), String> {
    let entry = RUNTIME.with(|slot| {
        let state = slot.borrow();
        if state.shutting_down || state.dialog.is_some() {
            return Err("저장 위치 선택 창을 먼저 닫아 주세요.".to_owned());
        }
        state
            .model
            .entry(id)
            .filter(|entry| entry.status == Status::Pending)
            .cloned()
            .ok_or_else(|| "저장 위치를 선택할 다운로드가 없습니다.".to_owned())
    })?;
    // The host is held only for this call; a row never retains a Browser/host.
    let host = match crate::browser::download_host(entry.tab, entry.scope.as_ref()) {
        Ok(host) => host,
        Err(error) => {
            let _ = cancel(id);
            return Err(error);
        }
    };
    // CEF's C bridge reads this list even when there are no filters.
    let mut filters = CefStringList::new();
    let filters_ptr: *const cef::sys::_cef_string_list_t = (&filters).into();
    if filters_ptr.is_null() {
        return Err("저장 위치 선택 창을 준비하지 못했습니다.".to_owned());
    }
    let dialog = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.dialog.is_some() || state.shutting_down {
            return Err("저장 위치 선택 창을 먼저 닫아 주세요.".to_owned());
        }
        if !state
            .callbacks
            .get(&id)
            .is_some_and(|handles| handles.before.is_some())
        {
            return Err("다운로드의 저장 요청이 만료되었습니다.".to_owned());
        }
        let sequence = state
            .next_dialog
            .checked_add(1)
            .ok_or_else(|| "저장 위치 선택 요청을 더 만들 수 없습니다.".to_owned())?;
        if !state.model.choose(id) {
            return Err("다운로드 상태가 변경되었습니다.".to_owned());
        }
        let dialog = Dialog { id, sequence };
        state.next_dialog = sequence;
        state.dialog = Some(dialog);
        state.notice = None;
        Ok(dialog)
    })?;
    let filename = Path::new(&entry.name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("download");
    let mut callback = SaveDialog::new(dialog);
    host.run_file_dialog(
        FileDialogMode::SAVE,
        Some(&CefString::from("다운로드 저장 위치")),
        Some(&CefString::from(filename)),
        Some(&mut filters),
        Some(&mut callback),
    );
    crate::browser::notify_panel();
    Ok(())
}

wrap_run_file_dialog_callback! {
    struct SaveDialog { dialog: Dialog }
    impl RunFileDialogCallback {
        fn on_file_dialog_dismissed(&self, file_paths: Option<&mut CefStringList>) {
            let path = file_paths.and_then(|paths| {
                if string_list_size(Some(&mut *paths)) != 1 { return None; }
                // Default is a null borrowed string, not writable CEF storage.
                let mut value = CefString::from("");
                let value_ptr: *mut cef::sys::cef_string_t = (&mut value).into();
                if value_ptr.is_null() {
                    return Some(Err("선택한 저장 경로를 읽을 준비를 하지 못했습니다.".to_owned()));
                }
                if string_list_value(Some(&mut *paths), 0, Some(&mut value)) == 0 { return None; }
                Some(bounded_path(&value))
            });
            dialog_dismissed(self.dialog, path);
        }
    }
}

fn dialog_dismissed(dialog: Dialog, path: Option<Result<String, String>>) {
    let entry = RUNTIME.with(|slot| slot.borrow_mut().dismiss(dialog));
    let Some(entry) = entry else {
        crate::browser::notify_panel();
        return;
    };
    let selected = path.transpose().and_then(|path| {
        crate::browser::validate_download(entry.tab, entry.scope.as_ref())?;
        Ok(path)
    });
    let path = match selected {
        Ok(Some(path)) => path,
        Ok(None) => {
            let _ = cancel(dialog.id);
            return;
        }
        Err(error) => {
            let _ = cancel(dialog.id);
            notice(&error);
            return;
        }
    };
    let result = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.shutting_down
            || !state.model.entry(dialog.id).is_some_and(|current| {
                current.status == Status::Choosing
                    && current.tab == entry.tab
                    && current.scope == entry.scope
            })
        {
            return Err("다운로드 요청이 만료되었습니다.".to_owned());
        }
        if !state
            .callbacks
            .get(&dialog.id)
            .is_some_and(|handles| handles.before.is_some())
        {
            return Err("다운로드의 저장 요청이 만료되었습니다.".to_owned());
        }
        state.model.start(dialog.id, &path)?;
        let handles = state.callbacks.get_mut(&dialog.id).unwrap();
        handles.approved_path = Some(path.clone());
        Ok(handles.before.take())
    });
    match result {
        Ok(Some(callback)) => {
            if let Err(error) = continue_download(&callback, &path) {
                let _ = cancel(dialog.id);
                notice(&error);
            }
        }
        Ok(None) => {
            let _ = cancel(dialog.id);
        }
        Err(error) => {
            let _ = cancel(dialog.id);
            notice(&error);
        }
    }
    crate::browser::notify_panel();
}

fn continue_download(callback: &BeforeDownloadCallback, path: &str) -> Result<(), String> {
    let path = CefString::from(path);
    let path_ptr: *const cef::sys::cef_string_t = (&path).into();
    if path_ptr.is_null() {
        return Err("선택한 저장 경로를 전달하지 못했습니다.".to_owned());
    }
    callback.cont(Some(&path), 0);
    Ok(())
}

pub fn cancel(id: u32) -> Result<(), String> {
    let result = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if !state.model.cancel(id) {
            return Err("취소할 다운로드가 없습니다.".to_owned());
        }
        Ok(take_for_cancel(&mut state, id))
    })?;
    if let Some(callbacks) = result {
        callbacks.cancel();
    }
    crate::browser::notify_panel();
    Ok(())
}

pub(crate) fn cancel_task(task: &str, only_tab: Option<i32>) {
    let removed = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let ids = state.model.cancel_task(task, only_tab);
        let removed = ids
            .into_iter()
            .filter_map(|id| take_for_cancel(&mut state, id))
            .collect::<Vec<_>>();
        if let Some(pending) = state.restart.as_mut().filter(|pending| {
            pending
                .request
                .target
                .owner
                .as_ref()
                .is_some_and(|(scope, _)| scope.task == task)
                && only_tab.is_none_or(|tab| pending.request.target.tab == tab)
        }) {
            pending.canceled = true;
        }
        removed
    });
    for callbacks in removed {
        callbacks.cancel();
    }
    crate::browser::notify_panel();
}

pub(crate) fn cancel_tab(tab: i32) {
    let (removed, restart) = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.requests.retain(|request| request.target.tab != tab);
        let restart = if state
            .restart
            .as_ref()
            .is_some_and(|pending| pending.request.target.tab == tab)
        {
            state.restart.take()
        } else {
            None
        };
        let ids = state.model.cancel_tab(tab);
        let removed = ids
            .into_iter()
            .filter_map(|id| take_for_cancel(&mut state, id))
            .collect::<Vec<_>>();
        (removed, restart)
    });
    drop(restart);
    for callbacks in removed {
        callbacks.cancel();
    }
    crate::browser::notify_panel();
}

fn take_for_cancel(state: &mut Runtime, id: u32) -> Option<Callbacks> {
    if state
        .model
        .entry(id)
        .is_some_and(|entry| entry.status == Status::Canceling)
    {
        let handles = state.callbacks.get_mut(&id)?;
        // Keep the transfer's slot until CEF confirms a terminal result. If its
        // first item callback has not arrived, updated() sends Cancel then.
        handles.cancel_requested = true;
        let item = handles.item.take();
        handles.cancel_sent = item.is_some();
        Some(Callbacks {
            before: handles.before.take(),
            item,
            approved_path: None,
            ..Default::default()
        })
    } else {
        state.callbacks.remove(&id)
    }
}

pub fn clear_finished() {
    RUNTIME.with(|slot| {
        slot.borrow_mut().model.clear_finished();
    });
    crate::browser::notify_panel();
}

pub(crate) fn clear() {
    let previous = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let next_dialog = state.next_dialog;
        let next_restart = state.next_restart;
        std::mem::replace(
            &mut *state,
            Runtime {
                next_dialog,
                next_restart,
                shutting_down: true,
                ..Default::default()
            },
        )
    });
    for callbacks in previous.callbacks.into_values() {
        callbacks.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restart_target() -> SourceTarget {
        SourceTarget {
            tab: 7,
            generation: 2,
            document_epoch: 3,
            owner: None,
        }
    }

    #[test]
    fn manual_restart_requires_unique_get_and_exact_document() {
        let target = restart_target();
        let url = "https://example.test/file";
        let attempt = |get| RequestAttempt {
            target: target.clone(),
            url: url.into(),
            get,
        };
        let mut runtime = Runtime::default();
        runtime.requests.push(attempt(true));
        assert!(runtime.consume_request(7, url, &target));
        assert!(!runtime.consume_request(7, url, &target));
        runtime.requests.extend([attempt(true), attempt(false)]);
        assert!(!runtime.consume_request(7, url, &target));
        assert!(!runtime.consume_request(7, url, &target));
        runtime.requests.extend([attempt(true), attempt(true)]);
        assert!(!runtime.consume_request(7, url, &target));
        assert!(!runtime.consume_request(7, url, &target));
        runtime.requests.push(attempt(true));
        assert!(!runtime.consume_request(
            7,
            url,
            &SourceTarget {
                document_epoch: 4,
                ..target.clone()
            }
        ));
        runtime.requests.push(attempt(true));
        assert!(!runtime.consume_request(8, url, &target));
        assert_eq!(runtime.requests.len(), 1);
    }

    #[test]
    fn manual_restart_reservation_prevents_duplicates_and_counts_toward_capacity() {
        let mut runtime = Runtime::default();
        runtime
            .model
            .insert(1, 7, None, "file", "내 공간", Some(restart_target()))
            .unwrap();
        runtime
            .model
            .set_restart_url(1, "https://example.test/file".into());
        assert!(runtime.restart_request(1).is_err());
        runtime.model.cancel(1);
        let request = runtime.restart_request(1).unwrap();
        assert!(request.scope.is_none());
        runtime.restart = Some(PendingRestart {
            request,
            cef_id: None,
            canceled: false,
        });
        for id in [1, 2, 999] {
            assert!(!runtime.restart.as_ref().unwrap().canceled_update(id));
        }
        assert!(runtime.restart_request(1).is_err());
        for id in 2..=MAX_ACTIVE as u32 {
            runtime
                .model
                .insert(id, 8, None, "active", "내 공간", None)
                .unwrap();
        }
        assert!(!runtime.has_capacity());
        // Timeout is not proof that StartDownload did nothing.
        runtime.restart.as_mut().unwrap().canceled = true;
        // Even after a timeout, late OnUpdated callbacks from erased records
        // cannot identify the new request before OnBeforeDownload.
        for id in [1, 2, 999] {
            assert!(!runtime.restart.as_ref().unwrap().canceled_update(id));
        }
        runtime.restart.as_mut().unwrap().cef_id = Some(999);
        assert!(runtime.restart.as_ref().unwrap().canceled_update(999));
        assert!(!runtime.restart.as_ref().unwrap().canceled_update(1));
        assert!(!runtime.has_capacity());
        assert!(runtime.restart_request(1).is_err());
        runtime.restart.take();
        assert!(runtime.has_capacity());
        runtime.model.cancel(2);
        runtime.requests.push(RequestAttempt {
            target: restart_target(),
            url: "https://example.test/other".into(),
            get: true,
        });
        assert!(runtime.restart_request(1).is_err());
        runtime.requests.clear();
        assert!(runtime.restart_request(1).is_ok());
        runtime.dialog = Some(Dialog { id: 3, sequence: 1 });
        assert!(runtime.restart_request(1).is_err());
        runtime.dialog = None;
        runtime.shutting_down = true;
        assert!(runtime.restart_request(1).is_err());
    }

    #[test]
    fn source_lookup_preserves_download_and_rejects_sheet_shutdown_and_removed_record() {
        let mut runtime = Runtime::default();
        let target = SourceTarget {
            tab: 7,
            generation: 2,
            document_epoch: 3,
            owner: None,
        };
        runtime
            .model
            .insert(
                1,
                7,
                None,
                "test.txt",
                "내 공간, 탭 7",
                Some(target.clone()),
            )
            .unwrap();
        let before = runtime.model.snapshot();
        assert_eq!(runtime.source_target(1), Ok(target.clone()));
        assert_eq!(runtime.model.snapshot(), before);
        runtime.dialog = Some(Dialog { id: 1, sequence: 4 });
        assert!(runtime.source_target(1).is_err());
        runtime.dialog = None;
        runtime.shutting_down = true;
        assert!(runtime.source_target(1).is_err());
        runtime.shutting_down = false;
        assert!(runtime.source_target(999).is_err());
        runtime.model.cancel(1);
        assert_eq!(runtime.source_target(1), Ok(target));
        runtime.model.clear_finished();
        assert!(runtime.source_target(1).is_err());
    }

    fn approved(runtime: &mut Runtime, id: u32, scope: Option<crate::protocol::TaskScope>) {
        runtime
            .model
            .insert(id, 7, scope, "test.txt", "사용자", None)
            .unwrap();
        assert!(runtime.model.choose(id));
        runtime.model.start(id, "/tmp/approved.txt").unwrap();
        runtime.callbacks.insert(
            id,
            Callbacks {
                approved_path: Some("/tmp/approved.txt".into()),
                ..Default::default()
            },
        );
    }

    #[test]
    fn completed_file_lookup_survives_callback_release_and_preserves_model() {
        let mut runtime = Runtime::default();
        approved(&mut runtime, 1, None);
        assert!(runtime.completed_file_path(1).is_err());
        assert!(runtime.update(
            1,
            Progress {
                path: Some("/tmp/approved.txt".into()),
                status: Status::Complete,
                ..Default::default()
            }
        ));
        runtime.callbacks.remove(&1);
        let snapshot = runtime.model.snapshot();
        assert_eq!(runtime.completed_file_path(1).unwrap(), "/tmp/approved.txt");
        assert_eq!(runtime.model.snapshot(), snapshot);
        assert!(runtime.completed_file_path(99).is_err());
        runtime.dialog = Some(Dialog { id: 2, sequence: 1 });
        assert!(runtime.completed_file_path(1).is_err());
        runtime.dialog = None;
        runtime.shutting_down = true;
        assert!(runtime.completed_file_path(1).is_err());
        runtime.shutting_down = false;
        runtime.model.clear_finished();
        assert!(runtime.completed_file_path(1).is_err());
    }

    #[test]
    fn completed_file_lookup_never_uses_unapproved_or_stale_progress_paths() {
        for (fresh, keep_approval) in [
            (None, true),
            (Some("/tmp/different.txt"), true),
            (Some("/tmp/approved.txt"), false),
        ] {
            let mut runtime = Runtime::default();
            approved(&mut runtime, 1, None);
            runtime.update(
                1,
                Progress {
                    path: Some("/tmp/approved.txt".into()),
                    ..Default::default()
                },
            );
            if !keep_approval {
                runtime.callbacks.remove(&1);
            }
            runtime.update(
                1,
                Progress {
                    path: fresh.map(str::to_owned),
                    status: Status::Complete,
                    ..Default::default()
                },
            );
            assert!(runtime.completed_file_path(1).is_err());
        }
    }

    #[test]
    fn same_download_retry_reuses_approved_path_and_scope_at_capacity() {
        let mut runtime = Runtime::default();
        let scope = crate::protocol::TaskScope {
            task: "task-download".into(),
            revision: 3,
        };
        approved(&mut runtime, 1, Some(scope.clone()));
        for id in 2..=MAX_ACTIVE as u32 {
            approved(&mut runtime, id, None);
        }
        // A progress path must never become permission to write a retry there.
        runtime.model.update(
            1,
            Progress {
                path: Some("/tmp/engine-progress-path.tmp".into()),
                ..Default::default()
            },
        );
        assert!(!runtime.model.can_accept());
        assert_eq!(
            runtime.retry(1, 7),
            Ok(Some(Retry {
                scope: Some(scope),
                path: "/tmp/approved.txt".into(),
            }))
        );
        assert_eq!(runtime.model.activity(), (MAX_ACTIVE, 0));
        assert_eq!(runtime.callbacks.len(), MAX_ACTIVE);
        assert_eq!(runtime.model.snapshot().rows.len(), MAX_ACTIVE);
    }

    #[test]
    fn retry_never_reopens_canceled_terminal_unapproved_or_other_tab_downloads() {
        let mut runtime = Runtime::default();
        approved(&mut runtime, 1, None);
        assert!(runtime.retry(1, 8).is_err());
        runtime.callbacks.get_mut(&1).unwrap().cancel_requested = true;
        assert!(runtime.retry(1, 7).is_err());
        runtime.callbacks.get_mut(&1).unwrap().cancel_requested = false;
        assert!(runtime.model.cancel(1));
        assert!(runtime.retry(1, 7).is_err());
        runtime.model.update(
            1,
            Progress {
                status: Status::Canceled,
                ..Default::default()
            },
        );
        runtime.callbacks.remove(&1);
        assert!(runtime.retry(1, 7).is_err());

        runtime
            .model
            .insert(2, 7, None, "pending.txt", "사용자", None)
            .unwrap();
        runtime.callbacks.insert(2, Callbacks::default());
        assert!(runtime.retry(2, 7).is_err());
        assert!(runtime.model.choose(2));
        assert!(runtime.retry(2, 7).is_err());
        runtime.model.start(2, "/tmp/unapproved.txt").unwrap();
        assert!(runtime.retry(2, 7).is_err());
        assert_eq!(runtime.retry(3, 7), Ok(None));
        runtime.shutting_down = true;
        assert!(runtime.retry(3, 7).is_err());
    }

    #[test]
    fn canceled_sheet_keeps_busy_until_its_matching_native_dismissal() {
        let mut runtime = Runtime::default();
        runtime
            .model
            .insert(1, 7, None, "test.txt", "사용자", None)
            .unwrap();
        assert!(runtime.model.choose(1));
        let dialog = Dialog { id: 1, sequence: 3 };
        runtime.dialog = Some(dialog);
        assert!(runtime.model.cancel(1));
        assert_eq!(runtime.model.clear_finished(), 1);
        assert!(
            runtime
                .dismiss(Dialog {
                    sequence: 2,
                    ..dialog
                })
                .is_none()
        );
        assert_eq!(runtime.dialog, Some(dialog));
        assert!(runtime.dismiss(dialog).is_none());
        assert_eq!(runtime.dialog, None);
    }

    #[test]
    fn old_sheet_completion_cannot_take_a_new_request_with_the_same_download_id() {
        let mut runtime = Runtime::default();
        runtime
            .model
            .insert(1, 7, None, "test.txt", "사용자", None)
            .unwrap();
        assert!(runtime.model.choose(1));
        let current = Dialog { id: 1, sequence: 4 };
        runtime.dialog = Some(current);
        assert!(
            runtime
                .dismiss(Dialog {
                    sequence: 3,
                    ..current
                })
                .is_none()
        );
        assert_eq!(runtime.dialog, Some(current));
        assert_eq!(runtime.dismiss(current).unwrap().tab, 7);
        assert!(runtime.dismiss(current).is_none());
    }

    #[test]
    fn cancel_before_first_item_callback_keeps_the_transfer_slot() {
        let mut runtime = Runtime::default();
        runtime
            .model
            .insert(1, 7, None, "test.txt", "사용자", None)
            .unwrap();
        runtime.callbacks.insert(1, Callbacks::default());
        assert!(runtime.model.choose(1));
        runtime.model.start(1, "/tmp/test.txt").unwrap();
        assert!(runtime.model.cancel(1));
        let removed = take_for_cancel(&mut runtime, 1).unwrap();
        assert!(removed.item.is_none());
        assert_eq!(runtime.model.activity(), (1, 0));
        assert_eq!(runtime.callbacks.len(), 1);
        assert!(runtime.callbacks[&1].cancel_requested);
        assert!(!runtime.callbacks[&1].cancel_sent);
        assert_eq!(runtime.model.clear_finished(), 0);
    }
}
