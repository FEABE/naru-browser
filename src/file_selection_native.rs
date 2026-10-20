//! One user-opened file selection sheet. No page or browser-space activation.

use crate::file_selection::{MAX_EXTENSIONS, MAX_FILES, MAX_PATH_BYTES, Mode};
use block2::RcBlock;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::NSObjectProtocol,
    sel,
};
use objc2_app_kit::{
    NSButton, NSControlStateValueOn, NSModalResponse, NSModalResponseCancel, NSModalResponseOK,
    NSOpenPanel,
};
use objc2_foundation::{NSArray, NSObject, NSPoint, NSRect, NSSize, NSString};
use std::{cell::RefCell, path::Path};

struct Dialog {
    token: u64,
    mode: Mode,
    panel: Retained<NSOpenPanel>,
    filter: Option<(Retained<FilterTarget>, Retained<NSButton>, Retained<NSArray<NSString>>)>,
    starting: bool,
    canceled: bool,
    completing: bool,
}

thread_local! { static DIALOG: RefCell<Option<Dialog>> = const { RefCell::new(None) }; }

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = u64]
    struct FilterTarget;

    unsafe impl NSObjectProtocol for FilterTarget {}
    impl FilterTarget {
        #[unsafe(method(showAllFiles:))]
        fn show_all_files(&self, sender: &NSButton) {
            let all = sender.state() == NSControlStateValueOn;
            let current = DIALOG.with(|slot| {
                let slot = slot.borrow();
                let dialog = slot.as_ref().filter(|dialog| {
                    dialog.token == *self.ivars() && !dialog.canceled && !dialog.completing
                })?;
                let (_, button, extensions) = dialog.filter.as_ref()?;
                std::ptr::eq(sender, &**button)
                    .then(|| (dialog.panel.clone(), extensions.clone()))
            });
            if let Some((panel, extensions)) = current {
                // NSOpenPanel ignores allowsOtherFileTypes. This explicit
                // accessory keeps HTML accept a user-overridable suggestion.
                #[allow(deprecated)]
                panel.setAllowedFileTypes(if all { None } else { Some(&extensions) });
            }
        }
    }
);

impl FilterTarget {
    fn new(token: u64, marker: MainThreadMarker) -> Retained<Self> {
        let object = Self::alloc(marker).set_ivars(token);
        unsafe { msg_send![super(object), init] }
    }
}

pub fn show(token: u64, mode: Mode, extensions: &[String]) -> Result<(), String> {
    let marker = MainThreadMarker::new()
        .ok_or_else(|| "파일 선택 창은 브라우저 창에서만 열 수 있습니다.".to_owned())?;
    if DIALOG.with(|slot| slot.borrow().is_some()) {
        return Err("열려 있는 파일 선택 창을 먼저 닫아 주세요.".into());
    }
    if extensions.len() > MAX_EXTENSIONS
        || extensions.iter().any(|extension| {
            extension.is_empty()
                || extension.len() > 64
                || !extension.as_bytes()[0].is_ascii_alphanumeric()
                || extension.split('.').any(str::is_empty)
                || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
        })
    {
        return Err("요청한 파일 형식을 선택 창에 표시할 수 없습니다.".into());
    }
    let window = crate::control_panel::file_selection_window()
        .ok_or_else(|| "파일을 선택할 브라우저 창이 없습니다.".to_owned())?;
    if !window.isVisible() || !window.isKeyWindow() || window.isMiniaturized()
        || window.attachedSheet().is_some()
    {
        return Err("브라우저 창의 다른 선택 창을 닫은 뒤 다시 선택해 주세요.".into());
    }
    let panel = NSOpenPanel::openPanel(marker);
    panel.setTitle(Some(&NSString::from_str("파일 선택 요청")));
    panel.setMessage(Some(&NSString::from_str(
        "선택한 파일을 요청 페이지에 전달합니다. 사이트에서 바로 업로드할 수 있습니다.",
    )));
    panel.setPrompt(Some(&NSString::from_str("전달")));
    panel.setCanChooseFiles(mode != Mode::Folder);
    panel.setCanChooseDirectories(mode == Mode::Folder);
    panel.setAllowsMultipleSelection(mode == Mode::Multiple);
    panel.setCanCreateDirectories(false);
    panel.setResolvesAliases(true);
    let filter = if mode != Mode::Folder && !extensions.is_empty() {
        let values: Vec<_> = extensions.iter().map(|value| NSString::from_str(value)).collect();
        let values = NSArray::from_retained_slice(&values);
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&values));
        let target = FilterTarget::new(token, marker);
        let button = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("모든 파일 보기"),
                Some(&target),
                Some(sel!(showAllFiles:)),
                marker,
            )
        };
        button.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(240.0, 28.0)));
        button.setToolTip(Some(&NSString::from_str("사이트가 제안한 형식 이외의 파일도 선택")));
        panel.setAccessoryView(Some(&button));
        Some((target, button, values))
    } else {
        #[allow(deprecated)]
        panel.setAllowedFileTypes(None);
        None
    };
    // Native getters/configuration above may reenter and revoke the request.
    if !window.isVisible() || !window.isKeyWindow() || window.isMiniaturized()
        || window.attachedSheet().is_some()
        || !crate::control_panel::file_selection_window()
            .is_some_and(|current| std::ptr::eq(&*current, &*window))
        || !crate::file_selection_runtime::snapshot()
            .iter().any(|row| row.token == token && row.choosing)
    {
        return Err("파일 선택 요청이 만료되었거나 다른 선택 창이 열렸습니다.".into());
    }
    let stored = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() { return false; }
        *slot = Some(Dialog {
            token, mode, panel: panel.clone(), filter,
            starting: true, canceled: false, completing: false,
        });
        true
    });
    if !stored {
        return Err("열려 있는 파일 선택 창을 먼저 닫아 주세요.".into());
    }
    // Capturing only the token avoids a panel -> block -> panel retain cycle.
    let completion = RcBlock::new(move |response: NSModalResponse| finished(token, response));
    panel.beginSheetModalForWindow_completionHandler(&window, &completion);
    let cancel_after_start = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let dialog = slot.as_mut().filter(|dialog| dialog.token == token)?;
        dialog.starting = false;
        (dialog.canceled && !dialog.completing).then(|| dialog.panel.clone())
    });
    if let Some(panel) = cancel_after_start {
        unsafe { panel.cancel(None); }
    }
    Ok(())
}

pub fn cancel(token: u64) {
    let panel = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let dialog = slot.as_mut().filter(|dialog| dialog.token == token)?;
        dialog.canceled = true;
        (!dialog.starting && !dialog.completing).then(|| dialog.panel.clone())
    });
    if let Some(panel) = panel {
        // Keep the slot until AppKit reports the sheet's completion.
        unsafe { panel.cancel(None); }
    }
}

fn selected_paths(panel: &NSOpenPanel, mode: Mode) -> Result<Vec<String>, String> {
    let urls = panel.URLs();
    let count = urls.count();
    if count == 0 || count > MAX_FILES || (mode != Mode::Multiple && count != 1) {
        return Err("선택한 파일 수를 처리할 수 없습니다.".into());
    }
    let mut paths = Vec::with_capacity(count);
    for url in urls.iter() {
        if !url.isFileURL() {
            return Err("로컬 파일이나 폴더만 선택할 수 있습니다.".into());
        }
        let path = url.path().ok_or_else(|| "선택한 파일 경로를 읽을 수 없습니다.".to_owned())?;
        // Bound the UTF-16 input before allocating its UTF-8 representation.
        if path.length() > MAX_PATH_BYTES {
            return Err("선택한 파일 경로가 너무 깁니다.".into());
        }
        let path = path.to_string();
        if path.len() > MAX_PATH_BYTES || path.contains('\0') || !Path::new(&path).is_absolute() {
            return Err("선택한 파일 경로를 처리할 수 없습니다.".into());
        }
        paths.push(path);
    }
    Ok(paths)
}

fn finished(token: u64, response: NSModalResponse) {
    let current = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let dialog = slot.as_mut().filter(|dialog| dialog.token == token && !dialog.completing)?;
        dialog.completing = true;
        Some((dialog.panel.clone(), dialog.mode, dialog.canceled))
    });
    let Some((panel, mode, canceled)) = current else { return };
    panel.orderOut(None);
    let result = if canceled || response == NSModalResponseCancel {
        Ok(Vec::new())
    } else if response == NSModalResponseOK {
        selected_paths(&panel, mode)
    } else {
        Err("파일 선택 창을 열지 못했습니다.".into())
    };
    let dialog = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|dialog| dialog.token == token) {
            slot.take()
        } else {
            None
        }
    });
    let Some(dialog) = dialog else { return };
    // Cancellation may have reentered during orderOut/URL extraction.
    let result = if dialog.canceled { Ok(Vec::new()) } else { result };
    if let Some((_, button, _)) = &dialog.filter {
        unsafe {
            button.setTarget(None);
            button.setAction(None);
        }
        panel.setAccessoryView(None);
    }
    drop(dialog);
    crate::file_selection_runtime::native_finished(token, result);
}
