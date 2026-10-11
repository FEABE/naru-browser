//! CEF download callbacks stay on the UI thread; rows contain copied metadata only.

use crate::downloads::{MAX_ACTIVE, MAX_PATH_BYTES, Model, Progress, Snapshot, Status};
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

#[derive(Default)]
struct Runtime {
    model: Model,
    callbacks: BTreeMap<u32, Callbacks>,
    dialog: Option<Dialog>,
    next_dialog: u64,
    notice: Option<String>,
    shutting_down: bool,
}

impl Runtime {
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
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        let mut snapshot = state.model.snapshot();
        snapshot.notice = state.notice.clone();
        if state.dialog.is_some() {
            for row in &mut snapshot.rows {
                row.can_choose = false;
            }
            if snapshot.notice.is_none() {
                snapshot.notice = Some("저장 위치 선택 창을 먼저 닫아 주세요.".into());
            }
        }
        snapshot
    })
}

pub fn activity() -> (usize, usize) {
    RUNTIME.with(|slot| slot.borrow().model.activity())
}

fn notice(message: &str) {
    RUNTIME.with(|slot| {
        slot.borrow_mut().notice = Some(message.chars().take(512).collect());
    });
    crate::browser::notify_panel();
}

pub(crate) fn can_accept(tab: i32) -> bool {
    let result = crate::browser::download_context(tab).and_then(|_| {
        RUNTIME.with(|slot| {
            let state = slot.borrow();
            if !state.shutting_down
                && state.model.can_accept()
                && state.callbacks.len() < MAX_ACTIVE
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
        if state.callbacks.len() >= MAX_ACTIVE {
            return Err("다운로드는 대기와 진행을 합쳐 최대 4개까지 받을 수 있습니다.".to_owned());
        }
        state
            .model
            .insert(id, tab, context.scope, &name, &context.source)?;
        state.callbacks.insert(
            id,
            Callbacks {
                before: before.take(),
                ..Default::default()
            },
        );
        state.notice = None;
        Ok(())
    });
    drop(before);
    if let Err(error) = result {
        notice(&error);
        return false;
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
        if !state.model.update(id, progress) {
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
        ids.into_iter()
            .filter_map(|id| take_for_cancel(&mut state, id))
            .collect::<Vec<_>>()
    });
    for callbacks in removed {
        callbacks.cancel();
    }
    crate::browser::notify_panel();
}

pub(crate) fn cancel_tab(tab: i32) {
    let removed = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let ids = state.model.cancel_tab(tab);
        ids.into_iter()
            .filter_map(|id| take_for_cancel(&mut state, id))
            .collect::<Vec<_>>()
    });
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
        std::mem::replace(
            &mut *state,
            Runtime {
                next_dialog,
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

    fn approved(runtime: &mut Runtime, id: u32, scope: Option<crate::protocol::TaskScope>) {
        runtime
            .model
            .insert(id, 7, scope, "test.txt", "사용자")
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
            .insert(2, 7, None, "pending.txt", "사용자")
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
            .insert(1, 7, None, "test.txt", "사용자")
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
            .insert(1, 7, None, "test.txt", "사용자")
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
            .insert(1, 7, None, "test.txt", "사용자")
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
