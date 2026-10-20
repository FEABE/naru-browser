//! JavaScript dialog callbacks for AI-owned tabs, retained on the UI thread only.
//!
//! A queued dialog keeps its renderer blocked until an agent answers it, the
//! task hands control back, or the tab closes. No RefCell borrow is held while
//! a CEF callback continues, because `cont` may reenter CEF handlers.

use crate::dialogs::{DialogError, Entry, Kind, Model, accept_text};
use crate::protocol::DialogAction;
use cef::*;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

const RESUME_GRACE: Duration = Duration::from_secs(5);

/// How a dialog from a tab is routed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// User-space tab, or a paused (user-controlled) task tab: CEF's native dialog.
    Default,
    /// Active task tab: hold the callback for the CLI.
    Queue,
    /// Task tab that is finishing or closing: never block the close.
    AutoClose,
}

#[derive(Default)]
struct Runtime {
    model: Model,
    callbacks: BTreeMap<u64, JsdialogCallback>,
    /// Tabs with an explicit close underway; beforeunload is accepted for them.
    closing: BTreeSet<i32>,
    /// Tabs whose callback this runtime is continuing right now. CEF answers
    /// that with OnDialogClosed/OnResetDialogState synchronously; those resets
    /// must not drop a different, already queued dialog of the same tab.
    self_continuing: BTreeSet<i32>,
    /// When a queued dialog of the tab was last answered or settled.
    resumed_at: BTreeMap<i32, Instant>,
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default()); }

fn text(value: Option<&CefString>) -> String {
    value.map(ToString::to_string).unwrap_or_default()
}

pub fn clear() {
    let callbacks = RUNTIME.with(|slot| std::mem::take(&mut *slot.borrow_mut()).callbacks);
    drop(callbacks);
}

pub fn pending_count() -> usize {
    RUNTIME.with(|slot| slot.borrow().model.len())
}

pub fn pending_tabs() -> BTreeSet<i32> {
    RUNTIME.with(|slot| {
        slot.borrow()
            .model
            .rows()
            .iter()
            .map(|row| row.tab)
            .collect()
    })
}

pub fn tab_json(tab: i32) -> Option<Value> {
    RUNTIME.with(|slot| slot.borrow().model.get(tab).map(Entry::to_json))
}

pub fn tab_overflow(tab: i32) -> u64 {
    RUNTIME.with(|slot| slot.borrow().model.overflow_for(tab))
}

pub fn status_json() -> Value {
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        let requests = state
            .model
            .rows()
            .iter()
            .map(|row| json!({"tab":row.tab,"type":row.kind.label()}))
            .collect::<Vec<_>>();
        json!({"pending":requests.len(),"overflow":state.model.overflow_total(),"requests":requests})
    })
}

/// True while the renderer of `tab` cannot acknowledge focus cleanup: a dialog
/// is queued, or one was answered within the grace period and the resumed
/// renderer has not had time to acknowledge yet.
pub(crate) fn focus_grace_active(tab: i32) -> bool {
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        state.model.get(tab).is_some()
            || state
                .resumed_at
                .get(&tab)
                .is_some_and(|at| at.elapsed() < RESUME_GRACE)
    })
}

/// Continue a callback that does not belong to the queued entry (auto-close or
/// overflow) while shielding that entry from the reentrant reset.
fn continue_guarded(tab: i32, callback: &JsdialogCallback, success: bool) {
    RUNTIME.with(|slot| slot.borrow_mut().self_continuing.insert(tab));
    callback.cont(i32::from(success), None);
    RUNTIME.with(|slot| slot.borrow_mut().self_continuing.remove(&tab));
}

pub(crate) fn is_closing(tab: i32) -> bool {
    RUNTIME.with(|slot| slot.borrow().closing.contains(&tab))
}

/// Shared entry for `on_jsdialog` and `on_before_unload_dialog`.
/// Returns true when this runtime owns the callback (CEF must not show a dialog).
pub(crate) fn route(
    tab: Option<i32>,
    origin_url: Option<String>,
    kind: Kind,
    message: Option<&CefString>,
    default_prompt: Option<&CefString>,
    callback: Option<&JsdialogCallback>,
) -> bool {
    let (Some(tab), Some(callback)) = (tab, callback) else {
        return false;
    };
    match crate::browser::dialog_disposition(tab) {
        Disposition::Default => false,
        Disposition::AutoClose => {
            // Leave the page for beforeunload, cancel everything else.
            continue_guarded(tab, callback, kind == Kind::BeforeUnload);
            true
        }
        Disposition::Queue => {
            let origin = crate::download_permissions::site_origin(&origin_url.unwrap_or_default())
                .unwrap_or_default();
            let inserted = RUNTIME.with(|slot| {
                let mut state = slot.borrow_mut();
                let token = state.model.insert(
                    tab,
                    kind,
                    &text(message),
                    &text(default_prompt),
                    &origin,
                )?;
                state.callbacks.insert(token, callback.clone());
                Ok::<_, crate::dialogs::Refusal>(token)
            });
            match inserted {
                Err(_) => {
                    // Over the per-tab or global cap: never block silently.
                    continue_guarded(tab, callback, false);
                    crate::browser::notify_panel();
                }
                Ok(_) => {
                    let dialog = tab_json(tab).unwrap_or(Value::Null);
                    crate::browser::dialog_opened(tab, dialog);
                    crate::browser::notify_panel();
                }
            }
            true
        }
    }
}

/// Queue a dialog without a CEF callback (state-machine tests only).
#[cfg(test)]
pub(crate) fn test_insert(tab: i32, kind: Kind, message: &str, default_prompt: &str) -> bool {
    RUNTIME.with(|slot| {
        slot.borrow_mut()
            .model
            .insert(tab, kind, message, default_prompt, "https://example.test")
            .is_ok()
    })
}

/// Run a CEF-style reset while the tab is shielded, as during `continue_guarded`.
#[cfg(test)]
pub(crate) fn test_reset_during_guard(tab: i32) {
    RUNTIME.with(|slot| slot.borrow_mut().self_continuing.insert(tab));
    reset_tab(tab);
    RUNTIME.with(|slot| slot.borrow_mut().self_continuing.remove(&tab));
}

/// Answer the pending dialog of `tab`. The callback continues outside all borrows.
pub(crate) fn answer(tab: i32, action: &DialogAction) -> Result<Value, DialogError> {
    let (entry, callback, reply_text) = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let entry = state.model.get(tab).cloned().ok_or(DialogError {
            code: "NO_DIALOG",
            message: "This tab has no pending JavaScript dialog.",
        })?;
        let reply_text = match action {
            DialogAction::Accept { text } => accept_text(&entry, text.as_deref())?,
            DialogAction::Dismiss {} => None,
        };
        state.model.take(tab);
        state.resumed_at.insert(tab, Instant::now());
        let callback = state.callbacks.remove(&entry.token);
        Ok::<_, DialogError>((entry, callback, reply_text))
    })?;
    let accepted = matches!(action, DialogAction::Accept { .. });
    if let Some(callback) = callback {
        match &reply_text {
            Some(value) => {
                callback.cont(i32::from(accepted), Some(&CefString::from(value.as_str())))
            }
            None => callback.cont(i32::from(accepted), None),
        }
    }
    crate::browser::notify_panel();
    let mut result = json!({
        "tab": tab,
        "action": if accepted { "accept" } else { "dismiss" },
        "dialog": entry.to_json(),
    });
    if let (Some(value), true) = (reply_text, entry.kind == Kind::Prompt) {
        result["text"] = json!(value);
    }
    Ok(result)
}

/// Settle every pending dialog of `tabs`. With `closing` the tabs are marked as
/// closing: beforeunload is accepted (the explicit close proceeds) and later
/// dialogs are auto-handled. Otherwise dialogs are dismissed (stay on the page).
pub(crate) fn settle_tabs(tabs: impl IntoIterator<Item = i32>, closing: bool) {
    let taken = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        let mut taken = Vec::new();
        for tab in tabs {
            if closing {
                state.closing.insert(tab);
            }
            if let Some(entry) = state.model.take(tab) {
                state.resumed_at.insert(tab, Instant::now());
                let callback = state.callbacks.remove(&entry.token);
                taken.push((entry, callback));
            }
        }
        taken
    });
    let changed = !taken.is_empty();
    for (entry, callback) in taken {
        if let Some(callback) = callback {
            callback.cont(i32::from(closing && entry.kind == Kind::BeforeUnload), None);
        }
    }
    if changed {
        crate::browser::notify_panel();
    }
}

/// A released tab is no longer closing on behalf of a task.
pub(crate) fn unmark_closing(tab: i32) {
    RUNTIME.with(|slot| slot.borrow_mut().closing.remove(&tab));
}

/// CEF already cancelled or closed the dialog; just forget it.
pub(crate) fn reset_tab(tab: i32) {
    let dropped = RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.self_continuing.contains(&tab) {
            return None;
        }
        let entry = state.model.take(tab)?;
        state.callbacks.remove(&entry.token)
    });
    let changed = dropped.is_some();
    drop(dropped);
    if changed {
        crate::browser::notify_panel();
    }
}

/// The browser is gone: drop its entry, close flag and overflow counter.
pub(crate) fn forget_tab(tab: i32) {
    reset_tab(tab);
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.closing.remove(&tab);
        state.self_continuing.remove(&tab);
        state.resumed_at.remove(&tab);
        state.model.forget_tab(tab);
    });
}

wrap_jsdialog_handler! {
    pub(crate) struct TabDialogHandler;
    impl JsdialogHandler {
        fn on_jsdialog(&self, browser: Option<&mut Browser>, origin_url: Option<&CefString>,
            dialog_type: JsdialogType, message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>, callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>) -> ::std::os::raw::c_int {
            let kind = if dialog_type == JsdialogType::ALERT {
                Kind::Alert
            } else if dialog_type == JsdialogType::CONFIRM {
                Kind::Confirm
            } else {
                Kind::Prompt
            };
            i32::from(route(browser.map(|browser| browser.identifier()), origin_url.map(ToString::to_string), kind,
                message_text, default_prompt_text, callback.as_deref()))
        }
        fn on_before_unload_dialog(&self, browser: Option<&mut Browser>,
            message_text: Option<&CefString>, _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>) -> ::std::os::raw::c_int {
            let url = browser.as_ref().and_then(|browser| browser.main_frame())
                .map(|frame| CefString::from(&frame.url()).to_string());
            i32::from(route(browser.map(|browser| browser.identifier()), url, Kind::BeforeUnload,
                message_text, None, callback.as_deref()))
        }
        fn on_reset_dialog_state(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser { reset_tab(browser.identifier()); }
        }
        fn on_dialog_closed(&self, _browser: Option<&mut Browser>) {
            // Fires synchronously inside any cont (also for other dialogs of the
            // tab); entries are removed by the answer, settle and reset paths.
            crate::browser::notify_panel();
        }
    }
}
