use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, define_class, extern_methods, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{
    NSApp, NSApplication, NSApplicationActivationPolicy, NSApplicationDidUnhideNotification,
    NSEvent, NSEventType, NSWorkspace,
};
use objc2_foundation::{
    NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSString, NSUserDefaults,
};
use std::cell::Cell;

#[derive(Default)]
pub struct ApplicationIvars {
    handling_event: Cell<Bool>,
}

// CEF requires these protocols for Cocoa event dispatch. Based on cef-rs cefsimple.
define_class!(
    #[unsafe(super(NSApplication))]
    #[ivars = ApplicationIvars]
    pub struct LightBrowserApplication;

    impl LightBrowserApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let previous = self.ivars().handling_event.replace(Bool::YES);
            if !crate::control_panel::handle_shortcut(event) {
                crate::control_panel::observe_page_input(event);
                let focus_trace = crate::control_panel::begin_focus_tab_trace(event);
                if event.r#type() == NSEventType::KeyDown {
                    crate::control_panel::trace_sidebar_geometry("key-before");
                }
                let _: () = unsafe { msg_send![super(self), sendEvent:event] };
                crate::control_panel::correct_hidden_cef_tab_focus(event);
                if event.r#type() == NSEventType::KeyDown {
                    crate::control_panel::trace_sidebar_geometry("key-after");
                }
                if let Some(previous) = focus_trace {
                    crate::control_panel::end_focus_tab_trace(previous);
                }
            }
            self.ivars().handling_event.set(previous);
        }

        #[unsafe(method(sendAction:to:from:))]
        unsafe fn send_action(&self, action: Sel, target: Option<&AnyObject>, sender: Option<&AnyObject>) -> bool {
            // Keep AppKit's original action, validation and responder routing.
            crate::control_panel::observe_page_menu_action(action, sender);
            unsafe { msg_send![super(self), sendAction: action, to: target, from: sender] }
        }

        #[unsafe(method(terminate:))]
        unsafe fn terminate(&self, _sender: Option<&AnyObject>) {
            crate::browser::close_all();
        }

        #[unsafe(method(refreshPanelAfterUnhide:))]
        fn refresh_panel_after_unhide(&self, _notification: &NSNotification) {
            crate::browser::notify_panel();
        }
    }

    unsafe impl CrAppControlProtocol for LightBrowserApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling_send_event(&self, value: Bool) {
            self.ivars().handling_event.set(value);
        }
    }
    unsafe impl CrAppProtocol for LightBrowserApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn is_handling_send_event(&self) -> Bool {
            self.ivars().handling_event.get()
        }
    }
    unsafe impl CefAppProtocol for LightBrowserApplication {}
);

impl LightBrowserApplication {
    extern_methods! {
        #[unsafe(method(sharedApplication))]
        fn shared_application() -> Retained<Self>;
    }
}

pub fn setup() {
    let app = LightBrowserApplication::shared_application();
    let marker = MainThreadMarker::new().expect("App startup must run on the main thread");
    assert!(NSApp(marker).isKindOfClass(LightBrowserApplication::class()));
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // After an unclean exit AppKit's launch handling can run a "reopen
    // windows" crash-history alert as a background modal loop. Chromium's
    // quit then stops that loop instead of the main one, so the app hangs
    // after its last tab closes. Naru restores no window state. Registered
    // defaults are not persisted.
    let key = NSString::from_str("ApplePersistenceIgnoreState");
    let ignore = NSNumber::new_bool(true);
    let ignore: &AnyObject = &ignore;
    unsafe {
        NSUserDefaults::standardUserDefaults()
            .registerDefaults(&NSDictionary::from_slices(&[&*key], &[ignore]));
    }
    // Observe this application's unhide without replacing CEF's app delegate.
    // The singleton application outlives the observer registration.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &app,
            sel!(refreshPanelAfterUnhide:),
            Some(NSApplicationDidUnhideNotification),
            Some(&app),
        );
    }
}

/// Chromium's macOS SIGTERM handler ends the session without closing CEF
/// browsers, so cookies set just before exit are not flushed. Route SIGTERM,
/// SIGINT and SIGHUP to the same browser close as Quit. Call after CEF
/// initialization, which installs Chromium's handlers.
pub fn quit_on_signals() {
    use std::ffi::c_void;
    #[repr(C)]
    struct Opaque([u8; 0]);
    unsafe extern "C" {
        static _dispatch_source_type_signal: Opaque;
        static _dispatch_main_q: Opaque;
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *const Opaque;
        fn dispatch_source_create(
            kind: *const Opaque,
            handle: usize,
            mask: usize,
            queue: *const Opaque,
        ) -> *mut c_void;
        fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
        fn dispatch_source_set_event_handler_f(
            source: *mut c_void,
            handler: extern "C" fn(*mut c_void),
        );
        fn dispatch_resume(object: *mut c_void);
        fn dispatch_async_f(
            queue: *const Opaque,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }
    extern "C" fn close(_: *mut c_void) {
        crate::browser::close_all();
    }
    // Runs on a background queue, outside signal context. An unload prompt or
    // a wedged main thread must not make termination signals ineffective. As
    // with Chromium's handler, a second signal exits immediately.
    extern "C" fn received(signal: *mut c_void) {
        static SIGNALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let status = 128 + signal as i32;
        if SIGNALLED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            unsafe { libc::_exit(status) };
        }
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(10));
            unsafe { libc::_exit(status) };
        });
        unsafe { dispatch_async_f(&raw const _dispatch_main_q, std::ptr::null_mut(), close) };
    }
    // Replaces Chromium's handler. Unlike SIG_IGN, a handler is reset to the
    // default in spawned helpers; the process-lifetime source still observes it.
    extern "C" fn ignore(_: libc::c_int) {}
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        unsafe {
            let source = dispatch_source_create(
                &raw const _dispatch_source_type_signal,
                signal as usize,
                0,
                dispatch_get_global_queue(0, 0),
            );
            assert!(!source.is_null(), "Signal source creation failed");
            dispatch_set_context(source, signal as usize as *mut c_void);
            dispatch_source_set_event_handler_f(source, received);
            dispatch_resume(source);
            libc::signal(
                signal,
                ignore as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
    }
}

/// Request Finder selection, never launch the downloaded file. The native
/// return value is not proof of visible selection or file identity/existence.
pub(crate) fn reveal_file(path: &str) -> Result<(), String> {
    let _main_thread = MainThreadMarker::new()
        .ok_or_else(|| "파일 위치 표시는 브라우저 UI에서 실행해 주세요.".to_owned())?;
    let path = NSString::from_str(path);
    let root = NSString::from_str("");
    let accepted =
        NSWorkspace::sharedWorkspace().selectFile_inFileViewerRootedAtPath(Some(&path), &root);
    if accepted {
        Ok(())
    } else {
        Err("파일 위치를 표시하지 못했습니다. 파일이 이동되거나 삭제됐는지 확인해 주세요.".into())
    }
}
