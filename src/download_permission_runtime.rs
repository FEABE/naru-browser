//! CEF permission callbacks live only in this UI-thread queue.

pub use crate::download_permissions::PermissionRow;
use crate::download_permissions::{MAX_ORIGIN_BYTES, Model, Request, SiteSetting, site_origin};
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

fn source_target(token: u64) -> Result<crate::download_permissions::Target, String> {
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        if state.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        state
            .model
            .get(token)
            .filter(|_| state.callbacks.contains_key(&token))
            .map(|request| request.target.clone())
            .ok_or_else(|| "다운로드 권한 요청이 만료되었습니다.".into())
    })
}

pub(crate) fn reveal_source(token: u64) -> Result<i32, String> {
    if !crate::download_runtime::source_reveal_allowed() {
        return Err("저장 위치 선택 창이나 종료 처리가 끝난 뒤 확인해 주세요.".into());
    }
    let target = source_target(token)?;
    // Merely looking at a page must not consume or approve its prompt. All
    // preflight calls below read Rust state only, before selection runs CEF.
    crate::browser::validate_download_permission(&target)?;
    let source = crate::browser::download_context(target.tab)?.source_target;
    crate::browser::reveal_download_source(&source)
}

#[derive(Clone, Debug)]
pub(crate) struct SiteSnapshot {
    pub target: crate::browser::DownloadSiteTarget,
    pub origin: String,
    pub setting: SiteSetting,
}

fn site_context(
    target: &crate::browser::DownloadSiteTarget,
) -> Result<(String, RequestContext), String> {
    let (browser, url) = crate::browser::read_download_site_page(target)?;
    let origin = site_origin(&url).map_err(str::to_owned)?;
    let context = browser
        .host()
        .and_then(|host| host.request_context())
        .ok_or_else(|| "현재 사이트 설정을 읽을 수 없습니다.".to_owned())?;
    crate::browser::validate_download_site_target(target)?;
    Ok((origin, context))
}

fn read_site_setting(context: &RequestContext, origin: &CefString) -> SiteSetting {
    let value =
        context.content_setting(Some(origin), None, ContentSettingTypes::AUTOMATIC_DOWNLOADS);
    match value {
        ContentSettingValues::ALLOW => SiteSetting::Allowed,
        ContentSettingValues::ASK => SiteSetting::Ask,
        ContentSettingValues::BLOCK => SiteSetting::Blocked,
        // DEFAULT can also mean an uninitialized context or a failed query.
        _ => SiteSetting::Unknown,
    }
}

pub(crate) fn site_snapshot(
    page: &crate::browser::CurrentPageTarget,
) -> Result<SiteSnapshot, String> {
    if RUNTIME.with(|slot| slot.borrow().shutting_down) {
        return Err("브라우저를 종료하고 있습니다.".into());
    }
    let target = crate::browser::capture_download_site_target(page)?;
    let (origin, context) = site_context(&target)?;
    let setting = read_site_setting(&context, &CefString::from(origin.as_str()));
    crate::browser::validate_download_site_target(&target)?;
    Ok(SiteSnapshot {
        target,
        origin,
        setting,
    })
}

fn reset_stored_allow(
    context: &RequestContext,
    native_origin: &CefString,
    validate: impl Fn() -> Result<(), String>,
) -> Result<SiteSetting, String> {
    // Reject an empty, non-origin or unnormalized scope before any CEF access.
    // Both empty URLs would modify the default for every site.
    let origin = origin_text(native_origin)
        .ok_or_else(|| "초기화할 사이트 주소를 확인할 수 없습니다.".to_owned())?;
    if site_origin(&origin).as_deref() != Ok(origin.as_str()) {
        return Err("초기화할 사이트 주소를 확인할 수 없습니다.".into());
    }
    if read_site_setting(context, native_origin) != SiteSetting::Allowed {
        return Err("사이트의 허용 상태가 바뀌었습니다. 현재 설정을 확인해 주세요.".into());
    }
    validate()?;
    context.set_content_setting(
        Some(native_origin),
        None,
        ContentSettingTypes::AUTOMATIC_DOWNLOADS,
        ContentSettingValues::DEFAULT,
    );
    // The void setter has no success callback. Confirm only the effective
    // saved setting, not every live tab's Chromium download-limiter cache.
    // After this call a changed UI target does not undo the submitted reset.
    match read_site_setting(context, native_origin) {
        setting @ (SiteSetting::Ask | SiteSetting::Blocked) => Ok(setting),
        SiteSetting::Allowed => Err(
            "초기화를 요청했지만 사이트 허용이 유지됩니다. 기본값이나 다른 설정이 적용될 수 있습니다."
                .into(),
        ),
        SiteSetting::Unknown => Err(
            "초기화를 요청했지만 결과를 확인할 수 없습니다. 사이트 설정을 다시 확인해 주세요."
                .into(),
        ),
    }
}

fn cancel_site_prompts(origin: &str) -> bool {
    // IGNORE does not persist a block. Chromium can cancel other queued
    // download callbacks in the same WebContents as well as this prompt.
    ignore_matching(|request| site_origin(&request.row.origin).as_deref() == Ok(origin))
}

pub(crate) fn reset_site(
    snapshot: &SiteSnapshot,
    still_current: impl Fn() -> bool,
) -> Result<SiteSetting, String> {
    if RUNTIME.with(|slot| slot.borrow().shutting_down) {
        return Err("브라우저를 종료하고 있습니다.".into());
    }
    if snapshot.setting != SiteSetting::Allowed || !still_current() {
        return Err("초기화할 사이트 화면이 바뀌었습니다. 다시 확인해 주세요.".into());
    }
    let (origin, context) = site_context(&snapshot.target)?;
    if origin != snapshot.origin {
        return Err("초기화할 사이트 주소가 바뀌었습니다. 다시 확인해 주세요.".into());
    }
    let native_origin = CefString::from(origin.as_str());
    if read_site_setting(&context, &native_origin) != SiteSetting::Allowed || !still_current() {
        return Err("사이트의 허용 상태나 화면이 바뀌었습니다. 다시 확인해 주세요.".into());
    }
    crate::browser::validate_download_site_target(&snapshot.target)?;
    // Continue may synchronously dismiss a prompt, navigate, or refresh UI.
    let cancelled = cancel_site_prompts(&origin);
    let result = reset_stored_allow(&context, &native_origin, || {
        let (_, current_url) = crate::browser::read_download_site_page(&snapshot.target)?;
        if site_origin(&current_url).as_deref() != Ok(origin.as_str()) || !still_current() {
            return Err("페이지가 바뀌어 설정을 초기화하지 않았습니다.".into());
        }
        if RUNTIME.with(|slot| slot.borrow().shutting_down) {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        crate::browser::validate_download_site_target(&snapshot.target)
    });
    result.map_err(|error| {
        if cancelled {
            format!("{error} 대기 중이던 다운로드 권한 요청은 취소했습니다.")
        } else {
            error
        }
    })
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

fn ignore_matching(matches: impl Fn(&Request) -> bool) -> bool {
    let callbacks = take_matching(matches);
    let changed = !callbacks.is_empty();
    for callback in callbacks {
        callback.cont(PermissionRequestResult::IGNORE);
    }
    if changed {
        crate::browser::notify_panel();
    }
    changed
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
    use std::{cell::Cell, rc::Rc};

    #[derive(Debug)]
    struct SettingCall {
        write: bool,
        origin: Option<String>,
        secondary: Option<String>,
        kind: u32,
        value: Option<u32>,
    }

    struct SettingFixture {
        value: Cell<sys::cef_content_setting_values_t>,
        after: sys::cef_content_setting_values_t,
        calls: Rc<RefCell<Vec<SettingCall>>>,
    }

    unsafe fn setting_text(value: *const sys::cef_string_t) -> Option<String> {
        let value = unsafe { value.as_ref() }?;
        if value.length == 0 {
            return None;
        }
        Some(
            String::from_utf16(unsafe { std::slice::from_raw_parts(value.str_, value.length) })
                .unwrap(),
        )
    }

    unsafe extern "C" fn setting_get(
        raw: *mut sys::_cef_request_context_t,
        origin: *const sys::cef_string_t,
        secondary: *const sys::cef_string_t,
        kind: sys::cef_content_setting_types_t,
    ) -> sys::cef_content_setting_values_t {
        let fixture =
            &cef::rc::RcImpl::<sys::_cef_request_context_t, SettingFixture>::get(raw).interface;
        fixture.calls.borrow_mut().push(SettingCall {
            write: false,
            origin: unsafe { setting_text(origin) },
            secondary: unsafe { setting_text(secondary) },
            kind: kind as u32,
            value: None,
        });
        // Native CEF callbacks can reenter the permission runtime.
        let _ = snapshot();
        fixture.value.get()
    }

    unsafe extern "C" fn setting_set(
        raw: *mut sys::_cef_request_context_t,
        origin: *const sys::cef_string_t,
        secondary: *const sys::cef_string_t,
        kind: sys::cef_content_setting_types_t,
        value: sys::cef_content_setting_values_t,
    ) {
        let fixture =
            &cef::rc::RcImpl::<sys::_cef_request_context_t, SettingFixture>::get(raw).interface;
        fixture.calls.borrow_mut().push(SettingCall {
            write: true,
            origin: unsafe { setting_text(origin) },
            secondary: unsafe { setting_text(secondary) },
            kind: kind as u32,
            value: Some(value as u32),
        });
        fixture.value.set(fixture.after);
        let _ = snapshot();
    }

    fn setting_context(
        before: ContentSettingValues,
        after: ContentSettingValues,
    ) -> (RequestContext, Rc<RefCell<Vec<SettingCall>>>) {
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_request_context_t>() };
        object.get_content_setting = Some(setting_get);
        object.set_content_setting = Some(setting_set);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let raw = cef::rc::RcImpl::new(
            object,
            SettingFixture {
                value: Cell::new(before.into()),
                after: after.into(),
                calls: calls.clone(),
            },
        );
        let context =
            cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_request_context_t>());
        (context, calls)
    }

    // CEF's &str constructor calls engine string functions. Unit tests keep a
    // borrowed UTF-16 value alive for the exact native callback invocation,
    // without loading or initializing the CEF library.
    fn reset_fixture(
        context: &RequestContext,
        origin: &str,
        validate: impl Fn() -> Result<(), String>,
    ) -> Result<SiteSetting, String> {
        let mut units: Vec<u16> = origin.encode_utf16().collect();
        let raw = sys::cef_string_t {
            str_: units.as_mut_ptr(),
            length: units.len(),
            dtor: None,
        };
        let native = CefString::from(&raw as *const sys::cef_string_t);
        reset_stored_allow(context, &native, validate)
    }

    #[test]
    fn saved_allow_reset_uses_exact_origin_and_only_automatic_download_default() {
        for (after, expected) in [
            (ContentSettingValues::ASK, SiteSetting::Ask),
            (ContentSettingValues::BLOCK, SiteSetting::Blocked),
        ] {
            let (context, calls) = setting_context(ContentSettingValues::ALLOW, after);
            assert_eq!(
                reset_fixture(&context, "https://example.test:8443", || Ok(())),
                Ok(expected)
            );
            let calls = calls.borrow();
            assert_eq!(calls.len(), 3);
            for call in calls.iter() {
                assert_eq!(call.origin.as_deref(), Some("https://example.test:8443"));
                assert!(call.secondary.is_none());
                assert_eq!(
                    call.kind,
                    ContentSettingTypes::AUTOMATIC_DOWNLOADS.get_raw()
                );
            }
            assert!(!calls[0].write && calls[1].write && !calls[2].write);
            assert_eq!(
                calls[1].value,
                Some(ContentSettingValues::DEFAULT.get_raw())
            );
        }
    }

    #[test]
    fn reset_never_writes_global_invalid_stale_or_unallowed_scope() {
        let (context, calls) =
            setting_context(ContentSettingValues::ALLOW, ContentSettingValues::ASK);
        for origin in [
            "",
            "about:blank",
            "https://example.test/path",
            "https://example.test:443",
            "HTTPS://EXAMPLE.test",
        ] {
            assert!(reset_fixture(&context, origin, || Ok(())).is_err());
        }
        assert!(calls.borrow().is_empty());
        assert!(
            reset_fixture(&context, "https://example.test", || Err(
                "target changed".into()
            ))
            .is_err()
        );
        assert!(!calls.borrow().iter().any(|call| call.write));
        for before in [
            ContentSettingValues::ASK,
            ContentSettingValues::BLOCK,
            ContentSettingValues::DEFAULT,
        ] {
            let (context, calls) = setting_context(before, ContentSettingValues::ASK);
            assert!(
                reset_fixture(&context, "https://example.test", || panic!(
                    "no eligible allow"
                ))
                .is_err()
            );
            assert_eq!(calls.borrow().len(), 1);
            assert!(!calls.borrow()[0].write);
        }
    }

    #[test]
    fn reset_does_not_claim_success_when_effective_allow_or_unknown_remains() {
        for after in [
            ContentSettingValues::ALLOW,
            ContentSettingValues::DEFAULT,
            ContentSettingValues::SESSION_ONLY,
        ] {
            let (context, calls) = setting_context(ContentSettingValues::ALLOW, after);
            assert!(reset_fixture(&context, "http://example.test", || Ok(())).is_err());
            assert_eq!(calls.borrow().iter().filter(|call| call.write).count(), 1);
        }
    }

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
        queue_for(calls, 7, "https://example.test")
    }

    fn queue_for(calls: &std::rc::Rc<RefCell<Vec<u32>>>, tab: i32, origin: &str) -> u64 {
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_permission_prompt_callback_t>() };
        object.cont = Some(record_result);
        let raw = cef::rc::RcImpl::new(
            object,
            RecordingCallback {
                calls: calls.clone(),
                tab,
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
                        tab,
                        epoch: 1,
                        owner: None,
                    },
                    3,
                    origin,
                    "사용자",
                )
                .unwrap();
            state.callbacks.insert(token, callback);
            token
        })
    }

    #[test]
    fn source_lookup_never_consumes_or_approves_a_permission() {
        clear();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let token = queue(&calls);
        let rows = snapshot();
        let target = source_target(token).unwrap();
        assert_eq!(target.tab, 7);
        assert_eq!(source_target(token), Ok(target));
        assert!(source_target(token + 1).is_err());
        assert_eq!(snapshot(), rows);
        assert!(calls.borrow().is_empty());
        RUNTIME.with(|slot| slot.borrow_mut().shutting_down = true);
        assert!(source_target(token).is_err());
        assert!(calls.borrow().is_empty());
        RUNTIME.with(|slot| slot.borrow_mut().shutting_down = false);
        dismissed(Some(7), 3);
        assert!(source_target(token).is_err());
        assert_eq!(pending_count(), 0);
        assert!(calls.borrow().is_empty());
        clear();
    }

    #[test]
    fn reset_cancels_matching_origin_prompts_without_crossing_scheme_or_port() {
        clear();
        let matching = Rc::new(RefCell::new(Vec::new()));
        let other = Rc::new(RefCell::new(Vec::new()));
        let token = queue_for(&matching, 7, "https://EXAMPLE.test:443");
        queue_for(&other, 8, "http://example.test");
        queue_for(&other, 9, "https://example.test:8443");
        assert!(cancel_site_prompts("https://example.test"));
        assert_eq!(
            *matching.borrow(),
            [PermissionRequestResult::IGNORE.get_raw()]
        );
        assert!(other.borrow().is_empty());
        assert_eq!(pending_count(), 2);
        assert!(decide(token, true).is_err());
        assert!(!cancel_site_prompts("https://example.test"));
        clear();
        assert_eq!(other.borrow().len(), 2);
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
