//! CEF permission callbacks live only in this UI-thread queue.

pub use crate::download_permissions::PermissionRow;
use crate::download_permissions::{MAX_ORIGIN_BYTES, Model, Request};
use cef::*;
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Default)]
struct Runtime {
    model: Model,
    callbacks: BTreeMap<u64, PermissionPromptCallback>,
    shutting_down: bool,
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default()); }

pub fn snapshot() -> Vec<PermissionRow> {
    RUNTIME.with(|slot| slot.borrow().model.rows())
}

pub fn pending_count() -> usize {
    RUNTIME.with(|slot| slot.borrow().model.len())
}

fn origin_text(value: &CefString) -> Option<String> {
    let units = value.as_slice()?;
    if units.len() > MAX_ORIGIN_BYTES {
        return None;
    }
    let mut result = String::new();
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.ok()?;
        if result.len() + character.len_utf8() > MAX_ORIGIN_BYTES {
            return None;
        }
        result.push(character);
    }
    Some(result)
}

pub(crate) fn show(
    tab: Option<i32>,
    prompt_id: u64,
    origin: Option<&CefString>,
    permissions: u32,
    callback: Option<&PermissionPromptCallback>,
) -> bool {
    // All other requests, including mixed masks, retain Alloy's default IGNORE.
    if permissions != PermissionRequestTypes::MULTIPLE_DOWNLOADS.get_raw() {
        return false;
    }
    let Some(callback) = callback else {
        return false;
    };
    let context = tab.and_then(|tab| crate::browser::download_permission_context(tab).ok());
    let origin = origin.and_then(origin_text);
    let (Some(context), Some(origin)) = (context, origin) else {
        callback.cont(PermissionRequestResult::IGNORE);
        return true;
    };
    // Both retain and release can execute CEF code, so neither occurs while
    // RUNTIME or browser STATE is borrowed.
    let mut incoming = Some(callback.clone());
    let queued = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.shutting_down {
            return false;
        }
        let Ok(token) = state
            .model
            .insert(context.target, prompt_id, &origin, &context.source)
        else {
            return false;
        };
        state.callbacks.insert(token, incoming.take().unwrap());
        true
    });
    if let Some(callback) = incoming {
        callback.cont(PermissionRequestResult::IGNORE);
    }
    if queued {
        crate::browser::notify_panel();
    }
    true
}

pub fn decide(token: u64, allow: bool) -> Result<(), String> {
    let request = RUNTIME
        .with(|slot| {
            let mut state = slot.borrow_mut();
            let request = state.model.take(token)?;
            Some((request, state.callbacks.remove(&token), state.shutting_down))
        })
        .ok_or_else(|| "다운로드 권한 요청이 만료되었습니다.".to_owned())?;
    let (request, callback, shutting_down) = request;
    let valid = if shutting_down {
        Err("브라우저를 종료하고 있습니다.".to_owned())
    } else {
        crate::browser::validate_download_permission(&request.target)
    };
    // Take before Continue: CEF may dismiss synchronously and re-enter us.
    if let Some(callback) = callback {
        callback.cont(if allow && valid.is_ok() {
            PermissionRequestResult::ACCEPT
        } else {
            // IGNORE cancels this prompt. DENY would persist an origin block.
            PermissionRequestResult::IGNORE
        });
    }
    crate::browser::notify_panel();
    valid
}

fn take_matching(matches: impl Fn(&Request) -> bool) -> Vec<PermissionPromptCallback> {
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state
            .model
            .take_matching(matches)
            .into_iter()
            .filter_map(|request| state.callbacks.remove(&request.row.token))
            .collect()
    })
}

fn ignore_matching(matches: impl Fn(&Request) -> bool) {
    let callbacks = take_matching(matches);
    let changed = !callbacks.is_empty();
    for callback in callbacks {
        callback.cont(PermissionRequestResult::IGNORE);
    }
    if changed {
        crate::browser::notify_panel();
    }
}

pub(crate) fn dismissed(tab: Option<i32>, prompt_id: u64) {
    // CEF has already resolved the request. Release without another Continue.
    let callbacks = take_matching(|request| {
        request.prompt_id == prompt_id && tab.is_none_or(|tab| request.target.tab == tab)
    });
    let changed = !callbacks.is_empty();
    drop(callbacks);
    if changed {
        crate::browser::notify_panel();
    }
}

pub(crate) fn cancel_tab(tab: i32) {
    ignore_matching(|request| request.target.tab == tab);
}

pub(crate) fn cancel_task(task: &str, only_tab: Option<i32>) {
    ignore_matching(|request| {
        only_tab.is_none_or(|tab| request.target.tab == tab)
            && request
                .target
                .owner
                .as_ref()
                .is_some_and(|(owner, _)| owner.task == task)
    });
}

pub(crate) fn clear() {
    RUNTIME.with(|slot| slot.borrow_mut().shutting_down = true);
    // Keep the monotonically increasing model token even across shutdown.
    ignore_matching(|_| true);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingCallback {
        calls: std::rc::Rc<RefCell<Vec<u32>>>,
        tab: i32,
        prompt: u64,
    }

    unsafe extern "C" fn record_result(
        raw: *mut sys::_cef_permission_prompt_callback_t,
        result: sys::cef_permission_request_result_t,
    ) {
        let data =
            &cef::rc::RcImpl::<sys::_cef_permission_prompt_callback_t, RecordingCallback>::get(raw)
                .interface;
        data.calls.borrow_mut().push(result as u32);
        // Match CEF's synchronous dismissal before Chromium continues.
        dismissed(Some(data.tab), data.prompt);
    }

    fn queue(calls: &std::rc::Rc<RefCell<Vec<u32>>>) -> u64 {
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_permission_prompt_callback_t>() };
        object.cont = Some(record_result);
        let raw = cef::rc::RcImpl::new(
            object,
            RecordingCallback {
                calls: calls.clone(),
                tab: 7,
                prompt: 3,
            },
        );
        let callback = cef::rc::ConvertReturnValue::wrap_result(
            raw.cast::<sys::_cef_permission_prompt_callback_t>(),
        );
        RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            state.shutting_down = false;
            let token = state
                .model
                .insert(
                    crate::download_permissions::Target {
                        tab: 7,
                        epoch: 1,
                        owner: None,
                    },
                    3,
                    "https://example.test",
                    "사용자",
                )
                .unwrap();
            state.callbacks.insert(token, callback);
            token
        })
    }

    #[test]
    fn invalid_masks_never_handle_or_approve_another_permission() {
        let downloads = PermissionRequestTypes::MULTIPLE_DOWNLOADS.get_raw();
        for permissions in [0, 1, downloads | 1, u32::MAX] {
            assert!(!show(None, 1, None, permissions, None));
        }
        assert!(!show(None, 1, None, downloads, None));
    }

    #[test]
    fn cancel_removes_before_reentrant_dismissal_and_stale_tokens_cannot_approve() {
        clear();
        let calls = std::rc::Rc::new(RefCell::new(Vec::new()));
        let first = queue(&calls);
        cancel_tab(7);
        assert_eq!(*calls.borrow(), [PermissionRequestResult::IGNORE.get_raw()]);
        dismissed(Some(7), 3);
        let second = queue(&calls);
        assert!(second > first);
        assert!(decide(first, true).is_err());
        assert_eq!(pending_count(), 1);
        assert_eq!(calls.borrow().len(), 1);
        // A closed/nonexistent source tab can never receive ACCEPT.
        assert!(decide(second, true).is_err());
        assert_eq!(
            *calls.borrow(),
            [PermissionRequestResult::IGNORE.get_raw(); 2]
        );
        clear();
    }

    #[test]
    fn engine_dismissal_only_releases_and_shutdown_ignores_once() {
        clear();
        let calls = std::rc::Rc::new(RefCell::new(Vec::new()));
        queue(&calls);
        dismissed(Some(8), 3);
        assert_eq!(pending_count(), 1);
        dismissed(Some(7), 3);
        assert_eq!(pending_count(), 0);
        assert!(calls.borrow().is_empty());
        queue(&calls);
        clear();
        clear();
        assert_eq!(*calls.borrow(), [PermissionRequestResult::IGNORE.get_raw()]);
    }
}
