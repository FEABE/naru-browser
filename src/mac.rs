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
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
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
