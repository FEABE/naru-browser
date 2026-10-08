use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, define_class, extern_methods, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObjectProtocol},
};
use objc2_app_kit::{NSApp, NSApplication, NSApplicationActivationPolicy, NSEvent};
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
            let _: () = unsafe { msg_send![super(self), sendEvent:event] };
            self.ivars().handling_event.set(previous);
        }

        #[unsafe(method(terminate:))]
        unsafe fn terminate(&self, _sender: Option<&AnyObject>) {
            crate::browser::close_all();
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
}
