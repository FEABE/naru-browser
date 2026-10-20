//! Permission callbacks and live media flags, retained only on the UI thread.

use crate::site_permissions::{Kind, MAX_ORIGIN_BYTES, Model, Request, exact_origin};
pub use crate::site_permissions::{Row, SiteSetting};
use cef::*;
use std::{cell::RefCell, collections::BTreeMap};

enum Callback {
    Generic(PermissionPromptCallback),
    Media(MediaAccessCallback),
}

impl Callback {
    fn cancel(self) {
        match self {
            Self::Generic(callback) => callback.cont(PermissionRequestResult::IGNORE),
            Self::Media(callback) => callback.cancel(),
        }
    }

    fn dismiss(self) {
        match self {
            Self::Generic(callback) => callback.cont(PermissionRequestResult::DISMISS),
            Self::Media(callback) => callback.cancel(),
        }
    }

    fn allow(self, kind: &Kind) {
        match (self, kind) {
            (Self::Generic(callback), Kind::Generic { .. }) => {
                callback.cont(PermissionRequestResult::ACCEPT);
            }
            (Self::Media(callback), Kind::Media { mask, .. }) => callback.cont(*mask),
            (callback, _) => callback.cancel(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaRow {
    pub tab: i32,
    pub audio: bool,
    pub video: bool,
}

#[derive(Default)]
struct Runtime {
    model: Model,
    callbacks: BTreeMap<u64, Callback>,
    media: BTreeMap<i32, MediaRow>,
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

pub fn notice() -> Option<String> {
    RUNTIME.with(|slot| slot.borrow().notice.clone())
}

fn set_notice(value: Option<String>) {
    RUNTIME.with(|slot| slot.borrow_mut().notice = value);
}

pub fn active_media() -> Vec<MediaRow> {
    RUNTIME.with(|slot| slot.borrow().media.values().cloned().collect())
}

pub fn media_summary() -> Option<String> {
    let rows = active_media();
    if rows.is_empty() {
        return None;
    }
    let mut text = rows
        .iter()
        .take(8)
        .map(|row| {
            let label = match (row.audio, row.video) {
                (true, true) => "마이크, 카메라",
                (true, false) => "마이크",
                _ => "카메라",
            };
            format!("{label} 스트림 활성: 탭 {}", row.tab)
        })
        .collect::<Vec<_>>()
        .join("; ");
    if rows.len() > 8 {
        text.push_str(&format!("; 그 외 {}개 탭", rows.len() - 8));
    }
    Some(text)
}

pub(crate) fn media_changed(tab: i32, audio: bool, video: bool) {
    // Native events cannot create entries for unavailable tabs. This map has
    // at most one two-flag row per live tab and stores no device details.
    if !crate::browser::site_permission_tab_available(tab) {
        return;
    }
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.shutting_down {
            return;
        }
        if audio || video {
            state.media.insert(tab, MediaRow { tab, audio, video });
        } else {
            state.media.remove(&tab);
        }
    });
    crate::browser::notify_panel();
}

pub(crate) fn media_stopped(tab: i32) {
    let changed = RUNTIME.with(|slot| slot.borrow_mut().media.remove(&tab).is_some());
    if changed {
        crate::browser::notify_panel();
    }
}

fn origin_text(value: Option<&CefString>) -> Result<String, String> {
    let units = value
        .and_then(CefString::as_slice)
        .filter(|units| units.len() <= MAX_ORIGIN_BYTES)
        .ok_or_else(|| "권한을 요청한 사이트 주소를 확인할 수 없습니다.".to_owned())?;
    let mut text = String::new();
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.map_err(|_| "사이트 주소를 읽을 수 없습니다.".to_owned())?;
        if text.len() + character.len_utf8() > MAX_ORIGIN_BYTES {
            return Err("권한을 요청한 사이트 주소가 너무 깁니다.".into());
        }
        text.push(character);
    }
    exact_origin(&text).map_err(str::to_owned)
}

fn reject(callback: Callback, error: String) -> bool {
    callback.cancel();
    set_notice(Some(error));
    crate::browser::notify_panel();
    true
}

fn queue(
    context: crate::browser::SitePermissionContext,
    kind: Kind,
    origin: String,
    callback: Callback,
) -> bool {
    // The caller retained its callback before reaching here. Revalidate after
    // all CEF/native reads and that retain, not only at initial capture.
    let mut incoming = Some(callback);
    let queued = crate::browser::validate_site_permission(&context.target).and_then(|()| {
        RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            if state.shutting_down {
                return Err("브라우저를 종료하고 있습니다.".to_owned());
            }
            let token = state
                .model
                .insert(context.target, kind, &origin, &context.source)
                .map_err(str::to_owned)?;
            state.callbacks.insert(token, incoming.take().unwrap());
            state.notice = None;
            Ok(())
        })
    });
    if let Some(callback) = incoming {
        callback.cancel();
    }
    if let Err(error) = queued {
        set_notice(Some(error));
    }
    crate::browser::notify_panel();
    true
}

pub(crate) fn show(
    tab: Option<i32>,
    prompt: u64,
    origin: Option<&CefString>,
    mask: u32,
    callback: Option<&PermissionPromptCallback>,
) -> bool {
    let Some(callback) = callback else {
        set_notice(Some(
            "사이트 권한 요청을 확인할 수 없어 허용하지 않았습니다.".into(),
        ));
        crate::browser::notify_panel();
        return false; // Alloy's default is IGNORE, never a native approval UI.
    };
    let prepared = (|| {
        let kind = Kind::generic(prompt, mask).map_err(str::to_owned)?;
        let tab = tab.ok_or_else(|| "권한을 요청한 탭을 확인할 수 없습니다.".to_owned())?;
        let context = crate::browser::site_permission_context(tab)?;
        let origin = origin_text(origin)?;
        Ok::<_, String>((context, kind, origin))
    })();
    let callback = Callback::Generic(callback.clone());
    match prepared {
        Ok((context, kind, origin)) => queue(context, kind, origin, callback),
        Err(error) => reject(callback, error),
    }
}

pub(crate) fn show_media(
    tab: Option<i32>,
    frame: Option<&Frame>,
    origin: Option<&CefString>,
    mask: u32,
    callback: Option<&MediaAccessCallback>,
) -> bool {
    let Some(callback) = callback else {
        set_notice(Some(
            "미디어 권한 요청을 확인할 수 없어 허용하지 않았습니다.".into(),
        ));
        crate::browser::notify_panel();
        return false; // Alloy's default denies media access.
    };
    let prepared = (|| {
        // Validate the full mask before any frame lookup; screen sharing needs
        // an explicit surface picker, which this implementation does not have.
        Kind::media(mask, "pending".into()).map_err(str::to_owned)?;
        let tab = tab.ok_or_else(|| "미디어를 요청한 탭을 확인할 수 없습니다.".to_owned())?;
        let context = crate::browser::site_permission_context(tab)?;
        let origin = origin_text(origin)?;
        let frame = frame.ok_or_else(|| "미디어를 요청한 문서를 확인할 수 없습니다.".to_owned())?;
        let frame = crate::browser::capture_site_permission_frame(&context.target, frame, &origin)?;
        let kind = Kind::media(mask, frame).map_err(str::to_owned)?;
        Ok::<_, String>((context, kind, origin))
    })();
    let callback = Callback::Media(callback.clone());
    match prepared {
        Ok((context, kind, origin)) => queue(context, kind, origin, callback),
        Err(error) => reject(callback, error),
    }
}

fn pending(token: u64) -> Result<Request, String> {
    RUNTIME.with(|slot| {
        let state = slot.borrow();
        if state.shutting_down {
            return Err("브라우저를 종료하고 있습니다.".into());
        }
        state
            .model
            .get(token)
            .filter(|_| state.callbacks.contains_key(&token))
            .cloned()
            .ok_or_else(|| "사이트 권한 요청이 만료되었습니다.".into())
    })
}

fn validate(request: &Request) -> Result<(), String> {
    crate::browser::validate_site_permission(&request.target)?;
    if let Kind::Media { frame, .. } = &request.kind {
        crate::browser::validate_site_permission_frame(
            &request.target,
            frame,
            &request.row.origin,
        )?;
    }
    crate::browser::validate_site_permission(&request.target)
}

pub fn decide(token: u64, allow: bool) -> Result<(), String> {
    let request = pending(token)?;
    let valid = validate(&request);
    // Native frame getters above may reenter a cancellation. Keep the row
    // available to cancel until validation ends, then consume it exactly once.
    let callback = RUNTIME
        .with(|slot| {
            let mut state = slot.borrow_mut();
            let current = state.model.take(token)?;
            let callback = state.callbacks.remove(&token)?;
            Some((
                callback,
                !state.shutting_down && current.target == request.target,
            ))
        })
        .ok_or_else(|| "사이트 권한 요청이 만료되었습니다.".to_owned())?;
    if valid.is_ok() && callback.1 {
        if allow {
            callback.0.allow(&request.kind);
        } else {
            callback.0.dismiss();
        }
    } else {
        callback.0.cancel();
    }
    let valid = valid.and_then(|()| {
        if callback.1 {
            Ok(())
        } else {
            Err("사이트 권한 요청이 만료되었습니다.".into())
        }
    });
    if let Err(error) = &valid {
        set_notice(Some(error.clone()));
    }
    crate::browser::notify_panel();
    valid
}

pub fn reveal_source(token: u64) -> Result<i32, String> {
    if !crate::download_runtime::source_reveal_allowed()
        || crate::file_selection_runtime::is_choosing()
    {
        return Err("열려 있는 파일 선택 창을 먼저 닫아 주세요.".into());
    }
    let request = pending(token)?;
    validate(&request)?;
    pending(token)?;
    crate::browser::reveal_download_source(&request.target.source)
}

fn take_matching(matches: impl Fn(&Request) -> bool) -> Vec<Callback> {
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

fn cancel_matching(matches: impl Fn(&Request) -> bool) -> bool {
    let callbacks = take_matching(matches);
    let changed = !callbacks.is_empty();
    for callback in callbacks {
        callback.cancel();
    }
    if changed {
        crate::browser::notify_panel();
    }
    changed
}

pub(crate) fn dismissed(tab: Option<i32>, prompt: u64) {
    let Some(tab) = tab else {
        return;
    };
    let callbacks = take_matching(|request| {
        request.target.source.tab == tab
            && matches!(request.kind, Kind::Generic { prompt: current, .. } if current == prompt)
    });
    let changed = !callbacks.is_empty();
    drop(callbacks); // CEF already completed this prompt; never Continue again.
    if changed {
        crate::browser::notify_panel();
    }
}

pub(crate) fn cancel_tab(tab: i32) {
    cancel_matching(|request| request.target.source.tab == tab);
    // This cancels pending decisions, not streams that Chromium already began.
}

pub(crate) fn cancel_task(task: &str, only_tab: Option<i32>) {
    cancel_matching(|request| {
        only_tab.is_none_or(|tab| request.target.source.tab == tab)
            && request
                .target
                .source
                .owner
                .as_ref()
                .is_some_and(|(owner, _)| owner.task == task)
    });
}

pub(crate) fn clear() {
    RUNTIME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.shutting_down = true;
        state.media.clear();
        state.notice = None;
    });
    cancel_matching(|_| true);
}

#[derive(Clone, Debug)]
pub(crate) struct SiteSnapshot {
    pub target: crate::browser::DownloadSiteTarget,
    pub origin: String,
    pub geolocation: SiteSetting,
    pub notifications: SiteSetting,
}

impl SiteSnapshot {
    pub fn can_reset(&self) -> bool {
        self.geolocation.can_reset() || self.notifications.can_reset()
    }
}

fn site_context(
    target: &crate::browser::DownloadSiteTarget,
) -> Result<(String, RequestContext), String> {
    let (browser, url) = crate::browser::read_download_site_page(target)?;
    let origin = crate::download_permissions::site_origin(&url).map_err(str::to_owned)?;
    let context = browser
        .host()
        .and_then(|host| host.request_context())
        .ok_or_else(|| "현재 사이트 권한을 읽을 수 없습니다.".to_owned())?;
    crate::browser::validate_download_site_target(target)?;
    Ok((origin, context))
}

fn read_setting(
    context: &RequestContext,
    origin: &CefString,
    kind: ContentSettingTypes,
) -> SiteSetting {
    match context.content_setting(Some(origin), None, kind) {
        ContentSettingValues::ALLOW => SiteSetting::Allowed,
        ContentSettingValues::BLOCK => SiteSetting::Blocked,
        ContentSettingValues::ASK => SiteSetting::Ask,
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
    let native = CefString::from(origin.as_str());
    let geolocation = read_setting(&context, &native, ContentSettingTypes::GEOLOCATION);
    let notifications = read_setting(&context, &native, ContentSettingTypes::NOTIFICATIONS);
    crate::browser::validate_download_site_target(&target)?;
    Ok(SiteSnapshot {
        target,
        origin,
        geolocation,
        notifications,
    })
}

fn reset_settings(
    context: &RequestContext,
    native: &CefString,
    expected: [SiteSetting; 2],
    validate: impl Fn() -> Result<(), String>,
) -> Result<(), String> {
    // No empty URL or wildcard fallback: CEF would otherwise modify defaults.
    let origin = origin_text(Some(native))?;
    if !expected.iter().any(|setting| setting.can_reset()) {
        return Err("초기화할 사이트 권한을 확인할 수 없습니다.".into());
    }
    // Require the exact canonical scope that was displayed, including port.
    if native.as_slice().is_none_or(|units| !units.iter().copied().eq(origin.encode_utf16())) {
        return Err("초기화할 사이트 주소를 확인할 수 없습니다.".into());
    }
    let kinds = [
        ContentSettingTypes::GEOLOCATION,
        ContentSettingTypes::NOTIFICATIONS,
    ];
    for (kind, expected) in kinds.iter().zip(expected) {
        if read_setting(context, native, *kind) != expected {
            return Err("사이트 권한 상태가 바뀌었습니다. 다시 확인해 주세요.".into());
        }
    }
    validate()?;
    let mut writes = 0;
    for (kind, expected) in kinds.into_iter().zip(expected) {
        if !expected.can_reset() {
            continue;
        }
        // Each void setter can reenter. A later failure must not claim rollback
        // of a previous setting that has already been submitted.
        if let Err(error) = validate() {
            return Err(if writes > 0 {
                format!("일부 권한 초기화 요청 후 중단했습니다. {error}")
            } else {
                error
            });
        }
        context.set_content_setting(Some(native), None, kind, ContentSettingValues::DEFAULT);
        writes += 1;
        if read_setting(context, native, kind) != SiteSetting::Ask {
            return Err("초기화를 요청했지만 요청 시 확인 상태를 확인하지 못했습니다. 정책이나 기본 설정이 적용될 수 있습니다.".into());
        }
    }
    Ok(())
}

pub(crate) fn reset_site(
    snapshot: &SiteSnapshot,
    still_current: impl Fn() -> bool,
) -> Result<(), String> {
    if !snapshot.can_reset() || !still_current() || RUNTIME.with(|slot| slot.borrow().shutting_down)
    {
        return Err("초기화할 사이트 화면이 바뀌었습니다. 다시 확인해 주세요.".into());
    }
    let (origin, context) = site_context(&snapshot.target)?;
    if origin != snapshot.origin {
        return Err("초기화할 사이트 주소가 바뀌었습니다.".into());
    }
    let native = CefString::from(origin.as_str());
    if read_setting(&context, &native, ContentSettingTypes::GEOLOCATION) != snapshot.geolocation
        || read_setting(&context, &native, ContentSettingTypes::NOTIFICATIONS)
            != snapshot.notifications
        || !still_current()
        || RUNTIME.with(|slot| slot.borrow().shutting_down)
    {
        return Err("사이트 권한 상태나 화면이 바뀌었습니다. 다시 확인해 주세요.".into());
    }
    crate::browser::validate_download_site_target(&snapshot.target)?;
    let canceled =
        cancel_matching(|request| request.kind.is_generic() && request.row.origin == origin);
    let result = reset_settings(
        &context,
        &native,
        [snapshot.geolocation, snapshot.notifications],
        || {
            if !still_current() || RUNTIME.with(|slot| slot.borrow().shutting_down) {
                return Err("현재 사이트 화면이나 브라우저 상태가 바뀌었습니다.".into());
            }
            let (_, url) = crate::browser::read_download_site_page(&snapshot.target)?;
            if crate::download_permissions::site_origin(&url).as_deref() != Ok(origin.as_str()) {
                return Err("현재 사이트 주소가 바뀌었습니다.".into());
            }
            crate::browser::validate_download_site_target(&snapshot.target)
        },
    );
    result.map_err(|error| {
        if canceled {
            format!("{error} 대기 중이던 이 사이트의 권한 요청은 취소했습니다.")
        } else {
            error
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site_permissions::Target;
    use std::{cell::Cell, rc::Rc};

    struct Recorder {
        calls: Rc<RefCell<Vec<u32>>>,
    }
    impl Drop for Recorder {
        fn drop(&mut self) {
            assert!(RUNTIME.with(|slot| slot.try_borrow_mut().is_ok()));
        }
    }

    unsafe extern "C" fn generic_result(
        raw: *mut sys::_cef_permission_prompt_callback_t,
        result: sys::cef_permission_request_result_t,
    ) {
        let data =
            &cef::rc::RcImpl::<sys::_cef_permission_prompt_callback_t, Recorder>::get(raw).interface;
        data.calls.borrow_mut().push(result as u32);
        dismissed(Some(7), 1);
        cancel_tab(7);
    }

    unsafe extern "C" fn media_result(raw: *mut sys::_cef_media_access_callback_t, mask: u32) {
        let data = &cef::rc::RcImpl::<sys::_cef_media_access_callback_t, Recorder>::get(raw).interface;
        data.calls.borrow_mut().push(mask);
        cancel_tab(7);
    }

    unsafe extern "C" fn media_cancel(raw: *mut sys::_cef_media_access_callback_t) {
        unsafe {
            media_result(raw, 0);
        }
    }

    #[test]
    fn site_permission_callbacks_cancel_once_outside_borrows_and_forward_exact_media_mask() {
        clear();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_permission_prompt_callback_t>() };
        object.cont = Some(generic_result);
        let raw = cef::rc::RcImpl::new(
            object,
            Recorder {
                calls: calls.clone(),
            },
        );
        let callback = cef::rc::ConvertReturnValue::wrap_result(
            raw.cast::<sys::_cef_permission_prompt_callback_t>(),
        );
        let token = RUNTIME.with(|slot| {
            let mut state = slot.borrow_mut();
            state.shutting_down = false;
            let token = state
                .model
                .insert(
                    Target {
                        source: crate::downloads::SourceTarget {
                            tab: 7,
                            generation: 1,
                            document_epoch: 1,
                            owner: None,
                        },
                        generation: 1,
                    },
                    Kind::generic(1, crate::site_permissions::GEOLOCATION).unwrap(),
                    "https://example.test",
                    "user",
                )
                .unwrap();
            state.callbacks.insert(token, Callback::Generic(callback));
            token
        });
        cancel_tab(7);
        assert_eq!(
            &*calls.borrow(),
            &[PermissionRequestResult::IGNORE.get_raw()]
        );
        assert_eq!(pending_count(), 0);
        assert!(decide(token, true).is_err());
        assert!(decide(token, false).is_err());
        assert_eq!(calls.borrow().len(), 1);
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_permission_prompt_callback_t>() };
        object.cont = Some(generic_result);
        let raw = cef::rc::RcImpl::new(
            object,
            Recorder {
                calls: calls.clone(),
            },
        );
        let callback = cef::rc::ConvertReturnValue::wrap_result(
            raw.cast::<sys::_cef_permission_prompt_callback_t>(),
        );
        Callback::Generic(callback).dismiss();
        assert_eq!(
            &*calls.borrow(),
            &[
                PermissionRequestResult::IGNORE.get_raw(),
                PermissionRequestResult::DISMISS.get_raw(),
            ]
        );
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_media_access_callback_t>() };
        object.cont = Some(media_result);
        object.cancel = Some(media_cancel);
        let raw = cef::rc::RcImpl::new(
            object,
            Recorder {
                calls: calls.clone(),
            },
        );
        let callback = cef::rc::ConvertReturnValue::wrap_result(
            raw.cast::<sys::_cef_media_access_callback_t>(),
        );
        Callback::Media(callback).allow(&Kind::media(3, "frame".into()).unwrap());
        assert_eq!(
            &*calls.borrow(),
            &[
                PermissionRequestResult::IGNORE.get_raw(),
                PermissionRequestResult::DISMISS.get_raw(),
                3,
            ]
        );
        clear();
    }

    struct Settings {
        values: RefCell<BTreeMap<u32, sys::cef_content_setting_values_t>>,
        writes: Rc<RefCell<Vec<(String, u32, u32)>>>,
    }

    unsafe extern "C" fn setting_get(
        raw: *mut sys::_cef_request_context_t,
        _origin: *const sys::cef_string_t,
        _secondary: *const sys::cef_string_t,
        kind: sys::cef_content_setting_types_t,
    ) -> sys::cef_content_setting_values_t {
        let data = &cef::rc::RcImpl::<sys::_cef_request_context_t, Settings>::get(raw).interface;
        let _ = snapshot();
        data.values.borrow()[&(kind as u32)]
    }

    unsafe extern "C" fn setting_set(
        raw: *mut sys::_cef_request_context_t,
        origin: *const sys::cef_string_t,
        secondary: *const sys::cef_string_t,
        kind: sys::cef_content_setting_types_t,
        value: sys::cef_content_setting_values_t,
    ) {
        let data = &cef::rc::RcImpl::<sys::_cef_request_context_t, Settings>::get(raw).interface;
        assert!(secondary.is_null());
        let origin = unsafe { &*origin };
        let text =
            String::from_utf16(unsafe { std::slice::from_raw_parts(origin.str_, origin.length) })
                .unwrap();
        data.writes
            .borrow_mut()
            .push((text, kind as u32, value as u32));
        data.values
            .borrow_mut()
            .insert(kind as u32, ContentSettingValues::ASK.into());
        let _ = snapshot();
    }

    #[test]
    fn site_permission_reset_limits_keys_origin_and_stops_after_reentrant_change() {
        let mut object = unsafe { std::mem::zeroed::<sys::_cef_request_context_t>() };
        object.get_content_setting = Some(setting_get);
        object.set_content_setting = Some(setting_set);
        let writes = Rc::new(RefCell::new(Vec::new()));
        let raw = cef::rc::RcImpl::new(
            object,
            Settings {
                values: RefCell::new(BTreeMap::from([
                    (
                        ContentSettingTypes::GEOLOCATION.get_raw(),
                        ContentSettingValues::ALLOW.into(),
                    ),
                    (
                        ContentSettingTypes::NOTIFICATIONS.get_raw(),
                        ContentSettingValues::BLOCK.into(),
                    ),
                ])),
                writes: writes.clone(),
            },
        );
        let context =
            cef::rc::ConvertReturnValue::wrap_result(raw.cast::<sys::_cef_request_context_t>());
        let run = |origin: &str, validate: &dyn Fn() -> Result<(), String>| {
            let mut units = origin.encode_utf16().collect::<Vec<_>>();
            let raw = sys::cef_string_t {
                str_: units.as_mut_ptr(),
                length: units.len(),
                dtor: None,
            };
            let native = CefString::from(&raw as *const sys::cef_string_t);
            reset_settings(
                &context,
                &native,
                [SiteSetting::Allowed, SiteSetting::Blocked],
                validate,
            )
        };
        for origin in ["", "https://example.test/path", "https://example.test:443"] {
            assert!(run(origin, &|| Ok(())).is_err());
        }
        assert!(writes.borrow().is_empty());
        let calls = Cell::new(0);
        assert!(
            run("https://example.test:8443", &|| {
                calls.set(calls.get() + 1);
                if calls.get() > 2 {
                    Err("changed".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(
            &*writes.borrow(),
            &[(
                "https://example.test:8443".into(),
                ContentSettingTypes::GEOLOCATION.get_raw(),
                ContentSettingValues::DEFAULT.get_raw()
            )]
        );
        let mut units = "https://example.test:8443"
            .encode_utf16()
            .collect::<Vec<_>>();
        let raw = sys::cef_string_t {
            str_: units.as_mut_ptr(),
            length: units.len(),
            dtor: None,
        };
        let native = CefString::from(&raw as *const sys::cef_string_t);
        reset_settings(
            &context,
            &native,
            [SiteSetting::Ask, SiteSetting::Blocked],
            || Ok(()),
        )
        .unwrap();
        assert_eq!(writes.borrow().len(), 2);
        assert_eq!(
            writes.borrow()[1],
            (
                "https://example.test:8443".into(),
                ContentSettingTypes::NOTIFICATIONS.get_raw(),
                ContentSettingValues::DEFAULT.get_raw(),
            )
        );
    }
}
