//! CEF file chooser callbacks retained on the UI thread until an explicit user decision.

use crate::file_selection::{
    Filters, MAX_FILTER_BYTES, MAX_FILTERS, Model, Request, validate_paths,
};
pub use crate::file_selection::{Mode, Row};
use cef::*;
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Default)]
struct Runtime {
    model: Model,
    callbacks: BTreeMap<u64, FileDialogCallback>,
    notice: Option<String>,
    shutting_down: bool,
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default()); }

pub fn snapshot() -> Vec<Row> {
    RUNTIME.with(|slot| slot.borrow().model.rows())
}

pub fn pending_count() -> usize {
    RUNTIME.with(|slot| slot.borrow().model.len())
}

pub fn is_choosing() -> bool {
    RUNTIME.with(|slot| slot.borrow().model.choosing_token().is_some())
}

pub fn notice() -> Option<String> {
    RUNTIME.with(|slot| slot.borrow().notice.clone())
}

fn set_notice(value: Option<String>) {
    RUNTIME.with(|slot| slot.borrow_mut().notice = value);
}

fn context(request: &Request) -> crate::browser::FileSelectionContext {
    crate::browser::FileSelectionContext {
        target: request.target.clone(),
        scope: request.scope.clone(),
        generation: request.generation,
        source: request.row.source.clone(),
    }
}

fn bounded_filter(value: &CefString) -> Result<String, String> {
    let units = value.as_slice().unwrap_or(&[]);
    if units.len() > MAX_FILTER_BYTES {
        return Err("파일 형식 요청이 너무 길어 선택 창을 열 수 없습니다.".into());
    }
    let mut result = String::new();
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.map_err(|_| "요청한 파일 형식을 읽을 수 없습니다.".to_owned())?;
        if result.len() + character.len_utf8() > MAX_FILTER_BYTES {
            return Err("파일 형식 요청이 너무 길어 선택 창을 열 수 없습니다.".into());
        }
        result.push(character);
    }
    Ok(result)
}

fn filter_values(values: Option<&mut CefStringList>) -> Result<Vec<String>, String> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    let pointer: *const cef::sys::_cef_string_list_t = (&*values).into();
    if pointer.is_null() {
        return Err("요청한 파일 형식을 읽을 수 없습니다.".into());
    }
    let count = string_list_size(Some(&mut *values));
    if count > MAX_FILTERS {
        return Err("요청한 파일 형식이 너무 많아 선택 창을 열 수 없습니다.".into());
    }
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let mut value = CefString::from("");
        let pointer: *mut cef::sys::cef_string_t = (&mut value).into();
        if pointer.is_null() || string_list_value(Some(&mut *values), index, Some(&mut value)) == 0
        {
            return Err("요청한 파일 형식을 읽을 수 없습니다.".into());
        }
        result.push(bounded_filter(&value)?);
    }
    Ok(result)
}

pub(crate) fn show(
    tab: i32,
    mode: FileDialogMode,
    accept_filters: Option<&mut CefStringList>,
    accept_extensions: Option<&mut CefStringList>,
    callback: &FileDialogCallback,
) -> bool {
    let mode = if mode == FileDialogMode::OPEN {
        Mode::Open
    } else if mode == FileDialogMode::OPEN_MULTIPLE {
        Mode::Multiple
    } else if mode == FileDialogMode::OPEN_FOLDER {
        Mode::Folder
    } else {
        return false;
    };
    let context = match crate::browser::file_selection_context(tab) {
        Ok(Some(context)) => context,
        Ok(None) => return false,
        Err(error) => {
            callback.cancel();
            set_notice(Some(error));
            crate::browser::notify_panel();
            return true;
        }
    };
    let filters = filter_values(accept_filters).and_then(|filters| {
        let expanded = filter_values(accept_extensions)?;
        Filters::new(&filters, &expanded).map_err(str::to_owned)
    });
    let filters = match filters {
        Ok(filters) => filters,
        Err(error) => {
            callback.cancel();
            set_notice(Some(error));
            crate::browser::notify_panel();
            return true;
        }
    };
    // Retaining a CEF callback can reenter native code. Validate after retaining,
    // and never retain or release a callback while the runtime is borrowed.
    let mut incoming = Some(callback.clone());
    let result = crate::browser::validate_file_selection(&context).and_then(|()| {
        RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            if state.shutting_down {
                return Err("브라우저를 종료하고 있습니다.".to_owned());
            }
            let token = state
                .model
                .insert(
                    context.target,
                    context.scope,
                    context.generation,
                    &context.source,
                    mode,
                    filters,
                )
                .map_err(str::to_owned)?;
            // Model tokens never repeat, including after clearing the queue.
            debug_assert!(!state.callbacks.contains_key(&token));
            state.callbacks.insert(token, incoming.take().unwrap());
            state.notice = None;
            Ok(())
        })
    });
    if let Some(callback) = incoming {
        callback.cancel();
    }
    if let Err(error) = result {
        set_notice(Some(error));
    }
    crate::browser::notify_panel();
    true
}

pub fn choose(token: u64) -> Result<(), String> {
    let request = RUNTIME.with(|slot| {
        let state = slot.borrow();
        if state.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".to_owned());
        }
        state
            .model
            .get(token)
            .filter(|_| state.callbacks.contains_key(&token))
            .cloned()
            .ok_or_else(|| "파일 선택 요청이 만료되었습니다.".to_owned())
    })?;
    if let Err(error) = crate::browser::validate_file_selection(&context(&request)) {
        let _ = cancel(token);
        return Err(error);
    }
    if !crate::download_runtime::source_reveal_allowed() {
        return Err("열려 있는 저장 위치 선택 창을 먼저 닫아 주세요.".into());
    }
    let request = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.shutting_down || !state.callbacks.contains_key(&token) {
            return Err("파일 선택 요청이 만료되었습니다.".to_owned());
        }
        let request = state.model.begin(token).map_err(str::to_owned)?;
        state.notice = None;
        Ok(request)
    })?;
    // AppKit may synchronously dismiss or revoke the request during show().
    // Its completion is the only path that releases the native slot.
    let result = crate::file_selection_native::show(token, request.row.mode, &request.extensions);
    if let Err(error) = &result {
        native_finished(token, Err(error.clone()));
    }
    crate::browser::notify_panel();
    result
}

fn remove_matching(
    matches: impl Fn(&Request) -> bool,
) -> (Vec<FileDialogCallback>, Option<u64>, bool) {
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let requests = state.model.remove_matching(matches);
        let changed = !requests.is_empty();
        let dialog = state
            .model
            .choosing_token()
            .filter(|token| requests.iter().any(|request| request.row.token == *token));
        let callbacks = requests
            .into_iter()
            .filter_map(|request| state.callbacks.remove(&request.row.token))
            .collect();
        (callbacks, dialog, changed)
    })
}

fn cancel_removed(callbacks: Vec<FileDialogCallback>, dialog: Option<u64>) {
    if let Some(token) = dialog {
        crate::file_selection_native::cancel(token);
    }
    for callback in callbacks {
        callback.cancel();
    }
    crate::browser::notify_panel();
}

pub fn cancel(token: u64) -> Result<(), String> {
    let (callbacks, dialog, changed) = remove_matching(|request| request.row.token == token);
    if !changed {
        return Err("취소할 파일 선택 요청이 없습니다.".into());
    }
    cancel_removed(callbacks, dialog);
    Ok(())
}

pub fn cancel_tab(tab: i32) {
    let (callbacks, dialog, changed) = remove_matching(|request| request.target.tab == tab);
    if changed {
        cancel_removed(callbacks, dialog);
    }
}

pub fn cancel_task(task: &str, only_tab: Option<i32>) {
    let (callbacks, dialog, changed) = remove_matching(|request| {
        request.scope.task == task && only_tab.is_none_or(|tab| request.target.tab == tab)
    });
    if changed {
        cancel_removed(callbacks, dialog);
    }
}

pub fn clear() {
    let (callbacks, dialog) = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.shutting_down = true;
        state.notice = None;
        state.model.remove_matching(|_| true);
        // Keep both the token counter and any canceled native slot until its
        // real completion. A late response can never fulfill a future request.
        let dialog = state.model.choosing_token();
        (
            std::mem::take(&mut state.callbacks).into_values().collect(),
            dialog,
        )
    });
    cancel_removed(callbacks, dialog);
}

fn path_list(paths: &[String]) -> Result<CefStringList, String> {
    let mut list = CefStringList::new();
    let pointer: *const cef::sys::_cef_string_list_t = (&list).into();
    if pointer.is_null() {
        return Err("선택한 파일을 페이지에 전달할 준비를 하지 못했습니다.".into());
    }
    for path in paths {
        let value = CefString::from(path.as_str());
        let pointer: *const cef::sys::cef_string_t = (&value).into();
        if pointer.is_null() {
            return Err("선택한 파일을 페이지에 전달할 준비를 하지 못했습니다.".into());
        }
        string_list_append(Some(&mut list), Some(&value));
    }
    if string_list_size(Some(&mut list)) != paths.len() {
        return Err("선택한 파일 목록을 페이지에 전달할 준비를 하지 못했습니다.".into());
    }
    Ok(list)
}

fn consume_completion(token: u64) -> bool {
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.model.remove(token).is_some() && !state.shutting_down
    })
}

pub fn native_finished(token: u64, result: Result<Vec<String>, String>) {
    let finished = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let request = state.model.dismiss(token)?;
        let callback = state.callbacks.remove(&token);
        Some((request, callback, state.shutting_down))
    });
    let Some((request, Some(callback), shutting_down)) = finished else {
        crate::browser::notify_panel();
        return;
    };
    let selected = result.and_then(|paths| {
        if paths.is_empty() {
            return Ok(None);
        }
        validate_paths(request.row.mode, &paths).map_err(str::to_owned)?;
        if shutting_down {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        crate::browser::validate_file_selection(&context(&request))?;
        let list = path_list(&paths)?;
        // Native string operations may reenter. Check again immediately before
        // Continue, after removing the callback from all shared state.
        if RUNTIME.with(|slot| slot.borrow().shutting_down) {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        crate::browser::validate_file_selection(&context(&request))?;
        Ok(Some(list))
    });
    // Row removal also observes cancel_tab during native path preparation.
    // A child-frame invalidation can leave the main-document stamp unchanged.
    let selected = if consume_completion(token) {
        selected
    } else {
        Err("파일을 요청한 문서나 작업이 변경되어 선택을 취소했습니다.".to_owned())
    };
    match selected {
        Ok(Some(mut paths)) => callback.cont(Some(&mut paths)),
        Ok(None) => callback.cancel(),
        Err(error) => {
            callback.cancel();
            set_notice(Some(error));
        }
    }
    crate::browser::notify_panel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    struct RecordingCallback {
        canceled: Rc<RefCell<Vec<u64>>>,
        token: u64,
    }

    impl Drop for RecordingCallback {
        fn drop(&mut self) {
            assert!(RUNTIME.with(|slot| slot.try_borrow_mut().is_ok()));
        }
    }

    unsafe extern "C" fn record_cancel(raw: *mut sys::_cef_file_dialog_callback_t) {
        let data = &unsafe {
            cef::rc::RcImpl::<sys::_cef_file_dialog_callback_t, RecordingCallback>::get(raw)
        }
        .interface;
        data.canceled.borrow_mut().push(data.token);
        // CEF may synchronously reenter during cancellation. The callback was
        // already removed, so neither nested cancellation nor a late native
        // response can consume it again.
        assert!(cancel(data.token).is_err());
        native_finished(data.token, Ok(vec!["/tmp/late-selection".into()]));
    }

    fn queue(tab: i32, task: &str, canceled: Rc<RefCell<Vec<u64>>>) -> u64 {
        let token = RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            state.shutting_down = false;
            state
                .model
                .insert(
                    crate::downloads::SourceTarget {
                        tab,
                        generation: 1,
                        document_epoch: 1,
                        owner: None,
                    },
                    crate::protocol::TaskScope {
                        task: task.into(),
                        revision: 1,
                    },
                    1,
                    "AI 작업",
                    Mode::Open,
                    Filters::new(&[], &[]).unwrap(),
                )
                .unwrap()
        });
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_file_dialog_callback_t>() };
        object.cancel = Some(record_cancel);
        let raw = cef::rc::RcImpl::new(object, RecordingCallback { canceled, token });
        let callback = cef::rc::ConvertReturnValue::wrap_result(
            raw.cast::<sys::_cef_file_dialog_callback_t>(),
        );
        RUNTIME.with(|slot| {
            slot.borrow_mut().callbacks.insert(token, callback);
        });
        token
    }

    #[test]
    fn file_selection_callbacks_cancel_once_outside_borrows_and_match_scope() {
        clear();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let first = queue(1, "task-1", calls.clone());
        let second = queue(2, "task-1", calls.clone());
        let third = queue(3, "task-2", calls.clone());
        cancel_task("task-1", Some(1));
        assert_eq!(&*calls.borrow(), &[first]);
        assert_eq!(
            snapshot().iter().map(|row| row.token).collect::<Vec<_>>(),
            [second, third]
        );
        cancel_tab(2);
        assert_eq!(&*calls.borrow(), &[first, second]);
        clear();
        assert_eq!(&*calls.borrow(), &[first, second, third]);
        assert_eq!(pending_count(), 0);
        assert!(!is_choosing());
        clear();
        assert_eq!(calls.borrow().len(), 3);
    }

    #[test]
    fn file_selection_preparing_completion_remains_revocable_and_not_choosable() {
        clear();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let token = queue(1, "task-1", calls.clone());
        let callback = RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            state.model.begin(token).unwrap();
            state.model.dismiss(token).unwrap();
            state.callbacks.remove(&token).unwrap()
        });
        assert!(!is_choosing());
        assert_eq!(pending_count(), 1);
        assert!(choose(token).is_err());
        // Simulate cancellation reentering while the native CEF path list is
        // being prepared after the picker has dismissed.
        cancel_tab(1);
        assert_eq!(pending_count(), 0);
        assert!(!consume_completion(token));
        assert!(calls.borrow().is_empty());
        callback.cancel();
        drop(callback);
        assert_eq!(&*calls.borrow(), &[token]);
        clear();
    }
}
