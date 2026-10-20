//! One native AppKit browser window. Controls never create a web renderer.

use crate::page_recovery::{PageFailure, PageRecoveryTarget};
use crate::protocol::TaskScope;
use crate::start_page::StartPageTarget;
use crate::tab_search::{self, SearchMode, SearchResultId, SearchState, SearchTab};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApp, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBackingStoreType, NSButton, NSCellImagePosition, NSClipView, NSColor, NSEvent,
    NSEventModifierFlags, NSEventTrackingRunLoopMode, NSEventType, NSFont, NSFontWeightMedium,
    NSGestureRecognizer, NSGestureRecognizerState, NSImage, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSImageView, NSMenu, NSMenuDelegate, NSMenuItem, NSScrollView, NSTextField,
    NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSData, NSNotification, NSObject, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes,
    NSSize, NSString, NSTimer,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ffi::c_void,
    rc::Rc,
};

pub struct ShellSnapshot {
    pub tabs: Vec<ShellTab>,
    pub spaces: Vec<crate::spaces::SpaceView>,
    pub active_space: String,
    pub active_tab: Option<i32>,
    pub overview: bool,
}

pub struct ShellTab {
    pub id: i32,
    pub title: String,
    pub url: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    pub audio_muted: Option<bool>,
    pub crashed: bool,
    pub failure: Option<PageFailure>,
    pub start_page: bool,
    pub task_status: String,
    /// A JavaScript dialog is queued for the AI to answer through the CLI.
    pub dialog_pending: bool,
    /// The queued dialog was handed to the user; the native panel answers it.
    pub dialog_user: bool,
    pub stop_scope: Option<TaskScope>,
    pub space: String,
    pub navigation_epoch: u64,
}

pub enum ShellAction {
    Back(i32),
    Forward(i32),
    Navigate(i32, String),
    NewTab,
    CloseTab(i32),
    Stop(i32, TaskScope),
    SelectTab(i32),
    SelectSearchTab(i32),
    SelectSpace(String),
    ShowOverview(bool),
    StopSpace(TaskScope),
    /// Native "AI에게 돌려주기" for a paused task; same path as `task resume`.
    ResumeSpace(TaskScope),
}

const PREVIEW_LIMIT: usize = 8;
const PREVIEW_BYTES_LIMIT: usize = 1024 * 1024;
const SIDEBAR_WIDTH: f64 = 216.0;
const RAIL_WIDTH: f64 = 76.0;
const TOOLBAR_HEIGHT: f64 = 56.0;
const FIND_BAR_HEIGHT: f64 = 44.0;
const PAGE_RECOVERY_HEIGHT: f64 = 96.0;
const PAGE_DIALOG_HEIGHT: f64 = 170.0;
/// Display cap for untrusted dialog text; the original prompt default is kept elsewhere.
const PAGE_DIALOG_MESSAGE_CHARS: usize = 600;
const TAB_ROW_HEIGHT: f64 = 38.0;

struct StartPage {
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    input_surface: Retained<SurfaceView>,
    input: Retained<NSTextField>,
    open_surface: Retained<SurfaceView>,
    open: Retained<NSButton>,
    hint: Retained<NSTextField>,
    bookmarks: Retained<NSButton>,
    target: Option<StartPageTarget>,
    visible: bool,
    submission: Option<StartPageTarget>,
}

fn start_page_target(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
) -> Option<StartPageTarget> {
    let tab = tabs
        .iter()
        .find(|tab| Some(tab.id) == active && tab.space == space)?;
    (tab.start_page
        && tab.url == "about:blank"
        && !tab.loading
        && !tab.crashed
        && tab.failure.is_none())
    .then(|| StartPageTarget {
        tab: tab.id,
        space: tab.space.clone(),
        navigation_epoch: tab.navigation_epoch,
    })
}

struct StartPageFrames {
    title: NSRect,
    subtitle: NSRect,
    input: NSRect,
    open: NSRect,
    hint: NSRect,
    bookmarks: NSRect,
}

fn start_page_frames(size: NSSize) -> StartPageFrames {
    let width = (size.width - 64.0).clamp(240.0, 560.0);
    let left = (size.width - width) / 2.0;
    let top = (size.height * 0.64).max(236.0).min(size.height - 24.0);
    StartPageFrames {
        title: rect(left, top - 36.0, width, 36.0),
        subtitle: rect(left, top - 70.0, width, 24.0),
        input: rect(left, top - 138.0, width - 100.0, 44.0),
        open: rect(left + width - 88.0, top - 138.0, 88.0, 44.0),
        hint: rect(left, top - 166.0, width, 20.0),
        bookmarks: rect(size.width / 2.0 - 70.0, top - 214.0, 140.0, 32.0),
    }
}

impl StartPage {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Window, 0.0, false, marker);
        surface.setHidden(true);
        let title = label(&surface, "어디로 가볼까요?", 28.0, true, marker);
        let subtitle = label(
            &surface,
            "주소나 검색어를 입력해 시작하세요.",
            14.0,
            false,
            marker,
        );
        subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let input_surface = surface_child(&surface, SurfaceKind::Card, 12.0, true, marker);
        let input =
            NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 20.0));
        input.setEditable(true);
        input.setSelectable(true);
        input.setBezeled(false);
        input.setBordered(false);
        input.setDrawsBackground(false);
        input.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        input.setTextColor(Some(&NSColor::labelColor()));
        input.setPlaceholderString(Some(&NSString::from_str("주소 또는 Google 검색")));
        input.setToolTip(Some(&NSString::from_str(
            "새 탭에서 주소 또는 Google 검색 열기",
        )));
        unsafe {
            input.setTarget(Some(&*target));
            input.setAction(Some(sel!(submitStartPage:)));
            let _: () = msg_send![&*input, setAccessibilityLabel: &*NSString::from_str("새 탭 주소 또는 Google 검색")];
        }
        input_surface.addSubview(&input);
        let open_surface = surface_child(&surface, SurfaceKind::Primary, 12.0, false, marker);
        let open = button(
            &surface,
            "열기",
            target,
            sel!(submitStartPage:),
            "입력한 주소 또는 Google 검색 열기",
            marker,
        );
        open.setContentTintColor(Some(&NSColor::whiteColor()));
        open.setFont(Some(&medium_font(13.0)));
        let hint = label(
            &surface,
            "검색어는 Google로 검색합니다.",
            11.0,
            false,
            marker,
        );
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let bookmarks = button(
            &surface,
            "북마크 열기",
            target,
            sel!(openStartPageBookmarks:),
            "새 탭에서 공유 북마크 열기",
            marker,
        );
        set_symbol(
            &bookmarks,
            "bookmark",
            "북마크 열기",
            "북마크 열기",
            "새 탭에서 공유 북마크 열기",
        );
        bookmarks.setImageHugsTitle(true);
        bookmarks.setContentTintColor(Some(&accent_color()));
        for field in [&title, &subtitle, &hint] {
            unsafe {
                let _: () = msg_send![&**field, setAlignment: 1isize];
            }
        }
        Self {
            surface,
            title,
            subtitle,
            input_surface,
            input,
            open_surface,
            open,
            hint,
            bookmarks,
            target: None,
            visible: false,
            submission: None,
        }
    }

    fn render(&mut self, target: Option<StartPageTarget>, visible: bool) {
        // Only one document owns this field. Background events and overlays
        // never rewrite a matching draft; a new navigation epoch always resets it.
        if self.target != target {
            self.input.setStringValue(&NSString::from_str(""));
            self.hint
                .setStringValue(&NSString::from_str("검색어는 Google로 검색합니다."));
            self.hint.setToolTip(None);
            self.hint
                .setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        self.target = target;
        self.visible = visible && self.target.is_some();
        self.input.setEnabled(self.visible);
        self.open.setEnabled(self.visible);
        self.bookmarks.setEnabled(self.visible);
    }

    fn layout(&self, size: NSSize) {
        let frames = start_page_frames(size);
        self.title.setFrame(frames.title);
        self.subtitle.setFrame(frames.subtitle);
        self.input_surface.setFrame(frames.input);
        self.input
            .setFrame(rect(14.0, 12.0, frames.input.size.width - 28.0, 20.0));
        self.open_surface.setFrame(frames.open);
        self.open.setFrame(frames.open);
        self.hint.setFrame(frames.hint);
        self.bookmarks.setFrame(frames.bookmarks);
    }
}

#[derive(Default)]
struct PageFindState {
    open: bool,
    target: Option<(i32, u64)>,
    query: String,
    sequence: u64,
    token: Option<u64>,
    count: Option<i32>,
    ordinal: Option<i32>,
    final_update: bool,
    notice: Option<String>,
}

impl PageFindState {
    fn close(&mut self, remember: bool) -> Option<(i32, u64)> {
        let stop = self
            .target
            .zip(self.token.take())
            .map(|((tab, _), token)| (tab, token));
        self.open = false;
        self.count = None;
        self.ordinal = None;
        self.final_update = false;
        self.notice = None;
        if !remember {
            self.target = None;
            self.query.clear();
        }
        stop
    }

    fn request(&mut self) -> Option<u64> {
        let token = self.sequence.checked_add(1)?;
        self.sequence = token;
        self.token = Some(token);
        self.count = None;
        self.ordinal = None;
        self.final_update = false;
        self.notice = None;
        Some(token)
    }

    fn receive(
        &mut self,
        tab: i32,
        epoch: u64,
        token: u64,
        count: i32,
        ordinal: i32,
        final_update: bool,
    ) -> bool {
        if !self.open || self.target != Some((tab, epoch)) || self.token != Some(token) {
            return false;
        }
        // CEF can report -1 while this part of an incremental result is unknown.
        if count >= 0 {
            self.count = Some(count);
        }
        if ordinal >= 0 {
            self.ordinal = Some(ordinal);
        }
        self.final_update = final_update;
        true
    }
}

struct FindBar {
    state: PageFindState,
    surface: Retained<SurfaceView>,
    input_surface: Retained<SurfaceView>,
    field: Retained<NSTextField>,
    result: Retained<NSTextField>,
    previous: Retained<NSButton>,
    next: Retained<NSButton>,
    close: Retained<NSButton>,
}

struct PageRecoveryBar {
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    edit_address: Retained<NSButton>,
    retry: Retained<NSButton>,
    target: Option<PageRecoveryTarget>,
    loading: bool,
}

/// Native band for a JavaScript dialog handed to the user. It exists only while
/// the dialog's tab is the shown tab of the shown space.
struct PageDialogBar {
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    message: Retained<NSTextField>,
    field: Retained<NSTextField>,
    cancel: Retained<NSButton>,
    primary: Retained<NSButton>,
    shown: Option<crate::dialog_runtime::UserDialog>,
    /// Set when a new dialog appeared; the next layout focuses its control once.
    focus_pending: bool,
    /// The dialog that already received its one-time focus.
    focused_token: Option<u64>,
    /// Typed prompt text kept while the strip is hidden by an overlay or tab switch.
    draft: Option<(u64, String)>,
}

/// What `focus_tab` must do for the dialog strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogFocus {
    /// No strip for this tab: the page may take focus.
    Page,
    /// The strip already holds focus: leave it.
    Keep,
    /// Move focus into the strip.
    Take,
}

fn dialog_focus_decision(
    strip_tab: Option<i32>,
    requested: i32,
    responder_in_strip: bool,
) -> DialogFocus {
    match strip_tab {
        Some(tab) if tab == requested => {
            if responder_in_strip {
                DialogFocus::Keep
            } else {
                DialogFocus::Take
            }
        }
        _ => DialogFocus::Page,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogKey {
    Primary,
    Cancel,
}

/// Return/Enter answer the visible dialog strip and Esc cancels it, only while
/// focus is inside the strip or that tab's page, never in the address bar or
/// another native field, and never while an IME composition is active.
fn dialog_key_decision(
    key_code: u16,
    unmodified: bool,
    strip_visible: bool,
    responder_in_scope: bool,
    marked_text: bool,
    cancel_enabled: bool,
) -> Option<DialogKey> {
    if !strip_visible || !unmodified || !responder_in_scope || marked_text {
        return None;
    }
    match key_code {
        36 | 76 => Some(DialogKey::Primary),
        53 if cancel_enabled => Some(DialogKey::Cancel),
        _ => None,
    }
}

/// The tab whose user-owned dialog should be presented right now.
fn visible_user_dialog_tab(
    tabs: &[ShellTab],
    active_space: &str,
    active: Option<i32>,
    overview: bool,
    overlay_open: bool,
) -> Option<i32> {
    if overview || overlay_open {
        return None;
    }
    tabs.iter()
        .find(|tab| Some(tab.id) == active && tab.space == active_space && tab.dialog_user)
        .map(|tab| tab.id)
}

fn dialog_button_titles(kind: crate::dialogs::Kind) -> (&'static str, Option<&'static str>) {
    use crate::dialogs::Kind;
    match kind {
        Kind::Alert => ("확인", None),
        Kind::Confirm | Kind::Prompt => ("확인", Some("취소")),
        Kind::BeforeUnload => ("떠나기", Some("머무르기")),
    }
}

impl PageDialogBar {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Card, 0.0, true, marker);
        surface.setHidden(true);
        surface.set_focusable(true);
        let title = label(&surface, "", 14.0, true, marker);
        let message = label(&surface, "", 12.0, false, marker);
        unsafe {
            let _: () = msg_send![&*message, setUsesSingleLineMode: false];
            let _: () = msg_send![&*message, setLineBreakMode: 4isize];
            let _: () = msg_send![&*message, setMaximumNumberOfLines: 4isize];
        }
        message.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let field =
            NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 24.0));
        field.setEditable(true);
        field.setSelectable(true);
        field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        field.setHidden(true);
        unsafe {
            field.setTarget(Some(target));
            field.setAction(Some(sel!(answerPageDialog:)));
        }
        surface.addSubview(&field);
        let cancel = button(
            &surface,
            "취소",
            target,
            sel!(cancelPageDialog:),
            "페이지 대화상자 취소 (Esc)",
            marker,
        );
        let primary = button(
            &surface,
            "확인",
            target,
            sel!(answerPageDialog:),
            "페이지 대화상자 확인 (Return)",
            marker,
        );
        primary.setContentTintColor(Some(&accent_color()));
        cancel.setEnabled(false);
        primary.setEnabled(false);
        Self {
            surface,
            title,
            message,
            field,
            cancel,
            primary,
            shown: None,
            focus_pending: false,
            focused_token: None,
            draft: None,
        }
    }

    fn render(&mut self, dialog: Option<crate::dialog_runtime::UserDialog>) {
        let Some(dialog) = dialog else {
            if let Some(previous) = self.shown.take() {
                // Hidden by an overlay or a tab switch: keep what the user typed.
                if previous.kind == crate::dialogs::Kind::Prompt {
                    self.draft = Some((previous.token, self.field.stringValue().to_string()));
                }
                self.focus_pending = false;
                self.primary.setEnabled(false);
                self.cancel.setEnabled(false);
                self.field.setHidden(true);
            }
            return;
        };
        if self.shown.as_ref().map(|shown| shown.token) != Some(dialog.token) {
            let (primary, cancel) = dialog_button_titles(dialog.kind);
            let origin = if dialog.origin.is_empty() {
                "알 수 없는 출처".to_owned()
            } else {
                compact(&dialog.origin, 120)
            };
            self.title.setStringValue(&NSString::from_str(&format!(
                "이 페이지의 {} 대화상자 · {origin}",
                dialog.kind.label()
            )));
            dialog_message_label(&self.message, &dialog.message);
            let prompt = dialog.kind == crate::dialogs::Kind::Prompt;
            self.field.setHidden(!prompt);
            let text = match self.draft.take() {
                Some((token, text)) if token == dialog.token => text,
                _ if prompt => dialog.default_prompt.clone(),
                _ => String::new(),
            };
            self.field.setStringValue(&NSString::from_str(&text));
            self.primary.setTitle(&NSString::from_str(primary));
            self.cancel
                .setTitle(&NSString::from_str(cancel.unwrap_or("")));
            self.cancel.setHidden(cancel.is_none());
            // Re-showing the same dialog never takes focus a second time.
            self.focus_pending = self.focused_token != Some(dialog.token);
            self.focused_token = Some(dialog.token);
        }
        let (_, cancel) = dialog_button_titles(dialog.kind);
        self.primary.setEnabled(true);
        self.cancel.setEnabled(cancel.is_some());
        self.shown = Some(dialog);
    }

    fn layout(&self, width: f64) {
        let text_width = (width - 32.0).max(1.0);
        self.title.setFrame(rect(16.0, 140.0, text_width, 20.0));
        self.message.setFrame(rect(16.0, 76.0, text_width, 60.0));
        self.field.setFrame(rect(16.0, 46.0, text_width, 24.0));
        self.primary
            .setFrame(rect((width - 16.0 - 96.0).max(10.0), 6.0, 96.0, 28.0));
        self.cancel.setFrame(rect(
            (width - 16.0 - 96.0 - 8.0 - 96.0).max(10.0),
            6.0,
            96.0,
            28.0,
        ));
    }
}

fn visible_page_recovery_target(
    tabs: &[ShellTab],
    active_space: &str,
    active: Option<i32>,
    overview: bool,
    overlay_open: bool,
) -> Option<PageRecoveryTarget> {
    if overview || overlay_open {
        return None;
    }
    let tab = tabs
        .iter()
        .find(|tab| Some(tab.id) == active && tab.space == active_space)?;
    Some(PageRecoveryTarget {
        tab: tab.id,
        navigation_epoch: tab.navigation_epoch,
        space: tab.space.clone(),
        failure: tab.failure?,
    })
}

struct PageVerticalLayout {
    page_height: f64,
    find_y: f64,
    recovery_y: f64,
    dialog_y: f64,
}

fn page_vertical_layout(
    height: f64,
    find: bool,
    recovery: bool,
    dialog: bool,
) -> PageVerticalLayout {
    let toolbar_bottom = (height - TOOLBAR_HEIGHT).max(1.0);
    let find_height = if find { FIND_BAR_HEIGHT } else { 0.0 };
    let recovery_height = if recovery { PAGE_RECOVERY_HEIGHT } else { 0.0 };
    let dialog_height = if dialog { PAGE_DIALOG_HEIGHT } else { 0.0 };
    let page_height = (toolbar_bottom - find_height - recovery_height - dialog_height).max(1.0);
    PageVerticalLayout {
        page_height,
        dialog_y: page_height,
        recovery_y: page_height + dialog_height,
        find_y: page_height + dialog_height + recovery_height,
    }
}

impl PageRecoveryBar {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Card, 0.0, true, marker);
        surface.setHidden(true);
        let title = label(&surface, "", 14.0, true, marker);
        let detail = label(&surface, "", 12.0, false, marker);
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let edit_address = button(
            &surface,
            "주소 수정",
            target,
            sel!(editFailedPageAddress:),
            "실패한 페이지 주소 수정",
            marker,
        );
        let retry = button(
            &surface,
            "새로 고침",
            target,
            sel!(retryFailedPage:),
            "실패한 페이지 새로 고침",
            marker,
        );
        retry.setContentTintColor(Some(&accent_color()));
        edit_address.setEnabled(false);
        retry.setEnabled(false);
        Self {
            surface,
            title,
            detail,
            edit_address,
            retry,
            target: None,
            loading: false,
        }
    }

    fn render(&mut self, target: Option<PageRecoveryTarget>, loading: bool) {
        if self.target != target || self.loading != loading {
            if let Some(target) = &target {
                let (title, detail) = match target.failure {
                    PageFailure::Load { .. } => (
                        "페이지를 열지 못했습니다",
                        "주소와 연결 상태를 확인하고, 새로 고침으로 다시 불러오세요.",
                    ),
                    PageFailure::Renderer => (
                        "페이지가 종료됐습니다",
                        if loading {
                            "진행 중인 로딩이 끝나면 새로 고침할 수 있습니다."
                        } else {
                            "새로 고침을 누르면 페이지를 다시 불러옵니다."
                        },
                    ),
                };
                self.title.setStringValue(&NSString::from_str(title));
                self.detail.setStringValue(&NSString::from_str(detail));
                self.detail.setToolTip(Some(&NSString::from_str(detail)));
            }
        }
        self.edit_address.setEnabled(target.is_some());
        self.retry.setEnabled(target.is_some() && !loading);
        self.target = target;
        self.loading = loading;
    }

    fn layout(&self, width: f64) {
        let text_width = (width - 32.0).max(1.0);
        self.title.setFrame(rect(16.0, 67.0, text_width, 20.0));
        self.detail.setFrame(rect(16.0, 44.0, text_width, 18.0));
        self.edit_address.setFrame(rect(10.0, 8.0, 86.0, 28.0));
        self.retry.setFrame(rect(104.0, 8.0, 86.0, 28.0));
    }
}

struct SpaceCard {
    space: String,
    tab: Option<i32>,
    url: String,
    navigation_epoch: u64,
    container: Retained<SurfaceView>,
    preview_surface: Retained<SurfaceView>,
    image: Retained<NSImageView>,
    placeholder: Retained<NSTextField>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    badge: Retained<SurfaceView>,
    badge_label: Retained<NSTextField>,
    open: Retained<NSButton>,
    preview_button: Retained<NSButton>,
    stop: Retained<NSButton>,
    preview: Option<Retained<NSImage>>,
}

struct TabRow {
    id: i32,
    space: String,
    button: Retained<NSButton>,
    drag: Retained<TabDragRecognizer>,
    mute: Retained<NSButton>,
    close: Retained<NSButton>,
    icon: Retained<NSImageView>,
    title: Retained<NSTextField>,
}

struct TabDragSession {
    target: crate::browser::TabDragTarget,
    recognizer: Retained<TabDragRecognizer>,
    button: Retained<NSButton>,
    window: Retained<NSWindow>,
    scroll: Retained<NSScrollView>,
    clip: Retained<NSClipView>,
    document: Retained<TopDownView>,
    indicator: Retained<SurfaceView>,
    geometry: TabDragGeometry,
    viewport: Cell<NSRect>,
    clip_bounds: Cell<NSRect>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TabDragGeometry {
    window: NSRect,
    scroll: NSRect,
    document: NSRect,
    viewport: NSRect,
    source: NSRect,
    clip_frame: NSRect,
    clip_bounds: NSRect,
}

#[derive(Default)]
struct TabDragTracking {
    origin: Cell<Option<NSPoint>>,
    location: Cell<NSPoint>,
    epoch: Cell<u64>,
}

fn tab_drag_threshold(origin: NSPoint, current: NSPoint) -> bool {
    let dx = current.x - origin.x;
    let dy = current.y - origin.y;
    dx.is_finite() && dy.is_finite() && dx * dx + dy * dy >= 36.0
}

define_class!(
    #[unsafe(super(NSGestureRecognizer))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TabDragTracking]
    struct TabDragRecognizer;

    unsafe impl NSObjectProtocol for TabDragRecognizer {}
    impl TabDragRecognizer {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().epoch.set(TAB_DRAG_EPOCH.with(Cell::get));
            unsafe { let _: () = msg_send![super(self), mouseDown: event]; }
            let point = event.locationInWindow();
            self.ivars().origin.set(Some(point));
            self.ivars().location.set(point);
        }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            unsafe { let _: () = msg_send![super(self), mouseDragged: event]; }
            let point = event.locationInWindow();
            self.ivars().location.set(point);
            match self.state() {
                NSGestureRecognizerState::Possible => {
                    if self.ivars().origin.get().is_some_and(|origin| tab_drag_threshold(origin, point)) {
                        self.setState(NSGestureRecognizerState::Began);
                    }
                }
                NSGestureRecognizerState::Began | NSGestureRecognizerState::Changed => {
                    self.setState(NSGestureRecognizerState::Changed);
                }
                _ => {},
            }
        }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            unsafe { let _: () = msg_send![super(self), mouseUp: event]; }
            self.ivars().location.set(event.locationInWindow());
            match self.state() {
                // Failure releases the original mouse sequence to NSButton.
                // No manually synthesized click replaces AppKit tracking.
                NSGestureRecognizerState::Possible => self.setState(NSGestureRecognizerState::Failed),
                NSGestureRecognizerState::Began | NSGestureRecognizerState::Changed => {
                    self.setState(NSGestureRecognizerState::Ended);
                }
                _ => {},
            }
        }
        #[unsafe(method(locationInView:))]
        fn location_in_view(&self, view: Option<&NSView>) -> NSPoint {
            let point = self.ivars().location.get();
            view.map_or(point, |view| view.convertPoint_fromView(point, None))
        }
        #[unsafe(method(reset))]
        fn reset(&self) {
            unsafe { let _: () = msg_send![super(self), reset]; }
            self.ivars().origin.set(None);
        }
    }
);

impl TabDragRecognizer {
    fn new(target: &ShellTarget, marker: MainThreadMarker) -> Retained<Self> {
        let value = Self::alloc(marker).set_ivars(TabDragTracking::default());
        unsafe { msg_send![super(value), initWithTarget: target, action: sel!(dragTab:)] }
    }
}

#[derive(Clone)]
struct TabAudioMenu {
    item: Retained<NSMenuItem>,
    target: crate::browser::TabAudioTarget,
    expected: bool,
}

#[derive(Clone)]
struct TabOrderMenu {
    item: Retained<NSMenuItem>,
    target: crate::browser::TabOrderTarget,
}

#[derive(Clone)]
struct NavigationControlMenu {
    item: Retained<NSMenuItem>,
    target: crate::browser::NavigationControlTarget,
}

struct NavigationControlCall {
    target: crate::browser::NavigationControlTarget,
    url: String,
    window: Retained<NSWindow>,
    address: Retained<NSTextField>,
    reload: Retained<NSButton>,
    preserve_address: bool,
}

struct NavigationControlCleanup(Rc<NavigationControlCall>);

impl Drop for NavigationControlCleanup {
    fn drop(&mut self) {
        let removed = NAVIGATION_CONTROL_CALL.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|call| Rc::ptr_eq(call, &self.0)) {
                slot.take()
            } else {
                None
            }
        });
        drop(removed);
    }
}

fn tab_rows_signature(
    space: &str,
    active: Option<i32>,
    collapsed: bool,
    overview: bool,
    overlay_open: bool,
    visible: &[&ShellTab],
) -> String {
    format!(
        "{}:{:?}:{}:{}:{}:{:?}",
        space,
        active,
        collapsed,
        overview,
        overlay_open,
        visible
            .iter()
            .map(|tab| (
                tab.id,
                &tab.title,
                &tab.url,
                tab.loading,
                tab.audio_muted,
                tab.crashed,
                tab.failure,
                &tab.task_status,
                tab.dialog_pending,
                tab.dialog_user,
                tab.stop_scope.is_some()
            ))
            .collect::<Vec<_>>()
    )
}

fn tab_status_symbol(
    failure: Option<PageFailure>,
    muted: Option<bool>,
    ai_active: bool,
    loading: bool,
) -> &'static str {
    if failure.is_some() {
        "exclamationmark.triangle"
    } else if muted == Some(true) {
        "speaker.slash"
    } else if ai_active {
        "sparkles"
    } else if loading {
        "arrow.triangle.2.circlepath"
    } else {
        "globe"
    }
}

fn tab_status_description(tab: &ShellTab) -> String {
    format!(
        "{}{}{}{}{}",
        tab.task_status,
        if tab.dialog_user {
            ", 대화상자 사용자 응답 대기"
        } else if tab.dialog_pending {
            ", 대화상자 응답 대기"
        } else {
            ""
        },
        match tab.failure {
            Some(PageFailure::Load { .. }) => ", 페이지를 열지 못했습니다",
            Some(PageFailure::Renderer) => ", 페이지가 종료됐습니다",
            None => "",
        },
        if tab.loading {
            ", 불러오는 중"
        } else {
            ""
        },
        if tab.audio_muted == Some(true) {
            ", 음소거 설정됨"
        } else {
            ""
        },
    )
}

struct TabRowFrames {
    selection: NSRect,
    mute: NSRect,
    close: NSRect,
    icon: NSRect,
    title: NSRect,
    background: NSRect,
}

struct ToolbarFrames {
    address: NSRect,
    status: NSRect,
    stop: NSRect,
    close: NSRect,
}

fn toolbar_frames(width: f64, show_stop: bool) -> ToolbarFrames {
    let status_width = if width < 650.0 { 96.0 } else { 110.0 };
    let stop_width = if show_stop { 68.0 } else { 0.0 };
    let status_x = width - 48.0 - status_width - stop_width;
    ToolbarFrames {
        address: rect(120.0, 10.0, (status_x - 132.0).max(100.0), 36.0),
        status: rect(status_x, 16.0, status_width, 24.0),
        stop: rect(status_x + status_width + 8.0, 12.0, 60.0, 32.0),
        close: rect(width - 40.0, 12.0, 32.0, 32.0),
    }
}

fn tab_row_frames(width: f64, collapsed: bool, y: f64) -> TabRowFrames {
    let available = (width - 4.0).max(0.0);
    let controls = if collapsed { 1.0 } else { 2.0 };
    let gap = (available / controls).min(if collapsed { 2.0 } else { 4.0 });
    let close_width =
        ((available - gap * controls) / (controls + 1.0)).min(if collapsed { 22.0 } else { 24.0 });
    let mute_width = if collapsed { 0.0 } else { close_width };
    let selection_width = available - gap * controls - close_width - mute_width;
    let icon_width = selection_width.min(16.0);
    TabRowFrames {
        selection: rect(2.0, y, selection_width, 36.0),
        mute: rect(2.0 + selection_width + gap, y + 2.0, mute_width, 32.0),
        close: rect(2.0 + available - close_width, y + 2.0, close_width, 32.0),
        icon: rect(
            if collapsed {
                2.0 + (selection_width - icon_width) / 2.0
            } else {
                14.0
            },
            y + 10.0,
            icon_width,
            16.0,
        ),
        title: rect(40.0, y + 8.0, (selection_width - 46.0).max(0.0), 20.0),
        background: rect(2.0, y, available, 36.0),
    }
}

fn tab_row_is_current(
    id: i32,
    row_space: &str,
    active_space: &str,
    overlay_open: bool,
    tabs: &[ShellTab],
) -> bool {
    !overlay_open
        && row_space == active_space
        && tabs
            .iter()
            .any(|tab| tab.id == id && tab.space == row_space)
}

struct SearchRow {
    id: SearchResultId,
    button: Retained<NSButton>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchQueryTarget {
    revision: u64,
    query: String,
    selected: Option<SearchResultId>,
}

fn search_query_target(state: &SearchState, revision: u64) -> Option<SearchQueryTarget> {
    if state.revision != revision || state.query_destination().ok().flatten().is_none() {
        return None;
    }
    Some(SearchQueryTarget {
        revision,
        query: state.query.clone(),
        selected: state.selected,
    })
}

fn search_query_target_matches(state: &SearchState, target: &SearchQueryTarget) -> bool {
    state.open
        && state.mode == SearchMode::All
        && !state.confirm_clear
        && !state.query_overflow
        && state.revision != u64::MAX
        && state.revision == target.revision
        && state.query == target.query
        && state.selected == target.selected
}

fn select_query_submission(
    state: &mut SearchState,
    mut target: SearchQueryTarget,
) -> Option<SearchQueryTarget> {
    if !search_query_target_matches(state, &target) {
        return None;
    }
    // The explicit click selects this action before creating a tab. Staging
    // may replace an exact-URL History match with a Tab result during sync.
    let selected = Some(SearchResultId::Query(target.revision));
    state.selected = selected;
    target.selected = selected;
    Some(target)
}

struct AddressDraft {
    tab: i32,
    url: String,
    navigation_epoch: u64,
}

struct SidebarAddressHandoff {
    tab: i32,
    space: String,
    url: String,
    epoch: u64,
}

impl SidebarAddressHandoff {
    fn rebase(&self, draft: &mut AddressDraft, snapshot: &ShellSnapshot) -> bool {
        let Some(epoch) = self.epoch.checked_add(1) else {
            return false;
        };
        if snapshot.overview
            || snapshot.active_tab != Some(self.tab)
            || snapshot.active_space != self.space
            || draft.tab != self.tab
            || draft.url != self.url
            || draft.navigation_epoch != self.epoch
        {
            return false;
        }
        let matches_handoff = snapshot.tabs.iter().any(|tab| {
            tab.id == self.tab
                && tab.space == self.space
                && tab.url == self.url
                && tab.navigation_epoch == epoch
                && tab.stop_scope.is_none()
                && !tab.crashed
        });
        if matches_handoff {
            draft.navigation_epoch = epoch;
        }
        matches_handoff
    }
}

impl AddressDraft {
    fn superseded(&self, tab: i32, url: &str, navigation_epoch: u64, editing: bool) -> bool {
        !editing
            && self.tab == tab
            && (self.url != url || self.navigation_epoch != navigation_epoch)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BookmarkOperation {
    Prepare,
    Save,
    Remove,
}

#[derive(Default)]
struct BookmarkState {
    open: bool,
    query: String,
    selected: Option<u64>,
    request: u64,
    pending: Option<(u64, BookmarkOperation)>,
    draft: Option<crate::bookmark_runtime::Draft>,
    notice: Option<String>,
}

impl BookmarkState {
    fn begin(&mut self, operation: BookmarkOperation) -> Option<u64> {
        let request = self.request.checked_add(1)?;
        self.request = request;
        self.pending = Some((request, operation));
        self.notice = None;
        Some(request)
    }

    fn take_completion(&mut self, request: u64) -> Option<BookmarkOperation> {
        if !self.open || self.pending.map(|pending| pending.0) != Some(request) {
            return None;
        }
        self.pending.take().map(|pending| pending.1)
    }

    fn dismiss_editor(&mut self) {
        self.pending = None;
        self.draft = None;
    }
}

struct BookmarkRow {
    item: crate::bookmarks::Bookmark,
    open: Retained<NSButton>,
    rename: Retained<NSButton>,
    remove: Retained<NSButton>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
}

struct BookmarkPanel {
    state: BookmarkState,
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    hint: Retained<NSTextField>,
    query: Retained<NSTextField>,
    close: Retained<NSButton>,
    add: Retained<NSButton>,
    editor: Retained<SurfaceView>,
    name: Retained<NSTextField>,
    url: Retained<NSTextField>,
    save: Retained<NSButton>,
    cancel: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    document: Retained<TopDownView>,
    selection: Retained<SurfaceView>,
    rows: Vec<BookmarkRow>,
    signature: String,
    writable: bool,
    busy: bool,
}

struct DownloadRow {
    id: u32,
    can_reveal: bool,
    can_reveal_file: bool,
    can_restart: bool,
    surface: Retained<SurfaceView>,
    name: Retained<NSTextField>,
    source: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    choose: Retained<NSButton>,
    cancel: Retained<NSButton>,
    reveal: Retained<NSButton>,
    reveal_file: Retained<NSButton>,
    restart: Retained<NSButton>,
}

struct DownloadPermissionRow {
    token: u64,
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    origin: Retained<NSTextField>,
    source: Retained<NSTextField>,
    scope_hint: Retained<NSTextField>,
    save_hint: Retained<NSTextField>,
    allow: Retained<NSButton>,
    cancel: Retained<NSButton>,
    reveal: Retained<NSButton>,
}

struct FileSelectionRow {
    token: u64,
    choosing: bool,
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    source: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    delivery_hint: Retained<NSTextField>,
    upload_hint: Retained<NSTextField>,
    choose: Retained<NSButton>,
    cancel: Retained<NSButton>,
}

struct DownloadSiteCard {
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    origin: Retained<NSTextField>,
    status: Retained<NSTextField>,
    scope_hint: Retained<NSTextField>,
    reopen_hint: Retained<NSTextField>,
    pending_hint: Retained<NSTextField>,
    files_hint: Retained<NSTextField>,
    reset: Retained<NSButton>,
    current: Option<(
        crate::browser::CurrentPageTarget,
        crate::download_permission_runtime::SiteSnapshot,
    )>,
}

type DownloadSiteView = (
    crate::browser::CurrentPageTarget,
    Result<crate::download_permission_runtime::SiteSnapshot, String>,
);

enum DownloadSiteQuery {
    Skipped,
    Ready(Option<DownloadSiteView>),
}

impl DownloadSiteQuery {
    fn for_target(
        self,
        expected: Option<&crate::browser::CurrentPageTarget>,
        shown: Option<&crate::browser::CurrentPageTarget>,
    ) -> Self {
        match self {
            Self::Skipped if expected == shown => Self::Skipped,
            Self::Skipped => Self::Ready(None),
            Self::Ready(value) => Self::Ready(value.filter(|(page, _)| Some(page) == expected)),
        }
    }
}

struct DownloadPanel {
    open: bool,
    notice: Option<String>,
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    hint: Retained<NSTextField>,
    close: Retained<NSButton>,
    clear: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    document: Retained<TopDownView>,
    site: DownloadSiteCard,
    file_selections: Vec<FileSelectionRow>,
    permissions: Vec<DownloadPermissionRow>,
    rows: Vec<DownloadRow>,
}

struct SitePermissionRow {
    token: u64,
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    origin: Retained<NSTextField>,
    source: Retained<NSTextField>,
    scope_hint: Retained<NSTextField>,
    allow: Retained<NSButton>,
    cancel: Retained<NSButton>,
    reveal: Retained<NSButton>,
}

type SitePermissionSiteView = (
    crate::browser::CurrentPageTarget,
    Result<crate::site_permission_runtime::SiteSnapshot, String>,
);

enum SitePermissionSiteQuery {
    Skipped,
    Ready(Option<SitePermissionSiteView>),
}

impl SitePermissionSiteQuery {
    fn for_target(
        self,
        expected: Option<&crate::browser::CurrentPageTarget>,
        shown: Option<&crate::browser::CurrentPageTarget>,
    ) -> Self {
        match self {
            Self::Skipped if expected == shown => Self::Skipped,
            Self::Skipped => Self::Ready(None),
            Self::Ready(value) => Self::Ready(value.filter(|(page, _)| Some(page) == expected)),
        }
    }
}

struct SitePermissionPanel {
    open: bool,
    notice: Option<String>,
    surface: Retained<SurfaceView>,
    title: Retained<NSTextField>,
    hint: Retained<NSTextField>,
    close: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    document: Retained<TopDownView>,
    site_surface: Retained<SurfaceView>,
    site_title: Retained<NSTextField>,
    site_origin: Retained<NSTextField>,
    site_status: Retained<NSTextField>,
    site_scope: Retained<NSTextField>,
    site_hint: Retained<NSTextField>,
    reset: Retained<NSButton>,
    current: Option<(
        crate::browser::CurrentPageTarget,
        crate::site_permission_runtime::SiteSnapshot,
    )>,
    rows: Vec<SitePermissionRow>,
}

struct Shell {
    window: Retained<NSWindow>,
    root: Retained<SurfaceView>,
    sidebar: Retained<SurfaceView>,
    tab_scroll: Retained<NSScrollView>,
    tab_document: Retained<TopDownView>,
    toolbar: Retained<SurfaceView>,
    content: Retained<NSView>,
    overview_scroll: Retained<NSScrollView>,
    overview_document: Retained<TopDownView>,
    overview_title: Retained<NSTextField>,
    overview_hint: Retained<NSTextField>,
    overview_count: Retained<NSTextField>,
    overview_count_surface: Retained<SurfaceView>,
    overview_previous: Retained<NSButton>,
    overview_next: Retained<NSButton>,
    overview_page_label: Retained<NSTextField>,
    spaces_button: Retained<NSButton>,
    spaces_background: Retained<SurfaceView>,
    space_label: Retained<NSTextField>,
    brand: Retained<NSTextField>,
    brand_icon: Retained<NSImageView>,
    sidebar_toggle: Retained<NSButton>,
    search_button: Retained<NSButton>,
    search_button_background: Retained<SurfaceView>,
    sidebar_separator: Retained<SurfaceView>,
    bookmarks_button: Retained<NSButton>,
    bookmarks: BookmarkPanel,
    downloads_button: Retained<NSButton>,
    downloads: DownloadPanel,
    site_permissions_button: Retained<NSButton>,
    site_permissions: SitePermissionPanel,
    site_permission_count: usize,
    site_permission_notice: Option<String>,
    site_permission_media: Option<String>,
    download_activity: (usize, usize),
    download_permission_count: usize,
    file_selection_count: usize,
    file_selection_notice: Option<String>,
    find: FindBar,
    recovery: PageRecoveryBar,
    dialog: PageDialogBar,
    start_page: StartPage,
    search_surface: Retained<SurfaceView>,
    search_title: Retained<NSTextField>,
    search_hint: Retained<NSTextField>,
    search_field: Retained<NSTextField>,
    search_close: Retained<NSButton>,
    search_mode: Retained<NSButton>,
    history_clear: Retained<NSButton>,
    history_delete: Retained<NSButton>,
    history_cancel: Retained<NSButton>,
    history_clear_available: bool,
    search_scroll: Retained<NSScrollView>,
    search_document: Retained<TopDownView>,
    search_selection: Retained<SurfaceView>,
    search_rows: Vec<SearchRow>,
    search_results: Vec<SearchResultId>,
    search: SearchState,
    search_signature: String,
    back: Retained<NSButton>,
    forward: Retained<NSButton>,
    reload: Retained<NSButton>,
    address: Retained<NSTextField>,
    address_surface: Retained<SurfaceView>,
    address_icon: Retained<NSImageView>,
    close_tab: Retained<NSButton>,
    new_tab: Retained<NSButton>,
    new_tab_background: Retained<SurfaceView>,
    status: Retained<NSTextField>,
    status_surface: Retained<SurfaceView>,
    stop: Retained<NSButton>,
    tab_buttons: Vec<TabRow>,
    selected_tab_background: Retained<SurfaceView>,
    tab_drop_indicator: Retained<SurfaceView>,
    cards: Vec<SpaceCard>,
    views: BTreeMap<i32, Retained<NSView>>,
    tabs: Vec<ShellTab>,
    spaces: Vec<crate::spaces::SpaceView>,
    active_space: String,
    active: Option<i32>,
    overview: bool,
    overview_page: usize,
    sidebar_collapsed: bool,
    reveal_active_tab: bool,
    tab_signature: String,
    tab_focus_removed: bool,
    card_signature: String,
    displayed_address: Option<i32>,
    address_draft: Option<AddressDraft>,
    sidebar_address_handoff: Option<SidebarAddressHandoff>,
    notice: Option<String>,
    // AppKit target/delegate properties are weak. Drop this last.
    target: Retained<ShellTarget>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    // This fixed symbol vocabulary is shared by controls. It never grows with tabs/tasks.
    static SYMBOLS: RefCell<BTreeMap<&'static str, Retained<NSImage>>> = const { RefCell::new(BTreeMap::new()) };
    static FOCUS_TRACE_LINES: Cell<u16> = const { Cell::new(0) };
    static FOCUS_TRACE_SEQUENCE: Cell<u32> = const { Cell::new(0) };
    static FOCUS_TRACE_EVENT: Cell<Option<u32>> = const { Cell::new(None) };
    static FOCUS_TRACE_SAMPLING: Cell<bool> = const { Cell::new(false) };
    static DOWNLOAD_SITE_QUERYING: Cell<bool> = const { Cell::new(false) };
    static SITE_PERMISSION_QUERYING: Cell<bool> = const { Cell::new(false) };
    static TAB_AUDIO_QUERYING: Cell<bool> = const { Cell::new(false) };
    static TAB_AUDIO_MENU: RefCell<Option<TabAudioMenu>> = const { RefCell::new(None) };
    static TAB_ORDER_MENU: RefCell<[Option<TabOrderMenu>; 2]> = const { RefCell::new([None, None]) };
    static TAB_ORDER_QUERYING: Cell<bool> = const { Cell::new(false) };
    static NAVIGATION_CONTROL_MENU: RefCell<Option<NavigationControlMenu>> = const { RefCell::new(None) };
    static NAVIGATION_CONTROL_QUERYING: Cell<bool> = const { Cell::new(false) };
    // At most one synchronous call may defer its own address rendering.
    static NAVIGATION_CONTROL_CALL: RefCell<Option<Rc<NavigationControlCall>>> = const { RefCell::new(None) };
    // Rc clones share one order snapshot; pointer motion never clones its Vec.
    static TAB_DRAG: RefCell<Option<Rc<TabDragSession>>> = const { RefCell::new(None) };
    static TAB_DRAG_QUERYING: Cell<bool> = const { Cell::new(false) };
    static TAB_DRAG_EPOCH: Cell<u64> = const { Cell::new(0) };
}

fn focus_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("NARU_FOCUS_DIAGNOSTICS").is_some_and(|value| value == "1")
    })
}

fn is_native_tab_event(event: &NSEvent) -> bool {
    event.r#type() == NSEventType::KeyDown
        && event.keyCode() == 48
        && !event.modifierFlags().intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        )
}

fn site_permissions_cover_content(window: &NSWindow, content: &NSView) -> bool {
    SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                shell.page_is_covered_by_site_permissions()
                    && std::ptr::eq(&*shell.window, window)
                    && std::ptr::eq(&*shell.content, content)
            })
        })
    })
}

fn restore_site_permission_close_after_tab(
    event: &NSEvent,
    marker: MainThreadMarker,
    window: &NSWindow,
    content: &NSView,
    previous: &NSView,
) {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.page_is_covered_by_site_permissions()
                && std::ptr::eq(&*shell.window, window)
                && std::ptr::eq(&*shell.content, content)
        })?;
        Some((
            shell.site_permissions.surface.clone(),
            shell.site_permissions.close.clone(),
        ))
    });
    let Some((surface, close)) = controls else {
        return;
    };
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || event.window(marker).as_deref() != Some(window)
        || surface.window().as_deref() != Some(window)
        || surface.isHiddenOrHasHiddenAncestor()
        || close.window().as_deref() != Some(window)
        || close.isHiddenOrHasHiddenAncestor()
        || !close.isDescendantOf(&surface)
        || !close.isEnabled()
    {
        return;
    }
    let still_covered_cef = window.firstResponder().is_some_and(|responder| {
        responder.downcast_ref::<NSView>().is_some_and(|current| {
            std::ptr::eq(current, previous) && current.isDescendantOf(content)
        })
    });
    if !still_covered_cef {
        return;
    }
    // Native getters may reenter. Keep a newer responder or replacement panel,
    // and never use an approval button as the fallback keyboard destination.
    let current = SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                shell.page_is_covered_by_site_permissions()
                    && std::ptr::eq(&*shell.window, window)
                    && std::ptr::eq(&*shell.content, content)
                    && std::ptr::eq(&*shell.site_permissions.surface, &*surface)
                    && std::ptr::eq(&*shell.site_permissions.close, &*close)
            })
        })
    });
    if current {
        window.makeFirstResponder(Some(&close));
    }
}

pub(crate) fn correct_hidden_cef_tab_focus(event: &NSEvent) {
    if !is_native_tab_event(event) {
        return;
    }
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        Some((shell.window.clone(), shell.content.clone()))
    });
    let Some((window, content)) = controls else {
        return;
    };
    if !window.isKeyWindow() || event.window(marker).as_deref() != Some(&*window) {
        return;
    }
    let Some(first) = window.firstResponder() else {
        return;
    };
    let Some(first_view) = first.downcast_ref::<NSView>() else {
        return;
    };
    if !first_view.isDescendantOf(&content)
        || (!first_view.isHiddenOrHasHiddenAncestor()
            && !site_permissions_cover_content(&window, &content))
    {
        return;
    }
    let backwards = event.modifierFlags().contains(NSEventModifierFlags::Shift);
    // Chromium's canBecomeKeyView ignores hidden ancestors. Covered permission
    // pages are visible to Blink but remain excluded from native Tab traversal.
    let next = |view: &NSView| unsafe {
        if backwards {
            view.previousValidKeyView()
        } else {
            view.nextValidKeyView()
        }
    };
    let mut candidate = next(first_view);
    let mut visited: Vec<Retained<NSView>> = Vec::new();
    // Human tabs have no fixed count limit. Bound malformed/oversized loops to
    // 512 candidates and at most 4 KiB of temporary retained references on arm64.
    for _ in 0..512 {
        let Some(view) = candidate else { break };
        if std::ptr::eq(&*view, first_view)
            || visited.iter().any(|seen| std::ptr::eq(&**seen, &*view))
            || view.window().as_deref() != Some(&*window)
        {
            break;
        }
        if !view.isHiddenOrHasHiddenAncestor()
            && view.canBecomeKeyView()
            && !(view.isDescendantOf(&content) && site_permissions_cover_content(&window, &content))
        {
            // Native queries may re-enter. Never replace a newer responder or
            // move focus after the original event window stops being key.
            let still_blocked_cef = window.firstResponder().is_some_and(|responder| {
                responder.downcast_ref::<NSView>().is_some_and(|current| {
                    std::ptr::eq(current, first_view)
                        && current.isDescendantOf(&content)
                        && (current.isHiddenOrHasHiddenAncestor()
                            || site_permissions_cover_content(&window, &content))
                })
            });
            if window.isKeyWindow()
                && event.window(marker).as_deref() == Some(&*window)
                && still_blocked_cef
                && view.window().as_deref() == Some(&*window)
                && !view.isHiddenOrHasHiddenAncestor()
                && view.canBecomeKeyView()
                && !(view.isDescendantOf(&content)
                    && site_permissions_cover_content(&window, &content))
            {
                window.makeFirstResponder(Some(&view));
            }
            break;
        }
        candidate = next(&view);
        visited.push(view);
    }
    // FKA may exclude every native button from next/previousValidKeyView.
    // All unsuccessful traversal exits converge here; successful traversal has
    // already changed the responder and the fallback leaves it untouched.
    restore_site_permission_close_after_tab(event, marker, &window, &content, first_view);
}

pub(crate) fn begin_focus_tab_trace(event: &NSEvent) -> Option<Option<u32>> {
    // Outer None skips sampling; the inner value restores nested event context.
    if !focus_trace_enabled()
        || !is_native_tab_event(event)
        || FOCUS_TRACE_SAMPLING.with(Cell::get)
        || FOCUS_TRACE_LINES.with(Cell::get) >= 128
    {
        return None;
    }
    let marker = MainThreadMarker::new()?;
    let window = SHELL.with(|slot| {
        slot.try_borrow()
            .ok()?
            .as_ref()
            .map(|shell| shell.window.clone())
    })?;
    if !window.isKeyWindow() || event.window(marker).as_deref() != Some(&*window) {
        return None;
    }
    let sequence = FOCUS_TRACE_SEQUENCE.with(|value| {
        let next = value.get() + 1;
        value.set(next);
        next
    });
    let previous = FOCUS_TRACE_EVENT.with(|value| value.replace(Some(sequence)));
    trace_focus(
        if event.modifierFlags().contains(NSEventModifierFlags::Shift) {
            "backtab-before"
        } else {
            "tab-before"
        },
        None,
        None,
        None,
    );
    Some(previous)
}

pub(crate) fn end_focus_tab_trace(previous: Option<u32>) {
    trace_focus("tab-after", None, None, None);
    FOCUS_TRACE_EVENT.with(|value| value.set(previous));
}

fn focus_view_summary(
    view: Option<&NSView>,
    content: &NSView,
    address: &NSView,
    center: &NSView,
) -> String {
    view.map(|view| {
        let role = if std::ptr::eq(view, address) {
            "address"
        } else if std::ptr::eq(view, center) {
            "center"
        } else {
            "other"
        };
        format!(
            "{}[role={role},hidden={},ancestor_hidden={},cef={},key={}]",
            view.class().name().to_string_lossy(),
            view.isHidden(),
            view.isHiddenOrHasHiddenAncestor(),
            view.isDescendantOf(content),
            view.canBecomeKeyView()
        )
    })
    .unwrap_or_else(|| "none".into())
}

pub(crate) fn trace_focus(event: &str, tab: Option<i32>, source: Option<u32>, deny: Option<bool>) {
    // Arm on the first native Tab, keeping startup callbacks out of the budget.
    if !focus_trace_enabled()
        || MainThreadMarker::new().is_none()
        || FOCUS_TRACE_SEQUENCE.with(Cell::get) == 0
        || FOCUS_TRACE_LINES.with(Cell::get) >= 128
        || FOCUS_TRACE_SAMPLING.with(|value| value.replace(true))
    {
        return;
    }
    struct Sampling;
    impl Drop for Sampling {
        fn drop(&mut self) {
            FOCUS_TRACE_SAMPLING.with(|value| value.set(false));
        }
    }
    let _sampling = Sampling;
    FOCUS_TRACE_LINES.with(|value| value.set(value.get() + 1));
    let sequence = FOCUS_TRACE_EVENT.with(Cell::get);
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        Some((
            shell.window.clone(),
            shell.address.clone(),
            shell.start_page.input.clone(),
            shell.content.clone(),
            shell.active,
            shell.start_page.visible,
            shell.overlay_is_open(),
        ))
    });
    let Some((window, address, center, content, active, start_visible, overlay)) = controls else {
        eprintln!(
            "focus event={event} sequence={sequence:?} tab={tab:?} source={source:?} deny={deny:?} shell=unavailable"
        );
        return;
    };
    // Retained live window views are queried on the main thread, outside SHELL.
    // currentEditor never reads text. next/previousValidKeyView may lazily
    // recalculate AppKit's dirty loop; no diagnostic explicitly changes focus.
    let first = window.firstResponder();
    let first_class = first
        .as_ref()
        .map(|view| view.class().name().to_string_lossy());
    let first_view = first
        .as_ref()
        .and_then(|view| view.downcast_ref::<NSView>());
    let (
        address_editor,
        center_editor,
        address_next,
        address_previous,
        center_next,
        center_previous,
    ): (
        Option<Retained<NSView>>,
        Option<Retained<NSView>>,
        _,
        _,
        _,
        _,
    ) = unsafe {
        (
            msg_send![&*address, currentEditor],
            msg_send![&*center, currentEditor],
            address.nextValidKeyView(),
            address.previousValidKeyView(),
            center.nextValidKeyView(),
            center.previousValidKeyView(),
        )
    };
    eprintln!(
        "focus event={event} sequence={sequence:?} tab={tab:?} source={source:?} deny={deny:?} active={active:?} start={start_visible} overlay={overlay} first_class={first_class:?} first={} address_editor={} center_editor={} address_next={} address_previous={} center_next={} center_previous={}",
        focus_view_summary(first_view, &content, &address, &center),
        focus_view_summary(address_editor.as_deref(), &content, &address, &center),
        focus_view_summary(center_editor.as_deref(), &content, &address, &center),
        focus_view_summary(address_next.as_deref(), &content, &address, &center),
        focus_view_summary(address_previous.as_deref(), &content, &address, &center),
        focus_view_summary(center_next.as_deref(), &content, &address, &center),
        focus_view_summary(center_previous.as_deref(), &content, &address, &center)
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SurfaceKind {
    Window,
    Sidebar,
    Toolbar,
    Card,
    SelectedCard,
    Selection,
    Preview,
    Address,
    Badge,
    AccentBadge,
    Primary,
    Separator,
}

struct SurfaceStyle {
    kind: Cell<SurfaceKind>,
    radius: f64,
    border: bool,
    /// Only the page dialog strip takes keyboard focus (without Full Keyboard Access).
    focusable: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SurfaceStyle]
    struct SurfaceView;

    unsafe impl NSObjectProtocol for SurfaceView {}
    impl SurfaceView {
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool { true }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { self.ivars().focusable.get() }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            // AppKit owns this layer. Apply colors only in its supported update path.
            let style = self.ivars();
            let kind = style.kind.get();
            let dark = is_dark(self);
            let color = surface_color(kind, dark);
            let border_color = if kind == SurfaceKind::SelectedCard {
                accent_color()
            } else { rgb(if dark { 0x343A46 } else { 0xE5E8EB }) };
            unsafe {
                let layer: *mut AnyObject = msg_send![self, layer];
                if !layer.is_null() {
                    let background: *const c_void = msg_send![&*color, CGColor];
                    let border: *const c_void = msg_send![&*border_color, CGColor];
                    let _: () = msg_send![layer, setBackgroundColor: background];
                    let _: () = msg_send![layer, setBorderColor: border];
                    let _: () = msg_send![layer, setBorderWidth: if kind == SurfaceKind::SelectedCard { 1.0f64 } else if style.border { 0.5f64 } else { 0.0f64 }];
                    let _: () = msg_send![layer, setCornerRadius: style.radius];
                    let _: () = msg_send![layer, setMasksToBounds: style.radius > 0.0];
                }
            }
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.setNeedsDisplay(true);
        }
    }
);

impl SurfaceView {
    fn new(
        marker: MainThreadMarker,
        kind: SurfaceKind,
        radius: f64,
        border: bool,
    ) -> Retained<Self> {
        let view = Self::alloc(marker).set_ivars(SurfaceStyle {
            kind: Cell::new(kind),
            radius,
            border,
            focusable: Cell::new(false),
        });
        let view: Retained<Self> =
            unsafe { msg_send![super(view), initWithFrame: rect(0.0, 0.0, 1.0, 1.0)] };
        view.setWantsLayer(true);
        view.setNeedsDisplay(true);
        view
    }
    fn set_focusable(&self, focusable: bool) {
        self.ivars().focusable.set(focusable);
    }
    fn set_kind(&self, kind: SurfaceKind) {
        if self.ivars().kind.replace(kind) != kind {
            self.setNeedsDisplay(true);
        }
    }
}

fn surface_layer_geometry(layer: &AnyObject) -> String {
    // The caller supplies only AppKit's backing layer or its presentation layer.
    // These selectors read geometry; diagnostics never change owned layers.
    unsafe {
        let frame: NSRect = msg_send![layer, frame];
        let bounds: NSRect = msg_send![layer, bounds];
        let position: NSPoint = msg_send![layer, position];
        let anchor: NSPoint = msg_send![layer, anchorPoint];
        format!(
            "class={} frame={frame:?} bounds={bounds:?} position={position:?} anchor={anchor:?}",
            layer.class().name().to_string_lossy(),
        )
    }
}

thread_local! {
    static SURFACE_DIAGNOSTIC_STATE: Cell<(u8, u8)> = const { Cell::new((0, 0)) };
}

fn surface_geometry_diagnostics_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("NARU_SURFACE_DIAGNOSTICS").is_some_and(|value| value == "1")
    })
}

fn trace_sidebar_surface_geometry() {
    if !surface_geometry_diagnostics_enabled() {
        return;
    }
    let armed = SURFACE_DIAGNOSTIC_STATE.with(|state| {
        let (toggles, samples) = state.get();
        if samples >= 32 {
            return false;
        }
        state.set((toggles + 1, samples));
        true
    });
    if armed {
        trace_sidebar_geometry("toggle-after-layout");
    }
}

pub(crate) fn trace_sidebar_geometry(event: &str) {
    if !surface_geometry_diagnostics_enabled() {
        return;
    }
    let stamp = SURFACE_DIAGNOSTIC_STATE.with(|state| {
        let (toggles, samples) = state.get();
        (toggles > 0 && samples < 32).then(|| {
            state.set((toggles, samples + 1));
            (toggles, samples + 1)
        })
    });
    let Some((sequence, sample)) = stamp else {
        return;
    };
    let surfaces = SHELL.with(|slot| {
        let borrowed = slot.try_borrow().ok()?;
        let shell = borrowed.as_ref()?;
        Some((
            shell.sidebar_collapsed,
            shell.search_button_background.clone(),
            shell.new_tab_background.clone(),
        ))
    });
    let Some((collapsed, search, new_tab)) = surfaces else {
        return;
    };
    // Read AppKit only after dropping the shell borrow. The output contains no
    // page, tab, input, window title, or pointer identity information.
    for (role, surface) in [("search", search), ("new_tab", new_tab)] {
        let view: &NSView = &surface;
        let frame = view.frame();
        let bounds = view.bounds();
        let visible = view.visibleRect();
        let layer_rect = view.convertRectToLayer(bounds);
        let (model, presentation) = unsafe {
            let layer: *mut AnyObject = msg_send![view, layer];
            if let Some(layer) = layer.as_ref() {
                let presentation: *mut AnyObject = msg_send![layer, presentationLayer];
                (
                    Some(surface_layer_geometry(layer)),
                    presentation.as_ref().map(surface_layer_geometry),
                )
            } else {
                (None, None)
            }
        };
        eprintln!(
            "NARU_SURFACE_DIAGNOSTICS event={event} sample={sample} toggle={sequence} collapsed={collapsed} role={role} class={} frame={frame:?} bounds={bounds:?} visible={visible:?} layer_rect={layer_rect:?} clips={} needs_display={} model=[{}] presentation=[{}]",
            view.class().name().to_string_lossy(),
            view.clipsToBounds(),
            view.needsDisplay(),
            model.as_deref().unwrap_or("none"),
            presentation.as_deref().unwrap_or("none"),
        );
    }
}

fn probe_sidebar_surface_getter() {
    static MODE: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    let Some(mode) = *MODE.get_or_init(|| match std::env::var("NARU_SURFACE_PROBE").as_deref() {
        Ok("noop") => Some("noop"),
        Ok("frame") => Some("frame"),
        Ok("bounds") => Some("bounds"),
        Ok("visible") => Some("visible"),
        Ok("convert") => Some("convert"),
        Ok("layer") => Some("layer"),
        Ok("presentation") => Some("presentation"),
        _ => None,
    }) else {
        return;
    };
    // A full trace would add other getters and invalidate this comparison.
    if surface_geometry_diagnostics_enabled() || focus_trace_enabled() {
        return;
    }
    thread_local! {
        static SEQUENCE: Cell<u8> = const { Cell::new(0) };
    }
    let sequence = SEQUENCE.with(|count| {
        let previous = count.get();
        (previous < 16).then(|| {
            count.set(previous + 1);
            previous + 1
        })
    });
    let Some(sequence) = sequence else {
        return;
    };
    let surfaces = SHELL.with(|slot| {
        let borrowed = slot.try_borrow().ok()?;
        let shell = borrowed.as_ref()?;
        Some((
            shell.search_button_background.clone(),
            shell.new_tab_background.clone(),
            if shell.sidebar_collapsed {
                RAIL_WIDTH
            } else {
                SIDEBAR_WIDTH
            },
        ))
    });
    let Some((search, new_tab, side)) = surfaces else {
        return;
    };
    // Use the layout's known local rectangle without a second bounds getter.
    let known_bounds = rect(0.0, 0.0, side - 24.0, 40.0);
    for surface in [search, new_tab] {
        let view: &NSView = &surface;
        match mode {
            "frame" => {
                std::hint::black_box(view.frame());
            }
            "bounds" => {
                std::hint::black_box(view.bounds());
            }
            "visible" => {
                std::hint::black_box(view.visibleRect());
            }
            "convert" => {
                std::hint::black_box(view.convertRectToLayer(known_bounds));
            }
            "layer" => unsafe {
                let layer: *mut AnyObject = msg_send![view, layer];
                std::hint::black_box(layer);
            },
            "presentation" => unsafe {
                // Compare with the layer-only mode: this prerequisite is shared.
                let layer: *mut AnyObject = msg_send![view, layer];
                if let Some(layer) = layer.as_ref() {
                    let presentation: *mut AnyObject = msg_send![layer, presentationLayer];
                    std::hint::black_box(presentation);
                }
            },
            _ => {} // noop uses the same counter, retained views, and stderr.
        }
    }
    eprintln!("NARU_SURFACE_PROBE mode={mode} sequence={sequence}");
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct TopDownView;

    unsafe impl NSObjectProtocol for TopDownView {}
    impl TopDownView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }
    }
);

impl TopDownView {
    fn new(marker: MainThreadMarker) -> Retained<Self> {
        let view = Self::alloc(marker).set_ivars(());
        unsafe { msg_send![super(view), initWithFrame: rect(0.0, 0.0, 1.0, 1.0)] }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct ShellTarget;

    unsafe impl NSObjectProtocol for ShellTarget {}
    unsafe impl NSMenuDelegate for ShellTarget {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            for index in 0..menu.numberOfItems() {
                if let Some(item) = menu.itemAtIndex(index) {
                    if item.action() == Some(sel!(toggleTabAudio:)) {
                        prepare_tab_audio_menu(item.clone());
                        item.setEnabled(validate_tab_audio_menu(&item));
                    } else if let Some(forward) = tab_order_menu_direction(&item) {
                        prepare_tab_order_menu(item.clone(), forward);
                        item.setEnabled(validate_tab_order_menu(&item, forward));
                    } else if item.action() == Some(sel!(reloadTab:)) {
                        prepare_navigation_control_menu(item.clone());
                        item.setEnabled(validate_navigation_control_menu(&item));
                    }
                }
            }
        }
    }
    unsafe impl NSWindowDelegate for ShellTarget {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _window: &NSWindow) -> bool {
            // CEF must finish every beforeunload/close callback before teardown.
            crate::browser::close_all();
            false
        }
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            cancel_tab_drag();
            layout();
        }
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) { cancel_tab_drag(); }
        #[unsafe(method(windowWillMiniaturize:))]
        fn window_will_miniaturize(&self, _notification: &NSNotification) { cancel_tab_drag(); }
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            crate::browser::notify_panel();
        }
        #[unsafe(method(windowDidBecomeMain:))]
        fn window_did_become_main(&self, _notification: &NSNotification) {
            crate::browser::notify_panel();
        }
        #[unsafe(method(windowDidDeminiaturize:))]
        fn window_did_deminiaturize(&self, _notification: &NSNotification) {
            crate::browser::notify_panel();
        }
    }
    impl ShellTarget {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            if item.action() == Some(sel!(toggleTabAudio:)) {
                validate_tab_audio_menu(item)
            } else if let Some(forward) = tab_order_menu_direction(item) {
                validate_tab_order_menu(item, forward)
            } else if item.action() == Some(sel!(reloadTab:)) {
                validate_navigation_control_menu(item)
            } else if item.action() == Some(sel!(reopenClosedTab:)) {
                can_reopen_closed_tab_ui()
            } else if item.action() == Some(sel!(openCurrentPageInUserTab:)) {
                current_page_target_ui().is_some()
            } else if item.action() == Some(sel!(nextTab:)) {
                tab_selection_target(|shell| cycle_tab_target(&shell.tabs, &shell.active_space, shell.active, true)).is_some()
            } else if item.action() == Some(sel!(previousTab:)) {
                tab_selection_target(|shell| cycle_tab_target(&shell.tabs, &shell.active_space, shell.active, false)).is_some()
            } else {
                true
            }
        }

        #[unsafe(method(toggleTabAudio:))]
        fn toggle_tab_audio(&self, sender: &NSMenuItem) { toggle_tab_audio(sender); }

        #[unsafe(method(moveTabUp:))]
        fn move_tab_up(&self, sender: &NSMenuItem) { reorder_tab(sender, false); }

        #[unsafe(method(moveTabDown:))]
        fn move_tab_down(&self, sender: &NSMenuItem) { reorder_tab(sender, true); }

        #[unsafe(method(nextTab:))]
        fn next_tab(&self, _sender: Option<&AnyObject>) { cycle_tab(true); }

        #[unsafe(method(previousTab:))]
        fn previous_tab(&self, _sender: Option<&AnyObject>) { cycle_tab(false); }

        #[unsafe(method(openCurrentPageInUserTab:))]
        fn open_current_page_in_user_tab(&self, _sender: Option<&AnyObject>) {
            open_current_page_in_user_tab();
        }

        #[unsafe(method(reopenClosedTab:))]
        fn reopen_closed_tab(&self, _sender: Option<&AnyObject>) {
            reopen_closed_tab();
        }

        #[unsafe(method(selectTab:))]
        fn select_tab(&self, sender: &NSButton) {
            tab_row_action(sender, false);
        }
        #[unsafe(method(dragTab:))]
        fn drag_tab(&self, sender: &TabDragRecognizer) { tab_drag_action(sender); }
        #[unsafe(method(scrollDraggedTab:))]
        fn scroll_dragged_tab(&self, timer: &NSTimer) { tab_drag_timer_fired(timer); }
        #[unsafe(method(closeTabRow:))]
        fn close_tab_row(&self, sender: &NSButton) {
            tab_row_action(sender, true);
        }
        #[unsafe(method(toggleTabRowAudio:))]
        fn toggle_tab_row_audio(&self, sender: &NSButton) { toggle_tab_row_audio(sender); }
        #[unsafe(method(selectSpace:))]
        fn select_space(&self, sender: &NSButton) {
            let index: isize = unsafe { msg_send![sender, tag] };
            let space = SHELL.with(|slot| {
                slot.borrow().as_ref()?.cards.get(usize::try_from(index).ok()?)
                    .map(|card| card.space.clone())
            });
            run_action(space.map(ShellAction::SelectSpace));
        }
        #[unsafe(method(stopSpace:))]
        fn stop_space(&self, sender: &NSButton) {
            let index: isize = unsafe { msg_send![sender, tag] };
            let action = SHELL.with(|slot| {
                let slot = slot.borrow();
                let shell = slot.as_ref()?;
                let card = shell.cards.get(usize::try_from(index).ok()?)?;
                shell.spaces.iter().find(|space| space.id == card.space)
                    .and_then(space_stop_scope).map(ShellAction::StopSpace)
            });
            run_action(action);
        }
        #[unsafe(method(resumeSpace:))]
        fn resume_space(&self, sender: &NSButton) {
            let index: isize = unsafe { msg_send![sender, tag] };
            let action = SHELL.with(|slot| {
                let slot = slot.borrow();
                let shell = slot.as_ref()?;
                let card = shell.cards.get(usize::try_from(index).ok()?)?;
                shell.spaces.iter().find(|space| space.id == card.space)
                    .and_then(space_resume_scope).map(ShellAction::ResumeSpace)
            });
            run_action(action);
        }
        #[unsafe(method(showSpaces:))]
        fn show_spaces(&self, _sender: Option<&AnyObject>) {
            let visible = SHELL.with(|slot| slot.borrow().as_ref().is_some_and(|shell| shell.overview));
            run_action(Some(ShellAction::ShowOverview(!visible)));
        }
        #[unsafe(method(showTabSearch:))]
        fn show_tab_search(&self, _sender: Option<&AnyObject>) { open_tab_search(); }
        #[unsafe(method(showHistory:))]
        fn show_history(&self, _sender: Option<&AnyObject>) { open_search(SearchMode::History); }
        #[unsafe(method(toggleSearchMode:))]
        fn toggle_search_mode(&self, _sender: Option<&AnyObject>) { toggle_search_mode(); }
        #[unsafe(method(prepareClearHistory:))]
        fn prepare_clear_history(&self, sender: &NSButton) { history_clear_action(sender, false, false); }
        #[unsafe(method(confirmClearHistory:))]
        fn confirm_clear_history(&self, sender: &NSButton) { history_clear_action(sender, true, false); }
        #[unsafe(method(cancelClearHistory:))]
        fn cancel_clear_history(&self, sender: &NSButton) { history_clear_action(sender, false, true); }
        #[unsafe(method(closeTabSearch:))]
        fn close_tab_search(&self, _sender: Option<&AnyObject>) { close_tab_search(true); }
        #[unsafe(method(selectSearchTab:))]
        fn select_search_tab(&self, sender: &NSButton) {
            select_search_result(sender);
        }
        #[unsafe(method(submitTabSearch:))]
        fn submit_tab_search(&self, _sender: Option<&AnyObject>) { submit_tab_search(); }
        #[unsafe(method(showBookmarks:))]
        fn show_bookmarks(&self, _sender: Option<&AnyObject>) { open_bookmarks(false); }
        #[unsafe(method(addBookmark:))]
        fn add_bookmark(&self, _sender: Option<&AnyObject>) { open_bookmarks(true); }
        #[unsafe(method(closeBookmarks:))]
        fn close_bookmarks(&self, _sender: Option<&AnyObject>) { close_bookmarks(true); }
        #[unsafe(method(showDownloads:))]
        fn show_downloads(&self, _sender: Option<&AnyObject>) { open_downloads(); }
        #[unsafe(method(closeDownloads:))]
        fn close_downloads(&self, _sender: Option<&AnyObject>) { close_downloads(true); }
        #[unsafe(method(showSitePermissions:))]
        fn show_site_permissions(&self, _sender: Option<&AnyObject>) { open_site_permissions(); }
        #[unsafe(method(closeSitePermissions:))]
        fn close_site_permissions(&self, _sender: Option<&AnyObject>) { close_site_permissions(true); }
        #[unsafe(method(allowSitePermission:))]
        fn allow_site_permission(&self, sender: &NSButton) { site_permission_action(sender, SitePermissionAction::Allow); }
        #[unsafe(method(cancelSitePermission:))]
        fn cancel_site_permission(&self, sender: &NSButton) { site_permission_action(sender, SitePermissionAction::Cancel); }
        #[unsafe(method(revealSitePermissionSource:))]
        fn reveal_site_permission_source(&self, sender: &NSButton) { site_permission_action(sender, SitePermissionAction::Reveal); }
        #[unsafe(method(resetCurrentSitePermissions:))]
        fn reset_current_site_permissions(&self, sender: &NSButton) { reset_site_permissions(sender); }
        #[unsafe(method(chooseDownloadPath:))]
        fn choose_download_path(&self, sender: &NSButton) { download_row_action(sender, true); }
        #[unsafe(method(cancelDownload:))]
        fn cancel_download(&self, sender: &NSButton) { download_row_action(sender, false); }
        #[unsafe(method(revealDownloadSource:))]
        fn reveal_download_source(&self, sender: &NSButton) { reveal_download_source(sender, false); }
        #[unsafe(method(revealDownloadFile:))]
        fn reveal_download_file(&self, sender: &NSButton) { reveal_download_file(sender); }
        #[unsafe(method(restartDownload:))]
        fn restart_download(&self, sender: &NSButton) { restart_download(sender); }
        #[unsafe(method(chooseRequestedFiles:))]
        fn choose_requested_files(&self, sender: &NSButton) { file_selection_action(sender, true); }
        #[unsafe(method(cancelFileSelection:))]
        fn cancel_file_selection(&self, sender: &NSButton) { file_selection_action(sender, false); }
        #[unsafe(method(allowMultipleDownloads:))]
        fn allow_multiple_downloads(&self, sender: &NSButton) { download_permission_action(sender, true); }
        #[unsafe(method(cancelMultipleDownloadsRequest:))]
        fn cancel_multiple_downloads_request(&self, sender: &NSButton) { download_permission_action(sender, false); }
        #[unsafe(method(revealPermissionSource:))]
        fn reveal_permission_source(&self, sender: &NSButton) { reveal_download_source(sender, true); }
        #[unsafe(method(resetCurrentSiteDownloads:))]
        fn reset_current_site_downloads(&self, sender: &NSButton) { reset_download_site(sender); }
        #[unsafe(method(clearFinishedDownloads:))]
        fn clear_finished_downloads(&self, _sender: Option<&AnyObject>) {
            crate::download_runtime::clear_finished();
            set_download_notice(None);
            sync();
        }
        #[unsafe(method(openBookmark:))]
        fn open_bookmark(&self, sender: &NSButton) { bookmark_row_action(sender, None); }
        #[unsafe(method(renameBookmark:))]
        fn rename_bookmark(&self, sender: &NSButton) { bookmark_row_action(sender, Some(BookmarkOperation::Prepare)); }
        #[unsafe(method(removeBookmark:))]
        fn remove_bookmark(&self, sender: &NSButton) { bookmark_row_action(sender, Some(BookmarkOperation::Remove)); }
        #[unsafe(method(saveBookmark:))]
        fn save_bookmark(&self, _sender: Option<&AnyObject>) { save_bookmark(); }
        #[unsafe(method(cancelBookmark:))]
        fn cancel_bookmark(&self, _sender: Option<&AnyObject>) { cancel_bookmark_editor(); }
        #[unsafe(method(submitBookmarks:))]
        fn submit_bookmarks(&self, _sender: Option<&AnyObject>) { submit_bookmarks(); }
        #[unsafe(method(showPageFind:))]
        fn show_page_find(&self, _sender: Option<&AnyObject>) { open_page_find(false); }
        #[unsafe(method(nextPageFind:))]
        fn next_page_find(&self, _sender: Option<&AnyObject>) { step_page_find(true); }
        #[unsafe(method(previousPageFind:))]
        fn previous_page_find(&self, _sender: Option<&AnyObject>) { step_page_find(false); }
        #[unsafe(method(closePageFind:))]
        fn close_page_find(&self, _sender: Option<&AnyObject>) { close_page_find(true, true); }
        #[unsafe(method(editFailedPageAddress:))]
        fn edit_failed_page_address(&self, sender: &NSButton) { page_recovery_action(sender, false); }
        #[unsafe(method(retryFailedPage:))]
        fn retry_failed_page(&self, sender: &NSButton) { page_recovery_action(sender, true); }
        #[unsafe(method(answerPageDialog:))]
        fn answer_page_dialog(&self, sender: &NSObject) { page_dialog_action(sender, true); }
        #[unsafe(method(cancelPageDialog:))]
        fn cancel_page_dialog(&self, sender: &NSObject) { page_dialog_action(sender, false); }
        #[unsafe(method(submitStartPage:))]
        fn submit_start_page(&self, sender: &AnyObject) { start_page_action(sender, false); }
        #[unsafe(method(openStartPageBookmarks:))]
        fn open_start_page_bookmarks(&self, sender: &AnyObject) { start_page_action(sender, true); }
        #[unsafe(method(controlTextDidChange:))]
        fn search_text_changed(&self, _notification: &NSNotification) {
            if page_find_is_editing() { page_find_text_changed(); }
            else if bookmarks_are_open() { bookmark_text_changed(); } else { search_text_changed(); }
        }
        #[unsafe(method(toggleSidebar:))]
        fn toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            cancel_tab_drag();
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.sidebar_collapsed = !shell.sidebar_collapsed;
                    shell.tab_signature.clear();
                    shell.render_controls();
                }
            });
            layout();
            restore_hidden_tab_mute_focus();
            trace_sidebar_surface_geometry();
            probe_sidebar_surface_getter();
        }
        #[unsafe(method(previousSpaces:))]
        fn previous_spaces(&self, _sender: Option<&AnyObject>) {
            page_spaces(false);
        }
        #[unsafe(method(nextSpaces:))]
        fn next_spaces(&self, _sender: Option<&AnyObject>) {
            page_spaces(true);
        }
        #[unsafe(method(goBack:))]
        fn go_back(&self, _sender: Option<&AnyObject>) {
            run_action(active_tab().map(ShellAction::Back));
        }
        #[unsafe(method(goForward:))]
        fn go_forward(&self, _sender: Option<&AnyObject>) {
            run_action(active_tab().map(ShellAction::Forward));
        }
        #[unsafe(method(reloadTab:))]
        fn reload_tab(&self, sender: Option<&AnyObject>) {
            navigation_control_action(sender);
        }
        #[unsafe(method(focusAddress:))]
        fn focus_address(&self, _sender: Option<&AnyObject>) {
            focus_address();
        }
        #[unsafe(method(navigate:))]
        fn navigate(&self, _sender: Option<&AnyObject>) {
            let input = SHELL.with(|slot| {
                let slot = slot.borrow();
                let shell = slot.as_ref()?;
                Some((shell.active?, shell.address.stringValue().to_string()))
            });
            let Some((id, value)) = input else { return };
            let resolved = crate::address::resolve(&value);
            if matches!(resolved, Ok(None)) { return; }
            // Return may end the field editor before this action. Protect the
            // submitted text through validation and synchronous CEF callbacks.
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.address_draft = shell.tabs.iter().find(|tab| tab.id == id)
                        .map(|tab| AddressDraft {
                            tab: id,
                            url: tab.url.clone(),
                            navigation_epoch: tab.navigation_epoch,
                        });
                }
            });
            let result = resolved
                .map_err(|error| error.to_string())
                .and_then(|url| match url {
                    Some(url) => crate::browser::shell_action(ShellAction::Navigate(id, url))
                        .map_err(|error| format!("처리하지 못했습니다: {error}")),
                    None => Ok(()),
                });
            let failed = result.is_err();
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    if !failed { shell.address_draft = None; }
                    shell.notice = result.err();
                }
            });
            if failed { focus_address(); }
            refresh();
        }
        #[unsafe(method(newTab:))]
        fn new_tab(&self, _sender: Option<&AnyObject>) {
            run_action(Some(ShellAction::NewTab));
        }
        #[unsafe(method(closeTab:))]
        fn close_tab(&self, _sender: Option<&AnyObject>) {
            run_action(active_tab().map(ShellAction::CloseTab));
        }
        #[unsafe(method(resumeTask:))]
        fn resume_task(&self, _sender: Option<&AnyObject>) {
            let action = SHELL.with(|slot| {
                let slot = slot.borrow();
                let shell = slot.as_ref()?;
                shell.spaces.iter().find(|space| space.id == shell.active_space)
                    .and_then(space_resume_scope).map(ShellAction::ResumeSpace)
            });
            run_action(action);
        }
        #[unsafe(method(stopTask:))]
        fn stop_task(&self, _sender: Option<&AnyObject>) {
            let action = SHELL.with(|slot| {
                let slot = slot.borrow();
                let shell = slot.as_ref()?;
                shell.spaces.iter().find(|space| space.id == shell.active_space)
                    .and_then(space_stop_scope).map(ShellAction::StopSpace)
            });
            run_action(action);
        }
    }
);

impl ShellTarget {
    fn new(marker: MainThreadMarker) -> Retained<Self> {
        let object = Self::alloc(marker).set_ivars(());
        unsafe { msg_send![super(object), init] }
    }
}

impl BookmarkPanel {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Window, 0.0, true, marker);
        surface.setHidden(true);
        let title = label(&surface, "북마크", 24.0, true, marker);
        let hint = label(
            &surface,
            "모든 공간에서 공유하는 북마크",
            12.0,
            false,
            marker,
        );
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let query = bookmark_field(
            &surface,
            "북마크 제목 또는 URL 검색 (최대 200자)",
            target,
            sel!(submitBookmarks:),
            marker,
        );
        let close = button(
            &surface,
            "닫기",
            target,
            sel!(closeBookmarks:),
            "북마크 닫기 (Esc)",
            marker,
        );
        let add = button(
            &surface,
            "현재 페이지 추가",
            target,
            sel!(addBookmark:),
            "현재 페이지 북마크 추가 또는 이름 편집 (⌘D)",
            marker,
        );
        let editor = surface_child(&surface, SurfaceKind::Address, 9.0, true, marker);
        editor.setHidden(true);
        let name = bookmark_field(
            &editor,
            "북마크 이름 (최대 256자)",
            target,
            sel!(saveBookmark:),
            marker,
        );
        let url = label(&editor, "", 11.0, false, marker);
        url.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let save = button(
            &editor,
            "저장",
            target,
            sel!(saveBookmark:),
            "북마크 이름 저장",
            marker,
        );
        let cancel = button(
            &editor,
            "취소",
            target,
            sel!(cancelBookmark:),
            "북마크 편집 취소",
            marker,
        );
        let document = TopDownView::new(marker);
        let selection = surface_child(&document, SurfaceKind::Selection, 9.0, false, marker);
        selection.setHidden(true);
        let scroll = scroll_view(&surface, &document, marker);
        Self {
            state: BookmarkState::default(),
            surface,
            title,
            hint,
            query,
            close,
            add,
            editor,
            name,
            url,
            save,
            cancel,
            scroll,
            document,
            selection,
            rows: Vec::new(),
            signature: String::new(),
            writable: false,
            busy: false,
        }
    }

    fn clear_rows(&mut self) {
        for row in self.rows.drain(..) {
            row.open.removeFromSuperview();
            row.rename.removeFromSuperview();
            row.remove.removeFromSuperview();
            row.title.removeFromSuperview();
            row.detail.removeFromSuperview();
        }
        self.signature.clear();
        self.selection.setHidden(true);
    }

    fn render(&mut self, snapshot: crate::bookmark_runtime::Snapshot, target: &ShellTarget) {
        self.writable = snapshot.ready && snapshot.writable;
        self.busy = snapshot.busy || self.state.pending.is_some();
        let mutable = self.writable && !self.busy;
        self.add.setEnabled(mutable);
        self.save.setEnabled(mutable && self.state.draft.is_some());
        self.name.setEnabled(mutable);
        self.editor.setHidden(self.state.draft.is_none());
        let message = self.state.notice.as_deref().or(snapshot.notice.as_deref());
        let summary = if !snapshot.ready {
            "북마크를 불러오는 중입니다".to_owned()
        } else if self.busy {
            "저장 처리 중 · 닫아도 진행 중인 저장은 계속됩니다".to_owned()
        } else if !snapshot.writable {
            "읽기 전용 · 저장할 수 없습니다".to_owned()
        } else if snapshot.rows.is_empty() {
            if snapshot.total == 0 {
                "저장된 북마크가 없습니다 · ⌘D로 현재 페이지 추가".to_owned()
            } else {
                "일치하는 북마크가 없습니다".to_owned()
            }
        } else {
            format!(
                "전체 {}개 · 최대 30개 표시 · Enter로 내 공간에서 열기",
                snapshot.total
            )
        };
        let text = message
            .map(|message| format!("{summary} · {message}"))
            .unwrap_or(summary);
        self.hint
            .setStringValue(&NSString::from_str(&compact(&text, 105)));
        let detail = if snapshot.writable {
            text
        } else {
            format!("{text}\n{}", snapshot.path)
        };
        self.hint.setToolTip(Some(&NSString::from_str(&detail)));
        unsafe {
            let _: () = msg_send![&*self.hint, setAccessibilityHelp: &*NSString::from_str(&detail)];
        }
        if !snapshot
            .rows
            .iter()
            .any(|item| Some(item.id) == self.state.selected)
        {
            self.state.selected = snapshot.rows.first().map(|item| item.id);
        }
        let signature = format!("{:?}:{mutable}:{:?}", self.state.selected, snapshot.rows);
        if self.signature == signature {
            return;
        }
        self.clear_rows();
        self.signature = signature;
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        for (index, item) in snapshot.rows.into_iter().take(30).enumerate() {
            let selected = self.state.selected == Some(item.id);
            let title = label(
                &self.document,
                &compact(&item.title, 100),
                14.0,
                selected,
                marker,
            );
            let detail = label(
                &self.document,
                &compact(&item.url, 160),
                11.5,
                false,
                marker,
            );
            detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
            let open = button(
                &self.document,
                "",
                target,
                sel!(openBookmark:),
                &format!("{} · {} · 내 공간에서 열기", item.title, item.url),
                marker,
            );
            let rename = button(
                &self.document,
                "이름",
                target,
                sel!(renameBookmark:),
                &format!("{} 이름 편집", item.title),
                marker,
            );
            let remove = button(
                &self.document,
                "삭제",
                target,
                sel!(removeBookmark:),
                &format!("{} 북마크 삭제", item.title),
                marker,
            );
            rename.setEnabled(mutable);
            remove.setEnabled(mutable);
            for control in [&open, &rename, &remove] {
                unsafe {
                    let _: () = msg_send![&**control, setTag: index as isize];
                }
            }
            unsafe {
                let _: () = msg_send![&*open, setAccessibilityLabel: &*NSString::from_str(&format!("{}{}", item.title, if selected { ", 선택됨" } else { "" }))];
            }
            self.rows.push(BookmarkRow {
                item,
                open,
                rename,
                remove,
                title,
                detail,
            });
        }
    }

    fn layout(&self, size: NSSize) {
        let width = size.width - 72.0;
        self.surface
            .setFrame(rect(0.0, 0.0, size.width, size.height));
        self.title
            .setFrame(rect(36.0, size.height - 66.0, width - 260.0, 32.0));
        self.add
            .setFrame(rect(size.width - 265.0, size.height - 62.0, 160.0, 30.0));
        self.close
            .setFrame(rect(size.width - 94.0, size.height - 62.0, 58.0, 30.0));
        self.query
            .setFrame(rect(36.0, size.height - 112.0, width, 32.0));
        self.hint
            .setFrame(rect(36.0, size.height - 145.0, width, 22.0));
        let editing = self.state.draft.is_some();
        let editor_height = if editing { 112.0 } else { 0.0 };
        self.editor
            .setFrame(rect(36.0, size.height - 257.0, width, 100.0));
        self.name.setFrame(rect(14.0, 56.0, width - 28.0, 30.0));
        self.url.setFrame(rect(14.0, 17.0, width - 180.0, 20.0));
        self.save.setFrame(rect(width - 145.0, 12.0, 60.0, 30.0));
        self.cancel.setFrame(rect(width - 77.0, 12.0, 60.0, 30.0));
        let height = (size.height - 177.0 - editor_height).max(1.0);
        self.scroll.setFrame(rect(36.0, 24.0, width, height));
        let width = self.scroll.contentSize().width.max(1.0);
        self.document.setFrame(rect(
            0.0,
            0.0,
            width,
            (self.rows.len() as f64 * 64.0).max(height),
        ));
        self.selection.setHidden(true);
        for (index, row) in self.rows.iter().enumerate() {
            let top = index as f64 * 64.0;
            row.title
                .setFrame(rect(14.0, top + 9.0, width - 158.0, 21.0));
            row.detail
                .setFrame(rect(14.0, top + 34.0, width - 158.0, 18.0));
            row.open.setFrame(rect(0.0, top, width - 140.0, 61.0));
            row.rename
                .setFrame(rect(width - 132.0, top + 16.0, 58.0, 30.0));
            row.remove
                .setFrame(rect(width - 66.0, top + 16.0, 58.0, 30.0));
            if self.state.selected == Some(row.item.id) {
                self.selection.setFrame(rect(0.0, top, width, 61.0));
                self.selection.setHidden(false);
            }
        }
    }
}

impl DownloadSiteCard {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Card, 9.0, true, marker);
        let title = label(
            &surface,
            "현재 사이트의 여러 파일 다운로드",
            14.0,
            true,
            marker,
        );
        let origin = label(&surface, "현재 웹페이지가 없습니다", 12.0, false, marker);
        let status = label(&surface, "", 11.5, false, marker);
        let scope_hint = label(
            &surface,
            "모든 공간의 같은 사이트 설정에 적용됩니다.",
            11.0,
            false,
            marker,
        );
        let reopen_hint = label(
            &surface,
            "열린 페이지는 다시 열어 확인하세요.",
            11.0,
            false,
            marker,
        );
        let pending_hint = label(
            &surface,
            "대기 중인 요청은 취소될 수 있습니다.",
            11.0,
            false,
            marker,
        );
        let files_hint = label(
            &surface,
            "진행 중인 파일은 유지됩니다.",
            11.0,
            false,
            marker,
        );
        for label in [
            &status,
            &scope_hint,
            &reopen_hint,
            &pending_hint,
            &files_hint,
        ] {
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        let reset = button(
            &surface,
            "허용 설정 초기화",
            target,
            sel!(resetCurrentSiteDownloads:),
            "현재 사이트의 저장된 다운로드 허용을 기본값으로 되돌림, 모든 공간에 적용됨",
            marker,
        );
        reset.setEnabled(false);
        Self {
            surface,
            title,
            origin,
            status,
            scope_hint,
            reopen_hint,
            pending_hint,
            files_hint,
            reset,
            current: None,
        }
    }

    fn render(&mut self, query: DownloadSiteQuery, window: &NSWindow) -> bool {
        use crate::download_permissions::SiteSetting;
        // A same-target nested sync must not erase the card or move focus just
        // because its outer CEF query is still running.
        let DownloadSiteQuery::Ready(value) = query else {
            return false;
        };
        let (current, origin, status) = match value {
            Some((page, Ok(snapshot))) => {
                let status = match snapshot.setting {
                    SiteSetting::Allowed => "현재 설정: 허용됨",
                    SiteSetting::Ask => "현재 설정: 확인 필요 또는 기본값",
                    SiteSetting::Blocked => "현재 설정: 차단됨",
                    SiteSetting::Unknown => "현재 설정을 확인할 수 없습니다",
                };
                let origin = snapshot.origin.clone();
                (Some((page, snapshot)), origin, status.to_owned())
            }
            Some((_, Err(error))) => (None, "사이트 설정을 확인할 수 없습니다".to_owned(), error),
            None => (
                None,
                "현재 웹페이지가 없습니다".to_owned(),
                "HTTP(S) 페이지를 연 뒤 확인할 수 있습니다.".to_owned(),
            ),
        };
        let changed = self
            .current
            .as_ref()
            .map(|(page, snapshot)| (page, &snapshot.origin))
            != current
                .as_ref()
                .map(|(page, snapshot)| (page, &snapshot.origin));
        let enabled = current
            .as_ref()
            .is_some_and(|(_, snapshot)| snapshot.setting == SiteSetting::Allowed);
        let reset_view: &NSView = &self.reset;
        let focus_removed = (changed || !enabled)
            && window.firstResponder().is_some_and(|responder| {
                responder
                    .downcast_ref::<NSView>()
                    .is_some_and(|view| std::ptr::eq(view, reset_view))
            });
        for (label, value) in [
            (&self.origin, origin.as_str()),
            (&self.status, status.as_str()),
        ] {
            label.setStringValue(&NSString::from_str(&compact(value, 180)));
            let full = NSString::from_str(value);
            label.setToolTip(Some(&full));
            unsafe {
                let _: () = msg_send![&**label, setAccessibilityHelp: &*full];
            }
        }
        let help = NSString::from_str(&format!(
            "{origin}, 저장된 다운로드 허용 설정 초기화, 모든 공간에 적용, 열린 페이지는 다시 열어 확인, 대기 중인 요청은 취소될 수 있음, 진행 중인 파일은 유지"
        ));
        self.reset.setToolTip(Some(&help));
        unsafe {
            let _: () = msg_send![&*self.reset, setAccessibilityLabel: &*help];
        }
        self.reset.setEnabled(enabled);
        self.current = current;
        focus_removed
    }

    fn layout(&self, width: f64) {
        self.surface.setFrame(rect(0.0, 0.0, width, 218.0));
        let text_width = (width - 28.0).max(1.0);
        for (label, y, height) in [
            (&self.title, 186.0, 21.0),
            (&self.origin, 162.0, 18.0),
            (&self.status, 138.0, 18.0),
            (&self.scope_hint, 114.0, 18.0),
            (&self.reopen_hint, 92.0, 18.0),
            (&self.pending_hint, 70.0, 18.0),
            (&self.files_hint, 48.0, 18.0),
        ] {
            label.setFrame(rect(14.0, y, text_width, height));
        }
        self.reset
            .setFrame(rect((width - 152.0).max(0.0), 10.0, 138.0, 30.0));
    }
}

impl DownloadPanel {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Window, 0.0, true, marker);
        surface.setHidden(true);
        let title = label(&surface, "다운로드", 24.0, true, marker);
        let hint = label(&surface, "아직 다운로드가 없습니다", 12.0, false, marker);
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let clear = button(
            &surface,
            "완료 기록 지우기",
            target,
            sel!(clearFinishedDownloads:),
            "완료된 다운로드 기록 지우기, 저장된 파일은 삭제하지 않음",
            marker,
        );
        let close = button(
            &surface,
            "닫기",
            target,
            sel!(closeDownloads:),
            "다운로드 닫기 (Esc)",
            marker,
        );
        let document = TopDownView::new(marker);
        let scroll = scroll_view(&surface, &document, marker);
        let site = DownloadSiteCard::new(&document, target, marker);
        Self {
            open: false,
            notice: None,
            surface,
            title,
            hint,
            close,
            clear,
            scroll,
            document,
            site,
            file_selections: Vec::new(),
            permissions: Vec::new(),
            rows: Vec::new(),
        }
    }

    fn clear_rows(&mut self) {
        self.site.current = None;
        for row in self.file_selections.drain(..) {
            row.surface.removeFromSuperview();
        }
        for row in self.permissions.drain(..) {
            row.surface.removeFromSuperview();
        }
        for row in self.rows.drain(..) {
            row.surface.removeFromSuperview();
        }
    }

    // Keep stable controls during progress updates so Tab focus survives.
    // Return true only when the focused row itself was removed.
    fn render(
        &mut self,
        snapshot: crate::downloads::Snapshot,
        permissions: Vec<crate::download_permission_runtime::PermissionRow>,
        file_selections: Vec<crate::file_selection::Row>,
        file_notice: Option<String>,
        site: DownloadSiteQuery,
        target: &ShellTarget,
        window: &NSWindow,
    ) -> bool {
        let site_focus_removed = self.site.render(site, window);
        let summary = if !file_selections.is_empty() {
            format!(
                "파일 선택 요청 {}개 · 다운로드 진행 중 {}개 · 다운로드 허용 대기 {}개",
                file_selections.len(),
                snapshot.active_count,
                permissions.len()
            )
        } else if !permissions.is_empty() {
            format!(
                "여러 파일 다운로드 허용 대기 {}개, 진행 중 {}개",
                permissions.len(),
                snapshot.active_count
            )
        } else if snapshot.rows.is_empty() {
            "아직 다운로드가 없습니다".to_owned()
        } else {
            format!(
                "진행 중 {}개 · 최대 24개 표시 · 기록을 지워도 파일은 유지됩니다",
                snapshot.active_count
            )
        };
        let summary = if snapshot.rows.is_empty() {
            summary
        } else {
            format!("{summary}. 추가 파일이 보이지 않으면 접수 탭에서 확인하세요.")
        };
        let text = self
            .notice
            .as_deref()
            .or(file_notice.as_deref())
            .or(snapshot.notice.as_deref())
            .map(|notice| format!("{summary} · {notice}"))
            .unwrap_or(summary);
        self.hint
            .setStringValue(&NSString::from_str(&compact(&text, 110)));
        self.hint.setToolTip(Some(&NSString::from_str(&text)));
        unsafe {
            let _: () = msg_send![&*self.hint, setAccessibilityHelp: &*NSString::from_str(&text)];
        }
        self.clear.setEnabled(
            snapshot
                .rows
                .iter()
                .any(|row| !row.can_choose && !row.can_cancel),
        );
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        let mut previous_permissions = std::mem::take(&mut self.permissions);
        for item in permissions.into_iter().take(4) {
            let row = if let Some(index) = previous_permissions
                .iter()
                .position(|row| row.token == item.token)
            {
                previous_permissions.remove(index)
            } else {
                let surface = surface_child(&self.document, SurfaceKind::Card, 9.0, true, marker);
                let title = label(
                    &surface,
                    "이 사이트의 여러 파일 다운로드",
                    14.0,
                    true,
                    marker,
                );
                let origin = label(&surface, "", 12.0, false, marker);
                let source = label(&surface, "", 11.0, false, marker);
                let scope_hint = label(
                    &surface,
                    "사이트 설정으로 저장되며 다른 공간에도 적용됩니다.",
                    11.5,
                    false,
                    marker,
                );
                let save_hint = label(
                    &surface,
                    "파일은 저장 위치를 선택한 뒤 받습니다.",
                    11.5,
                    false,
                    marker,
                );
                for label in [&source, &scope_hint, &save_hint] {
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                }
                let allow = button(
                    &surface,
                    "허용",
                    target,
                    sel!(allowMultipleDownloads:),
                    "이 사이트의 여러 파일 다운로드 허용, 다른 공간에도 적용됨",
                    marker,
                );
                allow.setContentTintColor(Some(&accent_color()));
                let cancel = button(
                    &surface,
                    "이번 요청 취소",
                    target,
                    sel!(cancelMultipleDownloadsRequest:),
                    "여러 파일 다운로드 요청 취소, 사이트를 영구 차단하지 않음",
                    marker,
                );
                let reveal = button(
                    &surface,
                    "요청 탭 보기",
                    target,
                    sel!(revealPermissionSource:),
                    "다운로드를 요청한 탭 보기, AI 제어권 유지",
                    marker,
                );
                DownloadPermissionRow {
                    token: item.token,
                    surface,
                    title,
                    origin,
                    source,
                    scope_hint,
                    save_hint,
                    allow,
                    cancel,
                    reveal,
                }
            };
            // Keep the complete origin accessible even when the native label
            // truncates visually in a narrow window.
            for (label, value) in [(&row.origin, &item.origin), (&row.source, &item.source)] {
                let text = NSString::from_str(value);
                label.setStringValue(&text);
                label.setToolTip(Some(&text));
                unsafe {
                    let _: () = msg_send![&**label, setAccessibilityHelp: &*text];
                }
            }
            for (control, action) in [
                (&row.allow, "여러 파일 다운로드 허용, 다른 공간에도 적용됨"),
                (&row.cancel, "이번 요청 취소, 사이트를 영구 차단하지 않음"),
                (&row.reveal, "요청 탭 보기, AI 제어권 유지"),
            ] {
                let hint =
                    NSString::from_str(&format!("{}, {}, {action}", item.origin, item.source));
                control.setToolTip(Some(&hint));
                unsafe {
                    let _: () = msg_send![&**control, setAccessibilityLabel: &*hint];
                }
            }
            self.permissions.push(row);
        }
        let responder = window.firstResponder();
        let mut action_focus_removed = false;
        let mut previous_files = std::mem::take(&mut self.file_selections);
        for item in file_selections
            .into_iter()
            .take(crate::file_selection::MAX_PENDING)
        {
            let mut row = if let Some(index) = previous_files
                .iter()
                .position(|row| row.token == item.token)
            {
                previous_files.remove(index)
            } else {
                let surface = surface_child(&self.document, SurfaceKind::Card, 9.0, true, marker);
                let title = label(&surface, "파일 선택 요청", 14.0, true, marker);
                let source = label(&surface, "", 11.5, false, marker);
                let detail = label(&surface, "", 11.5, false, marker);
                let delivery_hint = label(
                    &surface,
                    "선택한 파일을 요청 페이지에 전달합니다.",
                    11.5,
                    false,
                    marker,
                );
                let upload_hint = label(
                    &surface,
                    "사이트에서 바로 업로드할 수 있습니다.",
                    11.5,
                    false,
                    marker,
                );
                for hint in [&source, &detail, &delivery_hint, &upload_hint] {
                    hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
                }
                let choose = button(
                    &surface,
                    "파일 선택",
                    target,
                    sel!(chooseRequestedFiles:),
                    "파일을 직접 선택하여 요청 페이지에 전달",
                    marker,
                );
                let cancel = button(
                    &surface,
                    "요청 취소",
                    target,
                    sel!(cancelFileSelection:),
                    "이번 파일 선택 요청 취소",
                    marker,
                );
                FileSelectionRow {
                    token: item.token,
                    choosing: item.choosing,
                    surface,
                    title,
                    source,
                    detail,
                    delivery_hint,
                    upload_hint,
                    choose,
                    cancel,
                }
            };
            let source = format!("{} · 요청 탭 {}", item.source, item.tab);
            let detail = format!("{} · 제안 형식: {}", item.mode.label(), item.accept_hint);
            for (label, value) in [(&row.source, &source), (&row.detail, &detail)] {
                label.setStringValue(&NSString::from_str(&compact(value, 180)));
                label.setToolTip(Some(&NSString::from_str(value)));
                unsafe {
                    let _: () =
                        msg_send![&**label, setAccessibilityHelp: &*NSString::from_str(value)];
                }
            }
            row.choose.setTitle(&NSString::from_str(if item.choosing {
                "선택 중"
            } else if item.mode == crate::file_selection::Mode::Folder {
                "폴더 선택"
            } else {
                "파일 선택"
            }));
            row.choose.setEnabled(!item.choosing);
            if item.choosing {
                let control: &NSView = &row.choose;
                action_focus_removed |= responder
                    .as_ref()
                    .and_then(|responder| responder.downcast_ref::<NSView>())
                    .is_some_and(|view| std::ptr::eq(view, control));
            }
            row.choosing = item.choosing;
            for (control, action) in [(&row.choose, "파일 선택"), (&row.cancel, "요청 취소")]
            {
                let hint = NSString::from_str(&format!(
                    "{source} · {action}. 선택한 파일을 요청 페이지에 전달합니다. 사이트에서 바로 업로드할 수 있습니다."
                ));
                control.setToolTip(Some(&hint));
                unsafe {
                    let _: () = msg_send![&**control, setAccessibilityLabel: &*hint];
                }
            }
            self.file_selections.push(row);
        }
        let mut previous = std::mem::take(&mut self.rows);
        for item in snapshot.rows.into_iter().take(24) {
            let row = if let Some(index) = previous.iter().position(|row| row.id == item.id) {
                previous.remove(index)
            } else {
                let surface = surface_child(&self.document, SurfaceKind::Card, 9.0, true, marker);
                let name = label(&surface, "", 14.0, true, marker);
                let source = label(&surface, "", 11.0, false, marker);
                let detail = label(&surface, "", 11.5, false, marker);
                source.setTextColor(Some(&NSColor::secondaryLabelColor()));
                detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
                let choose = button(
                    &surface,
                    "저장 위치 선택",
                    target,
                    sel!(chooseDownloadPath:),
                    "다운로드 저장 위치 선택",
                    marker,
                );
                let cancel = button(
                    &surface,
                    "취소",
                    target,
                    sel!(cancelDownload:),
                    "다운로드 취소",
                    marker,
                );
                let reveal = button(
                    &surface,
                    "접수 탭 보기",
                    target,
                    sel!(revealDownloadSource:),
                    "파일이 접수된 탭 보기, 접수 이후 페이지나 제어권이 바뀌면 이동하지 않음",
                    marker,
                );
                // A distinct selector keeps a delayed Save As action from
                // becoming a Finder action when this stable row completes.
                let reveal_file = button(
                    &surface,
                    "Finder에서 보기",
                    target,
                    sel!(revealDownloadFile:),
                    "완료한 다운로드 파일의 위치를 Finder에서 보기",
                    marker,
                );
                let restart = button(
                    &surface,
                    "다시 받기",
                    target,
                    sel!(restartDownload:),
                    "파일을 처음부터 다시 요청하고 저장 위치를 새로 선택",
                    marker,
                );
                for control in [&choose, &cancel] {
                    unsafe {
                        let _: () = msg_send![&**control, setTag: item.id as isize];
                    }
                }
                DownloadRow {
                    id: item.id,
                    can_reveal: item.can_reveal,
                    can_reveal_file: item.can_reveal_file,
                    can_restart: item.can_restart,
                    surface,
                    name,
                    source,
                    detail,
                    choose,
                    cancel,
                    reveal,
                    reveal_file,
                    restart,
                }
            };
            let mut row = row;
            for (label, value, limit) in [
                (&row.name, item.name.as_str(), 160),
                (&row.source, item.source.as_str(), 240),
                (&row.detail, item.detail.as_str(), 240),
            ] {
                label.setStringValue(&NSString::from_str(&compact(value, limit)));
                label.setToolTip(Some(&NSString::from_str(value)));
            }
            row.choose.setEnabled(item.can_choose);
            row.cancel.setEnabled(item.can_cancel);
            let save_visible = !item.can_reveal_file && (item.can_choose || item.can_cancel);
            row.choose.setHidden(!save_visible);
            row.reveal_file.setHidden(!item.can_reveal_file);
            row.reveal_file.setEnabled(item.can_reveal_file);
            row.restart.setHidden(!item.can_restart);
            row.restart.setEnabled(item.can_restart);
            for (control, available) in [
                (&row.reveal, item.can_reveal),
                (&row.reveal_file, item.can_reveal_file),
                (&row.choose, save_visible),
                (&row.restart, item.can_restart),
            ] {
                if !available {
                    let control_view: &NSView = control;
                    action_focus_removed |= responder
                        .as_ref()
                        .and_then(|responder| responder.downcast_ref::<NSView>())
                        .is_some_and(|view| std::ptr::eq(view, control_view));
                }
            }
            row.can_reveal = item.can_reveal;
            row.can_reveal_file = item.can_reveal_file;
            row.can_restart = item.can_restart;
            row.reveal.setEnabled(item.can_reveal);
            for (control, action) in [
                (&row.choose, "저장 위치 선택"),
                (&row.cancel, "다운로드 취소"),
                (
                    &row.restart,
                    "파일을 처음부터 다시 요청하고 저장 위치를 새로 선택",
                ),
                (
                    &row.reveal_file,
                    "완료한 다운로드 파일의 위치를 Finder에서 보기",
                ),
                (
                    &row.reveal,
                    "파일이 접수된 탭 보기, 접수 이후 페이지나 제어권이 바뀌면 이동하지 않음. 추가 파일이 보이지 않으면 접수 탭에서 확인하세요.",
                ),
            ] {
                let hint = NSString::from_str(&format!("{} · {action}", item.name));
                control.setToolTip(Some(&hint));
                unsafe {
                    let _: () = msg_send![&**control, setAccessibilityLabel: &*hint];
                }
            }
            self.rows.push(row);
        }
        let focus_removed = responder
            .as_ref()
            .and_then(|responder| responder.downcast_ref::<NSView>())
            .is_some_and(|responder| {
                previous
                    .iter()
                    .any(|row| responder.isDescendantOf(&row.surface))
                    || previous_files
                        .iter()
                        .any(|row| responder.isDescendantOf(&row.surface))
                    || previous_permissions
                        .iter()
                        .any(|row| responder.isDescendantOf(&row.surface))
            });
        for row in previous_files {
            row.surface.removeFromSuperview();
        }
        for row in previous_permissions {
            row.surface.removeFromSuperview();
        }
        for row in previous {
            row.surface.removeFromSuperview();
        }
        focus_removed || site_focus_removed || action_focus_removed
    }

    fn layout(&self, size: NSSize) {
        let width = (size.width - 72.0).max(1.0);
        self.surface
            .setFrame(rect(0.0, 0.0, size.width, size.height));
        self.title
            .setFrame(rect(36.0, size.height - 66.0, width - 240.0, 32.0));
        self.clear
            .setFrame(rect(size.width - 265.0, size.height - 62.0, 160.0, 30.0));
        self.close
            .setFrame(rect(size.width - 94.0, size.height - 62.0, 58.0, 30.0));
        self.hint
            .setFrame(rect(36.0, size.height - 105.0, width, 22.0));
        let height = (size.height - 145.0).max(1.0);
        self.scroll.setFrame(rect(36.0, 24.0, width, height));
        let width = self.scroll.contentSize().width.max(1.0);
        let site_height = 226.0;
        let selection_height = self.file_selections.len() as f64 * 180.0;
        let permission_height = self.permissions.len() as f64 * 180.0;
        self.document.setFrame(rect(
            0.0,
            0.0,
            width,
            (selection_height + site_height + permission_height + self.rows.len() as f64 * 128.0)
                .max(height),
        ));
        self.site.layout(width);
        self.site
            .surface
            .setFrameOrigin(NSPoint::new(0.0, selection_height));
        for (index, row) in self.file_selections.iter().enumerate() {
            row.surface
                .setFrame(rect(0.0, index as f64 * 180.0, width, 172.0));
            let text_width = (width - 28.0).max(1.0);
            row.title.setFrame(rect(14.0, 141.0, text_width, 21.0));
            row.source.setFrame(rect(14.0, 117.0, text_width, 18.0));
            row.detail.setFrame(rect(14.0, 94.0, text_width, 18.0));
            row.delivery_hint
                .setFrame(rect(14.0, 68.0, text_width, 18.0));
            row.upload_hint.setFrame(rect(14.0, 47.0, text_width, 18.0));
            row.choose
                .setFrame(rect((width - 224.0).max(14.0), 10.0, 100.0, 30.0));
            row.cancel
                .setFrame(rect((width - 116.0).max(122.0), 10.0, 102.0, 30.0));
        }
        for (index, row) in self.permissions.iter().enumerate() {
            row.surface.setFrame(rect(
                0.0,
                selection_height + site_height + index as f64 * 180.0,
                width,
                172.0,
            ));
            let text_width = (width - 28.0).max(1.0);
            // Card surfaces are unflipped: y = card height - top - control height.
            row.title.setFrame(rect(14.0, 141.0, text_width, 21.0));
            row.origin.setFrame(rect(14.0, 117.0, text_width, 18.0));
            row.source.setFrame(rect(14.0, 94.0, text_width, 18.0));
            row.scope_hint.setFrame(rect(14.0, 68.0, text_width, 18.0));
            row.save_hint.setFrame(rect(14.0, 47.0, text_width, 18.0));
            let [reveal, allow, cancel] = download_action_frames(width, true);
            row.allow.setFrame(allow);
            row.cancel.setFrame(cancel);
            row.reveal.setFrame(reveal);
        }
        for (index, row) in self.rows.iter().enumerate() {
            row.surface.setFrame(rect(
                0.0,
                selection_height + site_height + permission_height + index as f64 * 128.0,
                width,
                120.0,
            ));
            let text_width = (width - 28.0).max(1.0);
            row.name.setFrame(rect(14.0, 87.0, text_width, 21.0));
            row.source.setFrame(rect(14.0, 66.0, text_width, 18.0));
            row.detail.setFrame(rect(14.0, 45.0, text_width, 18.0));
            let [reveal, choose, cancel] = download_action_frames(width, false);
            row.choose.setFrame(choose);
            row.reveal_file.setFrame(choose);
            row.restart.setFrame(choose);
            row.cancel.setFrame(cancel);
            row.reveal.setFrame(reveal);
        }
    }
}

impl SitePermissionPanel {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Window, 0.0, true, marker);
        surface.setHidden(true);
        let title = label(&surface, "사이트 권한", 24.0, true, marker);
        let hint = label(
            &surface,
            "대기 중인 권한 요청이 없습니다.",
            12.0,
            false,
            marker,
        );
        site_permission_wrap(&hint, 3);
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let close = button(
            &surface,
            "닫기",
            target,
            sel!(closeSitePermissions:),
            "사이트 권한 닫기 (Esc), 대기 요청은 유지됨",
            marker,
        );
        let document = TopDownView::new(marker);
        let scroll = scroll_view(&surface, &document, marker);
        let site_surface = surface_child(&document, SurfaceKind::Card, 9.0, true, marker);
        let site_title = label(
            &site_surface,
            "현재 사이트의 저장된 권한",
            14.0,
            true,
            marker,
        );
        let site_origin = label(
            &site_surface,
            "현재 웹페이지가 없습니다",
            12.0,
            false,
            marker,
        );
        let site_status = label(&site_surface, "", 11.5, false, marker);
        let site_scope = label(
            &site_surface,
            "모든 공간의 같은 사이트에 적용됩니다.",
            11.5,
            false,
            marker,
        );
        let site_hint = label(
            &site_surface,
            "초기화하면 이 사이트의 대기 요청을 취소합니다. 열린 페이지는 다시 열어 확인하세요.",
            11.5,
            false,
            marker,
        );
        site_permission_wrap(&site_hint, 2);
        for label in [&site_status, &site_scope, &site_hint] {
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        let reset = button(
            &site_surface,
            "저장된 설정 초기화",
            target,
            sel!(resetCurrentSitePermissions:),
            "현재 사이트의 저장된 위치와 알림 권한 초기화",
            marker,
        );
        reset.setEnabled(false);
        Self {
            open: false,
            notice: None,
            surface,
            title,
            hint,
            close,
            scroll,
            document,
            site_surface,
            site_title,
            site_origin,
            site_status,
            site_scope,
            site_hint,
            reset,
            current: None,
            rows: Vec::new(),
        }
    }

    fn clear_rows(&mut self) {
        self.current = None;
        self.reset.setEnabled(false);
        for row in self.rows.drain(..) {
            row.surface.removeFromSuperview();
        }
    }

    fn render(
        &mut self,
        requests: Vec<crate::site_permission_runtime::Row>,
        notice: Option<String>,
        media: Option<String>,
        query: SitePermissionSiteQuery,
        target: &ShellTarget,
        window: &NSWindow,
    ) -> bool {
        let responder = window.firstResponder();
        let mut focus_removed = false;
        if let SitePermissionSiteQuery::Ready(value) = query {
            let (current, origin, status) = match value {
                Some((page, Ok(snapshot))) => {
                    let status = format!(
                        "위치: {}    알림: {}",
                        snapshot.geolocation.label(),
                        snapshot.notifications.label()
                    );
                    let origin = snapshot.origin.clone();
                    (Some((page, snapshot)), origin, status)
                }
                Some((_, Err(error))) => {
                    (None, "사이트 설정을 확인할 수 없습니다".to_owned(), error)
                }
                None => (
                    None,
                    "현재 웹페이지가 없습니다".to_owned(),
                    "HTTP(S) 페이지를 연 뒤 확인할 수 있습니다.".to_owned(),
                ),
            };
            let changed = self
                .current
                .as_ref()
                .map(|(page, site)| (page, &site.origin))
                != current.as_ref().map(|(page, site)| (page, &site.origin));
            let enabled = current
                .as_ref()
                .is_some_and(|(_, snapshot)| snapshot.can_reset());
            let reset_view: &NSView = &self.reset;
            focus_removed |= (changed || !enabled)
                && responder
                    .as_ref()
                    .and_then(|responder| responder.downcast_ref::<NSView>())
                    .is_some_and(|view| std::ptr::eq(view, reset_view));
            site_permission_label(&self.site_origin, &origin);
            site_permission_label(&self.site_status, &status);
            let help = NSString::from_str(&format!(
                "{origin}, 저장된 위치와 알림 설정 초기화, 모든 공간에 적용, 대기 요청 취소, 열린 페이지는 다시 열어 확인"
            ));
            self.reset.setToolTip(Some(&help));
            unsafe {
                let _: () = msg_send![&*self.reset, setAccessibilityLabel: &*help];
            }
            self.reset.setEnabled(enabled);
            self.current = current;
        }
        let summary = if requests.is_empty() {
            "대기 중인 권한 요청이 없습니다.".to_owned()
        } else {
            format!(
                "권한 요청 {}개. 출처와 적용 범위를 확인한 뒤 직접 선택하세요.",
                requests.len().min(4)
            )
        };
        let summary = media
            .as_deref()
            .map(|media| format!("{summary} {media}"))
            .unwrap_or(summary);
        let summary = self
            .notice
            .as_deref()
            .or(notice.as_deref())
            .map(|notice| format!("{summary} {notice}"))
            .unwrap_or(summary);
        site_permission_label(&self.hint, &summary);
        self.hint.setStringValue(&NSString::from_str(&summary));
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        let mut previous = std::mem::take(&mut self.rows);
        for item in requests.into_iter().take(4) {
            let row = if let Some(index) = previous.iter().position(|row| row.token == item.token) {
                previous.remove(index)
            } else {
                let surface = surface_child(&self.document, SurfaceKind::Card, 9.0, true, marker);
                let title = label(&surface, "", 14.0, true, marker);
                let origin = label(&surface, "", 12.0, false, marker);
                let source = label(&surface, "", 11.5, false, marker);
                let scope_hint = label(&surface, "", 11.5, false, marker);
                site_permission_wrap(&scope_hint, 3);
                for label in [&source, &scope_hint] {
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                }
                let allow = button(
                    &surface,
                    "허용",
                    target,
                    sel!(allowSitePermission:),
                    "표시된 사이트 권한 요청 허용",
                    marker,
                );
                allow.setContentTintColor(Some(&accent_color()));
                let cancel = button(
                    &surface,
                    "이번 요청 취소",
                    target,
                    sel!(cancelSitePermission:),
                    "이번 사이트 권한 요청 취소",
                    marker,
                );
                let reveal = button(
                    &surface,
                    "요청 탭 보기",
                    target,
                    sel!(revealSitePermissionSource:),
                    "권한을 요청한 탭 보기, AI 제어권 유지",
                    marker,
                );
                SitePermissionRow {
                    token: item.token,
                    surface,
                    title,
                    origin,
                    source,
                    scope_hint,
                    allow,
                    cancel,
                    reveal,
                }
            };
            let source = format!("{}, 요청 탭 {}", item.source, item.tab);
            for (label, value) in [
                (&row.title, format!("{} 사용 요청", item.permissions)),
                (&row.origin, item.origin.clone()),
                (&row.source, source.clone()),
                (&row.scope_hint, item.scope_hint.clone()),
            ] {
                site_permission_label(label, &value);
            }
            row.scope_hint
                .setStringValue(&NSString::from_str(&item.scope_hint));
            row.allow.setTitle(&NSString::from_str(&item.allow_label));
            for (control, action) in [
                (&row.allow, item.allow_label.as_str()),
                (&row.cancel, "이번 요청 취소"),
                (&row.reveal, "요청 탭 보기, AI 제어권 유지"),
            ] {
                let help = NSString::from_str(&format!(
                    "{}, {}, {}, {}, {action}",
                    item.origin, source, item.permissions, item.scope_hint
                ));
                control.setToolTip(Some(&help));
                unsafe {
                    let _: () = msg_send![&**control, setAccessibilityLabel: &*help];
                }
            }
            self.rows.push(row);
        }
        focus_removed |= responder
            .as_ref()
            .and_then(|responder| responder.downcast_ref::<NSView>())
            .is_some_and(|responder| {
                previous
                    .iter()
                    .any(|row| responder.isDescendantOf(&row.surface))
            });
        for row in previous {
            row.surface.removeFromSuperview();
        }
        focus_removed
    }

    fn layout(&self, size: NSSize) {
        let width = (size.width - 72.0).max(1.0);
        self.surface
            .setFrame(rect(0.0, 0.0, size.width, size.height));
        self.title.setFrame(rect(
            36.0,
            size.height - 66.0,
            (width - 80.0).max(1.0),
            32.0,
        ));
        self.close
            .setFrame(rect(size.width - 94.0, size.height - 62.0, 58.0, 30.0));
        self.hint
            .setFrame(rect(36.0, size.height - 132.0, width, 54.0));
        let height = (size.height - 169.0).max(1.0);
        self.scroll.setFrame(rect(36.0, 24.0, width, height));
        let width = self.scroll.contentSize().width.max(1.0);
        let narrow = width < 440.0;
        let row_height = if narrow { 248.0 } else { 214.0 };
        let requests_height = self.rows.len() as f64 * row_height;
        self.document
            .setFrame(rect(0.0, 0.0, width, (requests_height + 210.0).max(height)));
        let text_width = (width - 28.0).max(1.0);
        for (index, row) in self.rows.iter().enumerate() {
            let card_height = row_height - 8.0;
            row.surface
                .setFrame(rect(0.0, index as f64 * row_height, width, card_height));
            for (label, top, height) in [
                (&row.title, 10.0, 21.0),
                (&row.origin, 35.0, 18.0),
                (&row.source, 59.0, 18.0),
                (&row.scope_hint, 83.0, 54.0),
            ] {
                label.setFrame(rect(14.0, card_height - top - height, text_width, height));
            }
            row.reveal
                .setFrame(rect(14.0, if narrow { 44.0 } else { 10.0 }, 124.0, 30.0));
            row.allow
                .setFrame(rect((width - 264.0).max(14.0), 10.0, 124.0, 30.0));
            row.cancel
                .setFrame(rect((width - 130.0).max(148.0), 10.0, 116.0, 30.0));
        }
        self.site_surface
            .setFrame(rect(0.0, requests_height, width, 202.0));
        for (label, top, height) in [
            (&self.site_title, 10.0, 21.0),
            (&self.site_origin, 35.0, 18.0),
            (&self.site_status, 59.0, 18.0),
            (&self.site_scope, 83.0, 18.0),
            (&self.site_hint, 107.0, 36.0),
        ] {
            label.setFrame(rect(14.0, 202.0 - top - height, text_width, height));
        }
        self.reset
            .setFrame(rect((width - 170.0).max(14.0), 10.0, 156.0, 30.0));
    }
}

fn site_permission_label(label: &NSTextField, text: &str) {
    label.setStringValue(&NSString::from_str(&compact(text, 160)));
    let full = NSString::from_str(text);
    label.setToolTip(Some(&full));
    unsafe {
        let _: () = msg_send![label, setAccessibilityHelp: &*full];
    }
}

/// Untrusted page text: at most 600 characters, controls become spaces, the
/// label truncates with an ellipsis and the tooltip/help keep the stored text.
fn dialog_message_label(label: &NSTextField, text: &str) {
    label.setStringValue(&NSString::from_str(&compact(
        text,
        PAGE_DIALOG_MESSAGE_CHARS,
    )));
    let full = NSString::from_str(text);
    label.setToolTip(Some(&full));
    unsafe {
        let _: () = msg_send![label, setAccessibilityHelp: &*full];
    }
}

fn site_permission_wrap(label: &NSTextField, lines: isize) {
    unsafe {
        let _: () = msg_send![label, setUsesSingleLineMode: false];
        let _: () = msg_send![label, setLineBreakMode: 0isize];
        let _: () = msg_send![label, setMaximumNumberOfLines: lines];
    }
}

fn download_action_frames(width: f64, permission: bool) -> [NSRect; 3] {
    let (primary_x, primary_width, cancel_x, cancel_width) = if permission {
        (width - 196.0, 58.0, width - 130.0, 116.0)
    } else {
        (width - 205.0, 136.0, width - 65.0, 55.0)
    };
    [
        rect(14.0, 10.0, 132.0, 30.0),
        rect(primary_x, 10.0, primary_width, 30.0),
        rect(cancel_x, 10.0, cancel_width, 30.0),
    ]
}

fn bookmark_field(
    parent: &NSView,
    placeholder: &str,
    target: &ShellTarget,
    action: Sel,
    marker: MainThreadMarker,
) -> Retained<NSTextField> {
    let field = NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 30.0));
    field.setEditable(true);
    field.setSelectable(true);
    field.setFont(Some(&NSFont::systemFontOfSize(14.0)));
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    field.setToolTip(Some(&NSString::from_str(placeholder)));
    unsafe {
        field.setTarget(Some(target));
        field.setAction(Some(action));
        let _: () = msg_send![&*field, setDelegate: target];
    }
    parent.addSubview(&field);
    field
}

impl Shell {
    fn overlay_is_open(&self) -> bool {
        self.search.open
            || self.bookmarks.state.open
            || self.downloads.open
            || self.site_permissions.open
    }

    fn page_is_covered_by_site_permissions(&self) -> bool {
        self.site_permissions.open
            && !self.overview
            && !self.search.open
            && !self.bookmarks.state.open
            && !self.downloads.open
    }

    fn rebuild_search_results(
        &mut self,
        bookmarks: Option<crate::bookmark_runtime::Snapshot>,
        history: crate::history_runtime::Snapshot,
    ) {
        let history_only = self.search.mode == SearchMode::History;
        let tabs = if history_only {
            Vec::new()
        } else {
            tab_search::search(
                self.tabs.iter().map(|tab| SearchTab {
                    id: tab.id,
                    title: &tab.title,
                    url: &tab.url,
                    space_name: self
                        .spaces
                        .iter()
                        .find(|space| space.id == tab.space)
                        .map(|space| space.name.as_str())
                        .unwrap_or("내 공간"),
                }),
                &self.search.query,
            )
        };
        let bookmark_rows = bookmarks
            .as_ref()
            .map(|snapshot| snapshot.rows.as_slice())
            .unwrap_or(&[]);
        let mut results = tab_search::combined_results(
            tabs.iter().filter_map(|id| {
                self.tabs
                    .iter()
                    .find(|tab| tab.id == *id)
                    .map(|tab| (tab.id, tab.url.as_str()))
            }),
            bookmark_rows
                .iter()
                .map(|item| (item.id, item.url.as_str())),
            history.rows.iter().map(|item| (item.id, item.url.as_str())),
        );
        let query_destination = self.search.query_destination();
        if matches!(&query_destination, Ok(Some(_))) {
            tab_search::append_query_result(&mut results, self.search.revision);
        }
        self.search.reconcile(&results);
        let rows: Vec<_> = results
            .iter()
            .filter_map(|id| {
                let (name, detail, accessible) = match id {
                    SearchResultId::Tab(id) => {
                        let tab = self.tabs.iter().find(|tab| tab.id == *id)?;
                        let name = tab_search::display_title(&tab.title, &tab.url);
                        let space = self
                            .spaces
                            .iter()
                            .find(|space| space.id == tab.space)
                            .map(|space| space.name.as_str())
                            .unwrap_or("내 공간");
                        (
                            compact(name, 120),
                            format!(
                                "열린 탭 · {} · {}",
                                compact(space, 60),
                                compact(&tab.url, 180)
                            ),
                            format!(
                                "열린 탭, {}, 공간 {}, {}",
                                compact(name, 160),
                                compact(space, 80),
                                compact(&tab.url, 240)
                            ),
                        )
                    }
                    SearchResultId::Bookmark(id) => {
                        let item = bookmark_rows.iter().find(|item| item.id == *id)?;
                        (
                            compact(&item.title, 120),
                            format!("북마크 · 내 공간에서 열기 · {}", compact(&item.url, 180)),
                            format!(
                                "북마크, {}, 내 공간에서 열기, {}",
                                compact(&item.title, 160),
                                compact(&item.url, 240)
                            ),
                        )
                    }
                    SearchResultId::History(id) => {
                        let item = history.rows.iter().find(|item| item.id == *id)?;
                        let title = if item.title.is_empty() {
                            &item.url
                        } else {
                            &item.title
                        };
                        (
                            compact(title, 120),
                            format!("방문 기록, 내 공간에서 열기, {}", compact(&item.url, 180)),
                            format!(
                                "방문 기록, {}, 내 공간에서 열기, {}",
                                compact(title, 160),
                                compact(&item.url, 240)
                            ),
                        )
                    }
                    SearchResultId::Query(_) => {
                        let (_, kind) = query_destination.as_ref().ok()?.as_ref()?;
                        let action = match kind {
                            crate::address::AddressKind::Url => "주소 열기",
                            crate::address::AddressKind::Search => "Google에서 검색",
                        };
                        (
                            compact(&self.search.query, 120),
                            format!("{action} · 내 공간에서 열기"),
                            format!("{action}, {}, 내 공간에서 열기", self.search.query),
                        )
                    }
                };
                Some((*id, name, detail, accessible))
            })
            .collect();
        let tab_count = results
            .iter()
            .filter(|id| matches!(id, SearchResultId::Tab(_)))
            .count();
        let bookmark_count = results
            .iter()
            .filter(|id| matches!(id, SearchResultId::Bookmark(_)))
            .count();
        let history_count = results
            .iter()
            .filter(|id| matches!(id, SearchResultId::History(_)))
            .count();
        self.history_clear_available = history.ready && history.writable && !history.clearing;
        if !self.history_clear_available {
            self.search.confirm_clear = false;
        }
        let enter_hint = if self.search.confirm_clear {
            ""
        } else {
            ", Enter로 열기"
        };
        let summary = if history_only {
            format!(
                "기록 {}개, 전체 {}개, 최신순, 최대 40개{}",
                history_count, history.total, enter_hint
            )
        } else if results.is_empty() {
            "일치하는 탭, 북마크, 방문 기록이 없습니다".to_owned()
        } else if tab_count + bookmark_count + history_count == 0 {
            format!(
                "일치하는 탭, 북마크, 기록이 없습니다. 아래 주소·웹 검색 결과를 선택하세요{enter_hint}"
            )
        } else {
            format!(
                "탭 {}개, 북마크 {}개, 기록 {}개, 탭 우선, 최대 40개{}",
                tab_count, bookmark_count, history_count, enter_hint
            )
        };
        self.search_title
            .setStringValue(&NSString::from_str(if history_only {
                "방문 기록"
            } else {
                "통합 검색"
            }));
        self.search_mode
            .setTitle(&NSString::from_str(if history_only {
                "통합 검색으로"
            } else {
                "방문 기록만 보기"
            }));
        self.search_field
            .setPlaceholderString(Some(&NSString::from_str(if history_only {
                "기록의 제목 또는 URL (최대 200자)"
            } else {
                "탭, 북마크, 기록, 주소 또는 Google 검색 (최대 200자)"
            })));
        self.history_clear
            .setHidden(!history_only || self.search.confirm_clear);
        self.history_clear.setEnabled(self.history_clear_available);
        self.history_delete
            .setHidden(!history_only || !self.search.confirm_clear);
        self.history_delete.setEnabled(self.history_clear_available);
        self.history_cancel
            .setHidden(!history_only || !self.search.confirm_clear);
        let mut statuses = Vec::new();
        if self.search.query_overflow {
            statuses.push(
                "입력이 200자를 넘어 잘렸습니다. 주소나 웹 검색을 열려면 입력을 수정하세요."
                    .to_owned(),
            );
        } else if let Err(error) = query_destination {
            statuses.push(format!("주소 또는 웹 검색을 열 수 없습니다: {error}"));
        }
        if let Some(bookmarks) = &bookmarks {
            if !bookmarks.ready {
                statuses.push("북마크를 불러오는 중".to_owned());
            } else if let Some(notice) = &bookmarks.notice {
                statuses.push(format!("북마크: {notice}"));
            } else if bookmarks.busy {
                statuses.push("북마크 저장 중".to_owned());
            } else if !bookmarks.writable {
                statuses.push("북마크 읽기 전용".to_owned());
            }
        }
        if history.clearing {
            statuses.push("방문 기록을 지우는 중입니다".to_owned());
        } else if !history.ready {
            statuses.push("방문 기록을 불러오는 중".to_owned());
        } else if let Some(notice) = &history.notice {
            statuses.push(format!("방문 기록: {notice}"));
        } else if !history.writable {
            statuses.push("방문 기록 읽기 전용, 열기는 가능합니다".to_owned());
        } else if history.busy {
            statuses.push("방문 기록 저장 중".to_owned());
        }
        if self.search.confirm_clear {
            statuses.push("검색어와 관계없이 방문 기록 전체를 지웁니다. 삭제할까요?".to_owned());
        }
        if let Some(notice) = &self.search.notice {
            statuses.push(notice.clone());
        }
        statuses.push(summary);
        let hint = statuses.join(", ");
        self.search_hint
            .setStringValue(&NSString::from_str(&compact(&hint, 108)));
        self.search_hint
            .setToolTip(Some(&NSString::from_str(&hint)));
        unsafe {
            let _: () =
                msg_send![&*self.search_hint, setAccessibilityHelp: &*NSString::from_str(&hint)];
        }
        let signature = format!(
            "{:?}:{:?}:{:?}:{rows:?}",
            self.search.mode, self.search.query, self.search.selected
        );
        self.search_results = results;
        if self.search_signature == signature {
            return;
        }
        self.search_signature = signature;
        self.clear_search_rows();
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        for (index, (id, name, detail_text, accessible)) in rows.into_iter().enumerate() {
            let selected = self.search.selected == Some(id);
            let title = label(&self.search_document, &name, 14.0, selected, marker);
            let detail = label(&self.search_document, &detail_text, 11.5, false, marker);
            detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
            let button = button(
                &self.search_document,
                "",
                &self.target,
                sel!(selectSearchTab:),
                &accessible,
                marker,
            );
            unsafe {
                let _: () = msg_send![&*button, setTag: index as isize];
                let _: () = msg_send![&*button, setAccessibilityLabel: &*NSString::from_str(&format!("{}{}", accessible, if selected { ", 선택됨" } else { "" }))];
            }
            self.search_rows.push(SearchRow {
                id,
                button,
                title,
                detail,
            });
        }
    }

    fn clear_search_rows(&mut self) {
        for row in self.search_rows.drain(..) {
            row.button.removeFromSuperview();
            row.title.removeFromSuperview();
            row.detail.removeFromSuperview();
        }
        self.search_selection.setHidden(true);
    }

    fn layout_search(&self, size: NSSize) {
        let margin = 36.0;
        self.search_surface
            .setFrame(rect(0.0, 0.0, size.width, size.height));
        self.search_title
            .setFrame(rect(margin, size.height - 66.0, size.width - 150.0, 32.0));
        self.search_close
            .setFrame(rect(size.width - 94.0, size.height - 62.0, 58.0, 30.0));
        self.search_field.setFrame(rect(
            margin,
            size.height - 112.0,
            size.width - margin * 2.0,
            32.0,
        ));
        self.search_hint.setFrame(rect(
            margin,
            size.height - 182.0,
            size.width - margin * 2.0,
            22.0,
        ));
        let controls_y = size.height - 150.0;
        self.search_mode
            .setFrame(rect(margin, controls_y, 132.0, 28.0));
        self.history_clear
            .setFrame(rect(size.width - margin - 104.0, controls_y, 104.0, 28.0));
        self.history_delete
            .setFrame(rect(size.width - margin - 142.0, controls_y, 72.0, 28.0));
        self.history_cancel
            .setFrame(rect(size.width - margin - 62.0, controls_y, 62.0, 28.0));
        let height = (size.height - 214.0).max(1.0);
        self.search_scroll
            .setFrame(rect(margin, 24.0, size.width - margin * 2.0, height));
        let width = self.search_scroll.contentSize().width.max(1.0);
        self.search_document.setFrame(rect(
            0.0,
            0.0,
            width,
            (self.search_rows.len() as f64 * 62.0).max(height),
        ));
        self.search_selection.setHidden(true);
        for (index, row) in self.search_rows.iter().enumerate() {
            let top = index as f64 * 62.0;
            row.title
                .setFrame(rect(14.0, top + 9.0, width - 28.0, 21.0));
            row.detail
                .setFrame(rect(14.0, top + 33.0, width - 28.0, 18.0));
            row.button.setFrame(rect(0.0, top, width, 59.0));
            if self.search.selected == Some(row.id) {
                self.search_selection.setFrame(row.button.frame());
                self.search_selection.setHidden(false);
            }
        }
    }

    fn new(marker: MainThreadMarker) -> Self {
        let target = ShellTarget::new(marker);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(marker),
                rect(0.0, 0.0, 1180.0, 800.0),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(&NSString::from_str("Naru"));
        window.setContentMinSize(NSSize::new(800.0, 520.0));
        // Native rows and CEF views are added after the window is constructed.
        window.setAutorecalculatesKeyViewLoop(true);
        let root = SurfaceView::new(marker, SurfaceKind::Window, 0.0, false);
        root.setFrame(rect(0.0, 0.0, 1180.0, 800.0));
        window.setContentView(Some(&root));
        let sidebar = surface_child(&root, SurfaceKind::Sidebar, 0.0, false, marker);
        let toolbar = surface_child(&root, SurfaceKind::Toolbar, 0.0, false, marker);
        let content = child_view(&root, marker);
        content.setWantsLayer(true);
        let tab_document = TopDownView::new(marker);
        let selected_tab_background =
            surface_child(&tab_document, SurfaceKind::Selection, 10.0, false, marker);
        selected_tab_background.setHidden(true);
        let tab_drop_indicator =
            surface_child(&tab_document, SurfaceKind::Primary, 1.0, false, marker);
        tab_drop_indicator.setHidden(true);
        let tab_scroll = scroll_view(&sidebar, &tab_document, marker);
        let overview_document = TopDownView::new(marker);
        let overview_scroll = scroll_view(&root, &overview_document, marker);
        overview_scroll.setHidden(true);
        let overview_title = label(&overview_document, "공간", 24.0, true, marker);
        let overview_count_surface =
            surface_child(&overview_document, SurfaceKind::Badge, 10.0, false, marker);
        let overview_count = label(&overview_count_surface, "1개", 11.0, true, marker);
        let overview_hint = label(
            &overview_document,
            "직접 탐색과 AI 작업을 한곳에서",
            12.0,
            false,
            marker,
        );
        overview_hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let overview_previous = button(
            &overview_document,
            "←",
            &target,
            sel!(previousSpaces:),
            "이전 공간",
            marker,
        );
        let overview_next = button(
            &overview_document,
            "→",
            &target,
            sel!(nextSpaces:),
            "다음 공간",
            marker,
        );
        let overview_page_label = label(&overview_document, "", 11.0, false, marker);
        let spaces_background =
            surface_child(&sidebar, SurfaceKind::Selection, 10.0, false, marker);
        let spaces_button = button(
            &sidebar,
            "모든 공간",
            &target,
            sel!(showSpaces:),
            "모든 공간 보기",
            marker,
        );
        let sidebar_toggle = button(
            &sidebar,
            "‹",
            &target,
            sel!(toggleSidebar:),
            "탭 사이드바 접기",
            marker,
        );
        let search_button_background =
            surface_child(&sidebar, SurfaceKind::Card, 10.0, false, marker);
        search_button_background.setClipsToBounds(true);
        let search_button = button(
            &sidebar,
            "통합 검색",
            &target,
            sel!(showTabSearch:),
            "탭, 북마크, 방문 기록 통합 검색 (⌘⇧A)",
            marker,
        );
        let bookmarks_button = button(
            &sidebar,
            "북마크",
            &target,
            sel!(showBookmarks:),
            "공유 북마크",
            marker,
        );
        let downloads_button = button(
            &sidebar,
            "다운로드",
            &target,
            sel!(showDownloads:),
            "다운로드 목록",
            marker,
        );
        let site_permissions_button = button(
            &sidebar,
            "사이트 권한",
            &target,
            sel!(showSitePermissions:),
            "사이트 권한 설정과 대기 요청",
            marker,
        );
        let sidebar_separator = surface_child(&sidebar, SurfaceKind::Separator, 0.0, false, marker);
        let space_label = label(&sidebar, "내 공간", 11.0, true, marker);
        space_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let brand = label(&sidebar, "Naru", 18.0, true, marker);
        let brand_icon = symbol_view(&sidebar, "safari", "Naru", marker);
        brand_icon.setContentTintColor(Some(&accent_color()));
        let back = button(&toolbar, "←", &target, sel!(goBack:), "뒤로", marker);
        let forward = button(&toolbar, "→", &target, sel!(goForward:), "앞으로", marker);
        let reload = button(
            &toolbar,
            "↻",
            &target,
            sel!(reloadTab:),
            "새로 고침 (⌘R)",
            marker,
        );
        let address_surface = surface_child(&toolbar, SurfaceKind::Address, 10.0, false, marker);
        let address_icon = symbol_view(&address_surface, "globe", "페이지 주소", marker);
        let address =
            NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 28.0));
        address.setEditable(true);
        address.setSelectable(true);
        address.setBezeled(false);
        address.setBordered(false);
        address.setDrawsBackground(false);
        address.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        address.setTextColor(Some(&NSColor::labelColor()));
        address.setPlaceholderString(Some(&NSString::from_str("주소 또는 Google 검색")));
        address.setToolTip(Some(&NSString::from_str("주소 또는 Google 검색 (⌘L)")));
        unsafe {
            address.setTarget(Some(&*target));
            address.setAction(Some(sel!(navigate:)));
        }
        address_surface.addSubview(&address);
        let close_tab = button(
            &toolbar,
            "×",
            &target,
            sel!(closeTab:),
            "현재 탭 닫기 (⌘W)",
            marker,
        );
        let new_tab_background = surface_child(&sidebar, SurfaceKind::Primary, 10.0, false, marker);
        new_tab_background.setClipsToBounds(true);
        let new_tab = button(
            &sidebar,
            "새 탭",
            &target,
            sel!(newTab:),
            "현재 공간에 새 탭 (⌘T)",
            marker,
        );
        new_tab.setContentTintColor(Some(&NSColor::whiteColor()));
        new_tab.setFont(Some(&medium_font(13.0)));
        new_tab.setImageHugsTitle(true);
        let status_surface = surface_child(&toolbar, SurfaceKind::Badge, 12.0, false, marker);
        let status = label(&status_surface, "직접 사용 중", 11.0, false, marker);
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let stop = button(
            &toolbar,
            "중지",
            &target,
            sel!(stopTask:),
            "현재 공간의 AI 작업 중지",
            marker,
        );
        stop.setHidden(true);
        for control in [
            &spaces_button,
            &search_button,
            &bookmarks_button,
            &downloads_button,
            &site_permissions_button,
        ] {
            control.setFont(Some(&NSFont::systemFontOfSize(13.0)));
            control.setContentTintColor(Some(&NSColor::labelColor()));
        }
        search_button.setFont(Some(&medium_font(13.0)));
        for (control, symbol, fallback, hint) in [
            (&back, "chevron.left", "←", "뒤로"),
            (&forward, "chevron.right", "→", "앞으로"),
            (&reload, "arrow.clockwise", "↻", "새로 고침 (⌘R)"),
            (&close_tab, "xmark", "×", "현재 탭 닫기 (⌘W)"),
            (&sidebar_toggle, "sidebar.left", "‹", "탭 사이드바 접기"),
            (&overview_previous, "chevron.left", "←", "이전 공간"),
            (&overview_next, "chevron.right", "→", "다음 공간"),
        ] {
            set_symbol(control, symbol, "", fallback, hint);
        }
        set_symbol(
            &spaces_button,
            "square.grid.2x2",
            "모든 공간",
            "공간",
            "모든 공간 보기",
        );
        set_symbol(
            &new_tab,
            "plus",
            "새 탭",
            "+ 새 탭",
            "현재 공간에 새 탭 (⌘T)",
        );
        set_symbol(
            &stop,
            "stop.fill",
            "중지",
            "중지",
            "현재 공간의 AI 작업 중지",
        );
        stop.setContentTintColor(Some(&accent_color()));
        align_left(&spaces_button);
        align_left(&new_tab);
        let search_surface = surface_child(&root, SurfaceKind::Window, 0.0, true, marker);
        search_surface.setHidden(true);
        let search_title = label(&search_surface, "통합 검색", 24.0, true, marker);
        let search_hint = label(
            &search_surface,
            "열린 탭, 북마크, 방문 기록, 최대 40개",
            12.0,
            false,
            marker,
        );
        search_hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let search_field =
            NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 30.0));
        search_field.setEditable(true);
        search_field.setSelectable(true);
        search_field.setFont(Some(&NSFont::systemFontOfSize(15.0)));
        search_field.setPlaceholderString(Some(&NSString::from_str(
            "제목, URL 또는 공간 이름 (최대 200자)",
        )));
        search_field.setToolTip(Some(&NSString::from_str(
            "탭, 북마크, 방문 기록 검색, 방향키로 선택, Enter로 이동, Esc로 닫기",
        )));
        unsafe {
            search_field.setTarget(Some(&*target));
            search_field.setAction(Some(sel!(submitTabSearch:)));
            // The shell retains this weak delegate for the field's entire lifetime.
            let _: () = msg_send![&*search_field, setDelegate: &*target];
        }
        search_surface.addSubview(&search_field);
        let search_close = button(
            &search_surface,
            "닫기",
            &target,
            sel!(closeTabSearch:),
            "탭 검색 닫기 (Esc)",
            marker,
        );
        let search_mode = button(
            &search_surface,
            "방문 기록만 보기",
            &target,
            sel!(toggleSearchMode:),
            "통합 검색과 방문 기록 검색 전환",
            marker,
        );
        let history_clear = button(
            &search_surface,
            "모두 지우기",
            &target,
            sel!(prepareClearHistory:),
            "검색 결과뿐 아니라 방문 기록 전체 지우기",
            marker,
        );
        let history_delete = button(
            &search_surface,
            "삭제",
            &target,
            sel!(confirmClearHistory:),
            "방문 기록 전체 삭제 확인",
            marker,
        );
        let history_cancel = button(
            &search_surface,
            "취소",
            &target,
            sel!(cancelClearHistory:),
            "방문 기록 삭제 취소",
            marker,
        );
        history_delete.setContentTintColor(Some(&NSColor::systemRedColor()));
        for control in [&history_clear, &history_delete, &history_cancel] {
            control.setHidden(true);
        }
        let search_document = TopDownView::new(marker);
        let search_selection =
            surface_child(&search_document, SurfaceKind::Selection, 9.0, false, marker);
        search_selection.setHidden(true);
        let search_scroll = scroll_view(&search_surface, &search_document, marker);
        let bookmarks = BookmarkPanel::new(&root, &target, marker);
        let downloads = DownloadPanel::new(&root, &target, marker);
        let site_permissions = SitePermissionPanel::new(&root, &target, marker);
        let find = FindBar::new(&root, &target, marker);
        let recovery = PageRecoveryBar::new(&root, &target, marker);
        let dialog = PageDialogBar::new(&root, &target, marker);
        let start_page = StartPage::new(&root, &target, marker);
        window.center();
        window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        install_menu(&target, marker);
        Self {
            window,
            root,
            sidebar,
            tab_scroll,
            tab_document,
            toolbar,
            content,
            overview_scroll,
            overview_document,
            overview_title,
            overview_hint,
            overview_count,
            overview_count_surface,
            overview_previous,
            overview_next,
            overview_page_label,
            spaces_button,
            spaces_background,
            space_label,
            brand,
            brand_icon,
            sidebar_toggle,
            search_button,
            search_button_background,
            sidebar_separator,
            bookmarks_button,
            bookmarks,
            downloads_button,
            downloads,
            site_permissions_button,
            site_permissions,
            site_permission_count: 0,
            site_permission_notice: None,
            site_permission_media: None,
            download_activity: (0, 0),
            download_permission_count: 0,
            file_selection_count: 0,
            file_selection_notice: None,
            find,
            recovery,
            dialog,
            start_page,
            search_surface,
            search_title,
            search_hint,
            search_field,
            search_close,
            search_mode,
            history_clear,
            history_delete,
            history_cancel,
            history_clear_available: false,
            search_scroll,
            search_document,
            search_selection,
            search_rows: Vec::new(),
            search_results: Vec::new(),
            search: SearchState::default(),
            search_signature: String::new(),
            back,
            forward,
            reload,
            address,
            address_surface,
            address_icon,
            close_tab,
            new_tab,
            new_tab_background,
            status,
            status_surface,
            stop,
            tab_buttons: Vec::new(),
            selected_tab_background,
            tab_drop_indicator,
            cards: Vec::new(),
            views: BTreeMap::new(),
            tabs: Vec::new(),
            spaces: Vec::new(),
            active_space: String::new(),
            active: None,
            overview: false,
            overview_page: 0,
            sidebar_collapsed: false,
            reveal_active_tab: true,
            tab_signature: String::new(),
            tab_focus_removed: false,
            card_signature: String::new(),
            displayed_address: None,
            address_draft: None,
            sidebar_address_handoff: None,
            notice: None,
            target,
        }
    }

    fn render(&mut self, snapshot: ShellSnapshot) {
        if self.active_space != snapshot.active_space
            || self.active != snapshot.active_tab
            || self.overview != snapshot.overview
        {
            self.address_draft = None;
        }
        if self.active_space != snapshot.active_space || self.active != snapshot.active_tab {
            self.notice = None;
            self.reveal_active_tab = true;
        }
        if let (Some(handoff), Some(draft)) =
            (&self.sidebar_address_handoff, &mut self.address_draft)
        {
            // Handoff expires automation refs and synchronously renders before
            // close returns. Rebase only its one ownership epoch; a real new
            // navigation adds another epoch and keeps the usual draft rules.
            handoff.rebase(draft, &snapshot);
        }
        if self.overview && !snapshot.overview {
            self.clear_cards();
            self.card_signature.clear();
        }
        self.tabs = snapshot.tabs;
        self.spaces = snapshot.spaces;
        self.active_space = snapshot.active_space;
        self.active = snapshot.active_tab;
        self.overview = snapshot.overview;
        self.overview_page = self
            .overview_page
            .min(self.spaces.len().saturating_sub(1) / PREVIEW_LIMIT);
        self.render_controls();
        if self.overview {
            self.rebuild_cards();
        }
    }

    fn render_controls(&mut self) {
        self.rebuild_tab_buttons();
        let tab = self.tabs.iter().find(|tab| Some(tab.id) == self.active);
        let start_target = start_page_target(&self.tabs, &self.active_space, self.active);
        self.start_page
            .render(start_target, !self.overview && !self.overlay_is_open());
        let browsing = !self.overview && tab.is_some();
        let recovery_target = visible_page_recovery_target(
            &self.tabs,
            &self.active_space,
            self.active,
            self.overview,
            self.overlay_is_open(),
        );
        self.recovery
            .render(recovery_target, tab.is_some_and(|tab| tab.loading));
        // Only the shown tab's user-owned dialog is presented; any other tab's
        // dialog waits (and the page keeps rejecting commands) until it is shown.
        let dialog = visible_user_dialog_tab(
            &self.tabs,
            &self.active_space,
            self.active,
            self.overview,
            self.overlay_is_open(),
        )
        .and_then(crate::dialog_runtime::user_dialog);
        self.dialog.render(dialog);
        self.back
            .setEnabled(browsing && tab.is_some_and(|tab| tab.can_go_back));
        self.forward
            .setEnabled(browsing && tab.is_some_and(|tab| tab.can_go_forward));
        self.reload.setEnabled(browsing && !self.overlay_is_open());
        let loading = tab.is_some_and(|tab| tab.loading);
        set_symbol(
            &self.reload,
            if loading { "xmark" } else { "arrow.clockwise" },
            "",
            if loading { "×" } else { "↻" },
            if loading {
                "로딩 중지 (⌘R)"
            } else {
                "새로 고침 (⌘R)"
            },
        );
        self.close_tab.setEnabled(browsing);
        self.address.setEnabled(browsing);
        // Background state updates must not replace an address being typed.
        let editor: *mut AnyObject = unsafe { msg_send![&*self.address, currentEditor] };
        let hold_address = NAVIGATION_CONTROL_CALL.with(|slot| {
            slot.borrow().as_ref().is_some_and(|call| {
                call.preserve_address
                    && std::ptr::eq(&*call.address, &*self.address)
                    && navigation_control_page_matches(
                        &call.target,
                        &call.url,
                        tab,
                        &self.active_space,
                        self.active,
                        self.overview,
                    )
            })
        });
        if !hold_address
            && tab.is_some_and(|tab| {
                self.address_draft.as_ref().is_some_and(|draft| {
                    draft.superseded(tab.id, &tab.url, tab.navigation_epoch, !editor.is_null())
                })
            })
        {
            // Once editing ends, a later page navigation owns the address again.
            self.address_draft = None;
            self.notice = None;
        }
        if !hold_address
            && (self.overview
                || self.displayed_address != self.active
                || (editor.is_null()
                    && !self
                        .address_draft
                        .as_ref()
                        .is_some_and(|draft| Some(draft.tab) == self.active)))
        {
            self.address
                .setStringValue(&NSString::from_str(if self.overview {
                    "공간 둘러보기"
                } else {
                    tab.map(|tab| tab.url.as_str())
                        .filter(|url| *url != "about:blank")
                        .unwrap_or("")
                }));
            self.displayed_address = self.active;
        }
        let space = self
            .spaces
            .iter()
            .find(|space| space.id == self.active_space);
        let can_stop = !self.overview && space.and_then(space_stop_scope).is_some();
        let can_resume =
            !self.overview && !can_stop && space.and_then(space_resume_scope).is_some();
        // The one toolbar button stops an active task or, once paused, returns
        // it to the AI. The tag records which role it currently has.
        let resume_role: isize = unsafe { msg_send![&*self.stop, tag] };
        if can_resume != (resume_role == 1) {
            if can_resume {
                set_symbol(&self.stop, "play.fill", "", "재개", "AI에게 돌려주기");
                unsafe { self.stop.setAction(Some(sel!(resumeTask:))) };
            } else {
                set_symbol(
                    &self.stop,
                    "stop.fill",
                    "중지",
                    "중지",
                    "현재 공간의 AI 작업 중지",
                );
                unsafe { self.stop.setAction(Some(sel!(stopTask:))) };
            }
            unsafe {
                let _: () = msg_send![&*self.stop, setTag: isize::from(can_resume)];
            }
        }
        self.stop.setEnabled(can_stop || can_resume);
        self.stop.setHidden(!(can_stop || can_resume));
        let text = if let Some(notice) = &self.notice {
            compact(notice, 100)
        } else if self.overview {
            "공간 보기".into()
        } else if tab.is_some_and(|tab| tab.dialog_user) {
            "대화상자: 직접 응답".into()
        } else if tab.is_some_and(|tab| tab.dialog_pending) {
            "대화상자 대기".into()
        } else if can_stop {
            "AI 제어 중".into()
        } else if let Some(tab) = tab {
            if tab.crashed {
                "페이지 복구 필요".into()
            } else if tab.stop_scope.is_some() {
                "AI 제어 중".into()
            } else if tab.loading {
                "불러오는 중".into()
            } else if tab.task_status.contains("중지") {
                "AI 중지됨".into()
            } else {
                "직접 사용 중".into()
            }
        } else if space.is_some_and(|space| space.status == "paused") {
            "AI 중지됨".into()
        } else {
            "새 탭을 열어 시작하세요".into()
        };
        self.status.setStringValue(&NSString::from_str(&text));
        self.status_surface.set_kind(if can_stop {
            SurfaceKind::AccentBadge
        } else {
            SurfaceKind::Badge
        });
        let status_color = if can_stop {
            accent_color()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.status.setTextColor(Some(&status_color));
        self.overview_count
            .setStringValue(&NSString::from_str(&format!("{}개", self.spaces.len())));
        self.spaces_background.setHidden(!self.overview);
        self.brand_icon.setHidden(self.sidebar_collapsed);
        let status_detail = self
            .notice
            .as_deref()
            .or_else(|| tab.map(|tab| tab.task_status.as_str()))
            .unwrap_or(&text);
        self.status
            .setToolTip(Some(&NSString::from_str(status_detail)));
        unsafe {
            let _: () =
                msg_send![&*self.status, setAccessibilityHelp: &*NSString::from_str(status_detail)];
        }
        let space_name = self
            .spaces
            .iter()
            .find(|space| space.id == self.active_space)
            .map(|space| space.name.as_str())
            .unwrap_or("내 공간");
        self.space_label
            .setStringValue(&NSString::from_str(&compact(space_name, 22)));
        self.space_label
            .setToolTip(Some(&NSString::from_str(space_name)));
        self.space_label.setHidden(self.sidebar_collapsed);
        self.brand.setHidden(self.sidebar_collapsed);
        set_symbol(
            &self.spaces_button,
            "square.grid.2x2",
            if self.sidebar_collapsed {
                ""
            } else {
                "모든 공간"
            },
            "공간",
            if self.overview {
                "공간 닫기"
            } else {
                "모든 공간 보기"
            },
        );
        let spaces_color = if self.overview {
            accent_color()
        } else {
            NSColor::labelColor()
        };
        self.spaces_button.setContentTintColor(Some(&spaces_color));
        set_symbol(
            &self.sidebar_toggle,
            "sidebar.left",
            "",
            if self.sidebar_collapsed { "›" } else { "‹" },
            if self.sidebar_collapsed {
                "탭 사이드바 펼치기"
            } else {
                "탭 사이드바 접기"
            },
        );
        set_symbol(
            &self.new_tab,
            "plus",
            if self.sidebar_collapsed {
                ""
            } else {
                "새 탭"
            },
            "+",
            "현재 공간에 새 탭 (⌘T)",
        );
        set_symbol(
            &self.search_button,
            "magnifyingglass",
            if self.sidebar_collapsed {
                ""
            } else {
                "통합 검색"
            },
            "검색",
            "탭, 북마크, 방문 기록 통합 검색 (⌘⇧A)",
        );
        set_symbol(
            &self.bookmarks_button,
            "bookmark",
            if self.sidebar_collapsed {
                ""
            } else {
                "북마크"
            },
            "북마크",
            "공유 북마크",
        );
        let (active_downloads, pending_downloads) = self.download_activity;
        let permission_count = self.download_permission_count;
        let file_count = self.file_selection_count;
        let file_notice = self
            .file_selection_notice
            .as_deref()
            .filter(|_| file_count == 0);
        let download_title = if file_notice.is_some() {
            let title = if self.sidebar_collapsed {
                "!"
            } else {
                "파일 선택 안내"
            };
            Some(title.to_owned())
        } else {
            (active_downloads > 0 || permission_count > 0 || file_count > 0).then(|| {
                if self.sidebar_collapsed {
                    (active_downloads + permission_count + file_count).to_string()
                } else if file_count > 0 {
                    format!("파일 선택 {file_count}")
                } else if permission_count > 0 {
                    format!("다운로드 {active_downloads}, 허용 {permission_count}")
                } else {
                    format!("다운로드 {active_downloads}")
                }
            })
        };
        let show_download_hint =
            active_downloads > 0 || permission_count > 0 || file_count > 0 || file_notice.is_some();
        let download_hint = show_download_hint.then(|| {
            let notice = file_notice
                .map(|notice| format!("파일 선택 안내: {notice}, "))
                .unwrap_or_default();
            format!(
                "{notice}다운로드 목록, 파일 선택 요청 {file_count}개, 여러 파일 다운로드 허용 대기 {permission_count}개, 저장 위치 선택 대기 {pending_downloads}개, 전체 진행 {active_downloads}개"
            )
        });
        set_symbol(
            &self.downloads_button,
            "arrow.down.circle",
            download_title
                .as_deref()
                .unwrap_or(if self.sidebar_collapsed {
                    ""
                } else {
                    "다운로드"
                }),
            download_title.as_deref().unwrap_or("다운로드"),
            download_hint.as_deref().unwrap_or("다운로드 목록"),
        );
        let downloads_color = if (pending_downloads > 0
            || permission_count > 0
            || file_count > 0
            || file_notice.is_some())
            && !self.downloads.open
        {
            accent_color()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.downloads_button
            .setContentTintColor(Some(&downloads_color));
        let permission_count = self.site_permission_count;
        let permission_title = if self.sidebar_collapsed {
            if permission_count > 0 {
                permission_count.to_string()
            } else if self.site_permission_media.is_some() {
                "활성".to_owned()
            } else if self.site_permission_notice.is_some() {
                "!".to_owned()
            } else {
                String::new()
            }
        } else if permission_count > 0 {
            format!("사이트 권한 {permission_count}")
        } else if self.site_permission_media.is_some() {
            "미디어 활성".to_owned()
        } else if self.site_permission_notice.is_some() {
            "권한 안내".to_owned()
        } else {
            "사이트 권한".to_owned()
        };
        let permission_hint = format!(
            "사이트 권한 설정, 승인 대기 {permission_count}개{}{}",
            self.site_permission_notice
                .as_deref()
                .map(|notice| format!(", {notice}"))
                .unwrap_or_default(),
            self.site_permission_media
                .as_deref()
                .map(|media| format!(", {media}"))
                .unwrap_or_default()
        );
        set_symbol(
            &self.site_permissions_button,
            "hand.raised",
            &permission_title,
            "사이트 권한",
            &permission_hint,
        );
        let permission_color = if (permission_count > 0
            || self.site_permission_media.is_some()
            || self.site_permission_notice.is_some())
            && !self.site_permissions.open
        {
            accent_color()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.site_permissions_button
            .setContentTintColor(Some(&permission_color));
        if self.sidebar_collapsed {
            align_center(&self.spaces_button);
            align_center(&self.search_button);
            align_center(&self.bookmarks_button);
            align_center(&self.downloads_button);
            align_center(&self.site_permissions_button);
        } else {
            align_left(&self.spaces_button);
            align_left(&self.search_button);
            align_left(&self.bookmarks_button);
            align_left(&self.downloads_button);
            align_left(&self.site_permissions_button);
        }
        align_center(&self.new_tab);
        let title = if self.overview {
            "공간 - Naru".into()
        } else {
            tab.map(|tab| compact(tab_search::display_title(&tab.title, &tab.url), 80))
                .filter(|title| !title.is_empty())
                .map(|title| format!("{title} - Naru"))
                .unwrap_or_else(|| "Naru".into())
        };
        self.window.setTitle(&NSString::from_str(&title));
    }

    fn rebuild_tab_buttons(&mut self) {
        let visible: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| tab.space == self.active_space)
            .collect();
        let overlay_open = self.overlay_is_open();
        let signature = tab_rows_signature(
            &self.active_space,
            self.active,
            self.sidebar_collapsed,
            self.overview,
            overlay_open,
            &visible,
        );
        if self.tab_signature == signature {
            return;
        }
        self.tab_signature = signature;
        // Keep the same controls through loading/title/selection changes. A tab
        // moving to another space must not inherit an old row's pending action.
        let mut previous: BTreeMap<_, _> = std::mem::take(&mut self.tab_buttons)
            .into_iter()
            .map(|row| (row.id, row))
            .collect();
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        for (index, tab) in visible.into_iter().enumerate() {
            let name = tab_search::display_title(&tab.title, &tab.url);
            let selected = Some(tab.id) == self.active;
            let symbol = if tab.dialog_pending {
                "exclamationmark.bubble"
            } else {
                tab_status_symbol(
                    tab.failure,
                    tab.audio_muted,
                    tab.stop_scope.is_some(),
                    tab.loading,
                )
            };
            let status = tab_status_description(tab);
            let row = previous
                .get(&tab.id)
                .filter(|row| row.space == tab.space)
                .map(|_| tab.id)
                .and_then(|id| previous.remove(&id))
                .unwrap_or_else(|| {
                    let icon = symbol_view(&self.tab_document, symbol, name, marker);
                    let title = label(&self.tab_document, "", 13.0, false, marker);
                    let select_button = button(
                        &self.tab_document,
                        "",
                        &self.target,
                        sel!(selectTab:),
                        "",
                        marker,
                    );
                    let drag = TabDragRecognizer::new(&self.target, marker);
                    // AppKit delays the button's tracking until pan recognition
                    // fails. A plain click still uses selectTab: unchanged.
                    drag.setDelaysPrimaryMouseButtonEvents(true);
                    select_button.addGestureRecognizer(&drag);
                    let mute = button(
                        &self.tab_document,
                        "",
                        &self.target,
                        sel!(toggleTabRowAudio:),
                        "",
                        marker,
                    );
                    let close = button(
                        &self.tab_document,
                        "",
                        &self.target,
                        sel!(closeTabRow:),
                        "",
                        marker,
                    );
                    set_symbol(&close, "xmark", "", "×", "탭 닫기");
                    unsafe {
                        let _: () = msg_send![&*select_button, setTag: tab.id as isize];
                        let _: () = msg_send![&*mute, setTag: tab.id as isize];
                        let _: () = msg_send![&*close, setTag: tab.id as isize];
                    }
                    TabRow {
                        id: tab.id,
                        space: tab.space.clone(),
                        button: select_button,
                        drag,
                        mute,
                        close,
                        icon,
                        title,
                    }
                });
            row.icon.setImage(symbol_image(symbol).as_deref());
            let icon_color = if selected || tab.stop_scope.is_some() {
                accent_color()
            } else {
                NSColor::secondaryLabelColor()
            };
            row.icon.setContentTintColor(Some(&icon_color));
            row.icon
                .setToolTip(Some(&NSString::from_str(&format!("{name}, {status}"))));
            row.title
                .setStringValue(&NSString::from_str(&compact(name, 60)));
            let font = if selected {
                medium_font(13.0)
            } else {
                NSFont::systemFontOfSize(13.0)
            };
            row.title.setFont(Some(&font));
            row.title.setHidden(self.sidebar_collapsed);
            row.button.setTitle(&NSString::from_str(
                if self.sidebar_collapsed && row.icon.image().is_none() {
                    if tab.failure.is_some() { "!" } else { "□" }
                } else {
                    ""
                },
            ));
            row.button.setEnabled(!overlay_open);
            row.mute.setHidden(self.sidebar_collapsed);
            row.mute
                .setEnabled(!overlay_open && !self.overview && !tab.crashed);
            row.close.setEnabled(!overlay_open && !self.overview);
            let (mute_action, mute_state) = match tab.audio_muted {
                Some(true) => ("음소거 해제", "음소거 설정됨"),
                Some(false) => ("음소거", "음소거 설정 안 됨"),
                None => ("음소거 설정 전환", "현재 설정 확인 전"),
            };
            set_symbol(
                &row.mute,
                if tab.audio_muted == Some(true) {
                    "speaker.slash"
                } else {
                    "speaker"
                },
                "",
                if tab.audio_muted == Some(true) {
                    "음"
                } else {
                    "소리"
                },
                mute_action,
            );
            let mute_color = if tab.audio_muted == Some(true) {
                accent_color()
            } else {
                NSColor::secondaryLabelColor()
            };
            row.mute.setContentTintColor(Some(&mute_color));
            for (control, hint, accessible) in [
                (
                    &row.button,
                    format!("{name}\n{}\n{status}", tab.url),
                    format!(
                        "{}번째 탭: {name}{}, {status}",
                        index + 1,
                        if selected { ", 선택됨" } else { "" },
                    ),
                ),
                (
                    &row.mute,
                    format!(
                        "{mute_action}: {name}\n{mute_state}\n현재 설정을 확인한 뒤 전환합니다. 탭 선택과 AI 제어권을 유지합니다."
                    ),
                    format!("{}번째 탭 {mute_action}: {name}, {mute_state}", index + 1),
                ),
                (
                    &row.close,
                    format!("탭 닫기: {name}\n{}", tab.url),
                    format!("{}번째 탭 닫기: {name}", index + 1,),
                ),
            ] {
                control.setToolTip(Some(&NSString::from_str(&hint)));
                unsafe {
                    let _: () = msg_send![&**control, setAccessibilityLabel: &*NSString::from_str(&accessible)];
                }
            }
            self.tab_buttons.push(row);
        }
        self.tab_focus_removed |= self
            .window
            .firstResponder()
            .as_ref()
            .and_then(|responder| responder.downcast_ref::<NSView>())
            .is_some_and(|responder| {
                previous.values().any(|row| {
                    responder.isDescendantOf(&row.button)
                        || responder.isDescendantOf(&row.mute)
                        || responder.isDescendantOf(&row.close)
                })
            });
        for row in previous.into_values() {
            row.button.removeFromSuperview();
            row.mute.removeFromSuperview();
            row.close.removeFromSuperview();
            row.icon.removeFromSuperview();
            row.title.removeFromSuperview();
        }
    }

    fn clear_cards(&mut self) {
        for card in self.cards.drain(..) {
            card.image.setImage(None);
            card.container.removeFromSuperview();
        }
    }

    fn rebuild_cards(&mut self) {
        let spaces: Vec<_> = self
            .spaces
            .iter()
            .skip(self.overview_page * PREVIEW_LIMIT)
            .take(PREVIEW_LIMIT)
            .collect();
        let signature = format!(
            "{}:{:?}",
            self.overview_page,
            spaces
                .iter()
                .map(|space| (
                    &space.id,
                    &space.name,
                    &space.status,
                    &space.tabs,
                    space.selected_tab,
                    space.active,
                    space.revision,
                    space
                        .tabs
                        .iter()
                        .filter(|id| self
                            .tabs
                            .iter()
                            .any(|tab| tab.id == **id && tab.dialog_pending))
                        .count(),
                    space
                        .selected_tab
                        .or_else(|| space.tabs.first().copied())
                        .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
                        .map(|tab| (&tab.title, &tab.url, tab.navigation_epoch))
                ))
                .collect::<Vec<_>>()
        );
        if self.card_signature == signature {
            return;
        }
        self.card_signature = signature;
        // Cache only the visible page. Titles/status changes may reuse its small images.
        let mut previous = std::mem::take(&mut self.cards);
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        for (index, space) in spaces.into_iter().enumerate() {
            let container = surface_child(
                &self.overview_document,
                if space.active {
                    SurfaceKind::SelectedCard
                } else {
                    SurfaceKind::Card
                },
                14.0,
                true,
                marker,
            );
            let preview_surface =
                surface_child(&container, SurfaceKind::Preview, 9.0, false, marker);
            let selected = space.selected_tab.or_else(|| space.tabs.first().copied());
            let representative = selected.and_then(|id| self.tabs.iter().find(|tab| tab.id == id));
            let image =
                NSImageView::initWithFrame(NSImageView::alloc(marker), rect(0.0, 0.0, 1.0, 1.0));
            unsafe {
                let _: () = msg_send![&*image, setImageScaling: 3usize];
            }
            preview_surface.addSubview(&image);
            let placeholder = label(
                &preview_surface,
                if space.tabs.is_empty() {
                    "탭이 없는 공간"
                } else if representative.is_some_and(|tab| tab.url == "about:blank") {
                    "새 탭"
                } else {
                    "웹페이지 미리보기"
                },
                12.0,
                false,
                marker,
            );
            placeholder.setTextColor(Some(&NSColor::secondaryLabelColor()));
            unsafe {
                let _: () = msg_send![&*placeholder, setAlignment: 1isize];
            }
            let title = label(&container, &compact(&space.name, 26), 14.0, true, marker);
            title.setToolTip(Some(&NSString::from_str(&space.name)));
            let representative_title =
                representative
                    .map(|tab| tab.title.as_str())
                    .filter(|title| {
                        !title.is_empty() && *title != space.name && *title != "about:blank"
                    });
            let dialogs = space
                .tabs
                .iter()
                .filter(|id| {
                    self.tabs
                        .iter()
                        .any(|tab| tab.id == **id && tab.dialog_pending)
                })
                .count();
            let mut detail_text = representative_title
                .map(|title| format!("탭 {}개, {}", space.tabs.len(), compact(title, 48)))
                .unwrap_or_else(|| format!("탭 {}개", space.tabs.len()));
            if dialogs > 0 {
                detail_text.push_str(&format!(" · 대화상자 대기 {dialogs}"));
            }
            let detail = label(&container, &detail_text, 11.0, false, marker);
            detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
            let badge = surface_child(
                &container,
                if space.status == "active" && space.kind == "task" {
                    SurfaceKind::AccentBadge
                } else {
                    SurfaceKind::Badge
                },
                9.0,
                false,
                marker,
            );
            let badge_label = label(
                &badge,
                space_status(&space.kind, &space.status),
                10.0,
                false,
                marker,
            );
            let badge_color = if space.status == "active" && space.kind == "task" {
                accent_color()
            } else {
                NSColor::secondaryLabelColor()
            };
            badge_label.setTextColor(Some(&badge_color));
            let open = button(
                &container,
                "",
                &self.target,
                sel!(selectSpace:),
                &format!("{} 공간 열기", space.name),
                marker,
            );
            set_symbol(
                &open,
                "arrow.right",
                "",
                "→",
                &format!("{} 공간 열기", space.name),
            );
            open.setContentTintColor(Some(&accent_color()));
            let preview_button = button(
                &container,
                "",
                &self.target,
                sel!(selectSpace:),
                &format!("{} 미리보기 열기", space.name),
                marker,
            );
            let stop = button(
                &container,
                "중지",
                &self.target,
                sel!(stopSpace:),
                &format!("{} AI 작업 중지", space.name),
                marker,
            );
            set_symbol(
                &stop,
                "stop.fill",
                "중지",
                "중지",
                &format!("{} AI 작업 중지", space.name),
            );
            stop.setContentTintColor(Some(&accent_color()));
            stop.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            if space_resume_scope(space).is_some() {
                set_symbol(
                    &stop,
                    "play.fill",
                    "",
                    "재개",
                    &format!("{} AI에게 돌려주기", space.name),
                );
                unsafe { stop.setAction(Some(sel!(resumeSpace:))) };
                stop.setHidden(false);
            } else {
                stop.setHidden(space_stop_scope(space).is_none());
            }
            unsafe {
                let _: () = msg_send![&*open, setTag: index as isize];
                let _: () = msg_send![&*preview_button, setTag: index as isize];
                let _: () = msg_send![&*stop, setTag: index as isize];
            }
            let preview = previous
                .iter_mut()
                .find(|card| {
                    card.space == space.id
                        && card.tab == selected
                        && card.url == representative.map(|tab| tab.url.as_str()).unwrap_or("")
                        && card.navigation_epoch
                            == representative.map(|tab| tab.navigation_epoch).unwrap_or(0)
                })
                .and_then(|card| card.preview.take());
            image.setImage(preview.as_deref());
            placeholder.setHidden(preview.is_some());
            self.cards.push(SpaceCard {
                space: space.id.clone(),
                tab: selected,
                url: representative
                    .map(|tab| tab.url.clone())
                    .unwrap_or_default(),
                navigation_epoch: representative.map(|tab| tab.navigation_epoch).unwrap_or(0),
                container,
                preview_surface,
                image,
                placeholder,
                title,
                detail,
                badge,
                badge_label,
                open,
                preview_button,
                stop,
                preview,
            });
        }
        for card in previous {
            card.image.setImage(None);
            card.container.removeFromSuperview();
        }
        let pages = self.spaces.len().max(1).div_ceil(PREVIEW_LIMIT);
        self.overview_previous.setEnabled(self.overview_page > 0);
        self.overview_next
            .setEnabled(self.overview_page + 1 < pages);
        self.overview_page_label
            .setStringValue(&NSString::from_str(&format!(
                "{} / {}",
                self.overview_page + 1,
                pages
            )));
        self.overview_previous.setHidden(pages <= 1);
        self.overview_next.setHidden(pages <= 1);
        self.overview_page_label.setHidden(pages <= 1);
    }
}

fn ensure_shell() {
    let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
    if SHELL.with(|slot| slot.borrow().is_some()) {
        return;
    }
    // Creating an AppKit window can deliver delegate callbacks synchronously.
    let shell = Shell::new(marker);
    let window = shell.window.clone();
    SHELL.with(|slot| *slot.borrow_mut() = Some(shell));
    layout();
    // Seed geometric traversal after the initial frames are set. Later view
    // additions and visibility transitions update it without a shell borrow.
    window.recalculateKeyViewLoop();
}

/// Parent for every CEF child browser. Owned until shutdown; never a popup slot.
pub fn create_parent() -> Result<*mut c_void, String> {
    MainThreadMarker::new().ok_or("Browser shell requires the main thread")?;
    ensure_shell();
    SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|shell| view_pointer(&shell.content))
            .ok_or_else(|| "Browser shell is unavailable".to_owned())
    })
}

pub fn browser_bounds() -> cef::Rect {
    SHELL.with(|slot| {
        let bounds = slot
            .borrow()
            .as_ref()
            .map(|shell| shell.content.bounds())
            .unwrap_or_else(|| rect(0.0, 0.0, 1120.0, 668.0));
        cef::Rect {
            x: 0,
            y: 0,
            width: bounds.size.width.max(1.0) as i32,
            height: bounds.size.height.max(1.0) as i32,
        }
    })
}

/// Register a live CEF view after OnAfterCreated. Call on the main thread.
///
/// # Safety
/// `view` must be a live CEF NSView, and `parent` must be create_parent's result.
pub unsafe fn attach(
    id: i32,
    parent: *mut c_void,
    view: *mut c_void,
    _activate: bool,
) -> Result<(), String> {
    MainThreadMarker::new().ok_or("Browser shell requires the main thread")?;
    let native =
        unsafe { Retained::retain(view.cast::<NSView>()) }.ok_or("Browser has no native view")?;
    let accepted = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut().ok_or("Browser shell is unavailable")?;
        if view_pointer(&shell.content) != parent {
            return Err("Browser parent does not match the shell");
        }
        if shell.views.contains_key(&id) {
            return Err("Browser tab is already attached");
        }
        shell.views.insert(id, native.clone());
        Ok(())
    });
    accepted.map_err(str::to_owned)?;
    let bounds = browser_bounds();
    native.setFrame(rect(
        0.0,
        0.0,
        f64::from(bounds.width),
        f64::from(bounds.height),
    ));
    sync();
    Ok(())
}

/// Remove the child hierarchy before CEF's final OnBeforeClose callback.
/// No shell borrow is held while AppKit/CEF may dispatch close callbacks.
pub fn detach_view(id: i32) -> bool {
    let has_find = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.find.state.target.is_some_and(|(tab, _)| tab == id))
    });
    if has_find {
        close_page_find(false, false);
    }
    let entry = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        shell.views.remove(&id)
    });
    if let Some(view) = entry {
        clear_view_responders(&view);
        view.removeFromSuperview();
        true
    } else {
        false
    }
}

/// Tear down a browser rejected before it could be registered with this shell.
///
/// # Safety
/// `handle` must be null or a live CEF NSView, on the main thread.
pub unsafe fn detach_native_view(handle: *mut c_void) {
    if let Some(view) = unsafe { handle.cast::<NSView>().as_ref() } {
        clear_view_responders(view);
        view.removeFromSuperview();
    }
}

fn clear_view_responders(view: &NSView) {
    if let Some(window) = view.window() {
        // These are weak references. Clear stale input destinations, without
        // treating them as the cause of any outstanding native view ownership.
        if window
            .initialFirstResponder()
            .is_some_and(|initial| initial.isDescendantOf(view))
        {
            window.setInitialFirstResponder(None);
        }
        // The remembered active tab may sit behind search or bookmark chrome.
        // Only detach this view's responder; preserve any native editor/button
        // already focused elsewhere in the same window.
        if window.firstResponder().is_some_and(|responder| {
            responder
                .downcast_ref::<NSView>()
                .is_some_and(|responder| responder.isDescendantOf(view))
        }) {
            window.makeFirstResponder(None);
        }
    }
}

/// Final bookkeeping after CEF OnBeforeClose; safe after detach_view.
pub fn remove(id: i32) {
    detach_view(id);
    sync();
    crate::browser::notify_panel();
}

pub fn active_tab() -> Option<i32> {
    SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|shell| if shell.overview { None } else { shell.active })
    })
}

pub(crate) fn search_is_open() -> bool {
    SHELL.with(|slot| {
        slot.try_borrow()
            .map(|slot| slot.as_ref().is_some_and(|shell| shell.overlay_is_open()))
            .unwrap_or(true)
    })
}

fn bookmarks_are_open() -> bool {
    SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.bookmarks.state.open)
    })
}

fn downloads_are_open() -> bool {
    SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.downloads.open)
    })
}

fn site_permissions_are_open() -> bool {
    SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref()
                .is_some_and(|shell| shell.site_permissions.open)
        })
    })
}

fn open_site_permissions() {
    if page_find_has_marked_text() {
        return;
    }
    preserve_address_for_overlay();
    close_page_find(false, false);
    close_tab_search(false);
    close_bookmarks(false);
    close_downloads(false);
    let controls = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.site_permissions.open {
            shell.site_permissions.notice = None;
        }
        shell.site_permissions.open = true;
        Some((
            shell.window.clone(),
            shell.site_permissions.close.clone(),
            shell.content.clone(),
        ))
    });
    if let Some((window, close, content)) = controls {
        // The covered CEF view stays visible to Blink. Release its native input
        // destination before sync can reenter CEF, then focus chrome after layout.
        clear_view_responders(&content);
        sync();
        // Opening the panel is explicit; never preselect an approval button.
        window.makeFirstResponder(Some(&close));
    }
}

fn close_site_permissions(restore_focus: bool) {
    let window = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.site_permissions.open {
            return None;
        }
        shell.site_permissions.open = false;
        shell.site_permissions.notice = None;
        Some(shell.window.clone())
    });
    if let Some(window) = window {
        window.makeFirstResponder(None);
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.site_permissions.clear_rows();
            }
        });
        sync();
        apply_selection(restore_focus);
    }
}

fn site_permission_panel_matches(window: &NSWindow, surface: &NSView) -> bool {
    SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                let current: &NSView = &shell.site_permissions.surface;
                shell.site_permissions.open
                    && !shell.search.open
                    && !shell.bookmarks.state.open
                    && !shell.downloads.open
                    && std::ptr::eq(&*shell.window, window)
                    && std::ptr::eq(current, surface)
            })
        })
    })
}

fn site_permission_controls() -> Option<(Retained<NSWindow>, Retained<SurfaceView>)> {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.site_permissions.open)?;
        Some((shell.window.clone(), shell.site_permissions.surface.clone()))
    })?;
    let (window, surface) = &controls;
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || surface.window().as_deref() != Some(&**window)
        || surface.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
        || !site_permission_panel_matches(window, surface)
    {
        return None;
    }
    Some(controls)
}

#[derive(Clone, Copy)]
enum SitePermissionAction {
    Allow,
    Cancel,
    Reveal,
}

fn site_permission_action(sender: &NSButton, action: SitePermissionAction) {
    let Some((window, surface)) = site_permission_controls() else {
        return;
    };
    // Native getters above can reenter. Resolve the exact current visible button
    // afterwards; a queued action may never decide a replacement request.
    let token = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.site_permissions.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && !shell.downloads.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.site_permissions.surface, &*surface)
        })?;
        shell.site_permissions.rows.iter().find_map(|row| {
            let control = match action {
                SitePermissionAction::Allow => &row.allow,
                SitePermissionAction::Cancel => &row.cancel,
                SitePermissionAction::Reveal => &row.reveal,
            };
            std::ptr::eq(sender, &**control).then_some(row.token)
        })
    });
    let Some(token) = token else { return };
    // A CEF callback can synchronously dismiss requests and refresh this panel.
    // No SHELL borrow survives a runtime call.
    let result = match action {
        SitePermissionAction::Allow => {
            crate::site_permission_runtime::decide(token, true).map(|_| None)
        }
        SitePermissionAction::Cancel => {
            crate::site_permission_runtime::decide(token, false).map(|_| None)
        }
        SitePermissionAction::Reveal => {
            crate::site_permission_runtime::reveal_source(token).map(Some)
        }
    };
    let revealed = match result {
        Ok(tab) => tab,
        Err(error) => {
            if site_permission_panel_matches(&window, &surface) {
                SHELL.with(|slot| {
                    if let Some(shell) = slot.borrow_mut().as_mut() {
                        shell.site_permissions.notice = Some(error);
                    }
                });
                sync();
            }
            return;
        }
    };
    let Some(tab) = revealed else {
        if site_permission_panel_matches(&window, &surface) {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.site_permissions.notice = None;
                }
            });
            sync();
        }
        return;
    };
    let Some(snapshot) = crate::browser::shell_snapshot() else {
        return;
    };
    let Some(space) = download_reveal_selection(&snapshot, tab).map(str::to_owned) else {
        return;
    };
    if !site_permission_panel_matches(&window, &surface)
        || !window.isKeyWindow()
        || page_find_has_marked_text()
        || !crate::browser::shell_snapshot().is_some_and(|snapshot| {
            download_reveal_selection(&snapshot, tab) == Some(space.as_str())
        })
    {
        return;
    }
    close_site_permissions(false);
    if !window.isKeyWindow() || page_find_has_marked_text() {
        return;
    }
    let can_focus = SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                !shell.overview
                    && !shell.overlay_is_open()
                    && shell.active == Some(tab)
                    && shell.active_space == space
                    && std::ptr::eq(&*shell.window, &*window)
            })
        })
    });
    if can_focus
        && crate::browser::shell_snapshot().is_some_and(|snapshot| {
            download_reveal_selection(&snapshot, tab) == Some(space.as_str())
        })
    {
        crate::browser::focus_tab(tab);
    }
}

fn site_permission_page_target(shell: &Shell) -> Option<crate::browser::CurrentPageTarget> {
    if !shell.site_permissions.open
        || shell.search.open
        || shell.bookmarks.state.open
        || shell.downloads.open
    {
        return None;
    }
    current_page_target(
        &shell.tabs,
        &shell.active_space,
        shell.active,
        shell.overview,
        false,
    )
}

fn site_permission_page_target_ui() -> Option<crate::browser::CurrentPageTarget> {
    let (window, surface) = site_permission_controls()?;
    let (page, view) = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let page = site_permission_page_target(shell)?;
        Some((page.clone(), shell.views.get(&page.tab)?.clone()))
    })?;
    if view.window().as_deref() != Some(&*window)
        || !site_permission_panel_matches(&window, &surface)
    {
        return None;
    }
    let current = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if !shell
            .views
            .get(&page.tab)
            .is_some_and(|current| std::ptr::eq(&**current, &*view))
        {
            return None;
        }
        site_permission_page_target(shell)
    });
    if current.as_ref() != Some(&page) {
        return None;
    }
    crate::browser::validate_current_page_target(&page).ok()?;
    Some(page)
}

fn site_permission_site_snapshot_ui() -> SitePermissionSiteQuery {
    if SITE_PERMISSION_QUERYING.with(|querying| querying.replace(true)) {
        return SitePermissionSiteQuery::Skipped;
    }
    let mut snapshot_invalid = false;
    let value = (|| {
        let window = SHELL.with(|slot| {
            let slot = slot.try_borrow().ok()?;
            let shell = slot.as_ref().filter(|shell| {
                shell.site_permissions.open
                    && !shell.search.open
                    && !shell.bookmarks.state.open
                    && !shell.downloads.open
            })?;
            Some(shell.window.clone())
        })?;
        if !window.isVisible() || window.isMiniaturized() {
            return None;
        }
        let Some(current) = crate::browser::shell_snapshot() else {
            snapshot_invalid = true;
            return None;
        };
        let page = current_page_target(
            &current.tabs,
            &current.active_space,
            current.active_tab,
            current.overview,
            false,
        )?;
        let snapshot = crate::site_permission_runtime::site_snapshot(&page);
        (site_permissions_are_open() && crate::browser::validate_current_page_target(&page).is_ok())
            .then_some((page, snapshot))
    })();
    SITE_PERMISSION_QUERYING.with(|querying| querying.set(false));
    if snapshot_invalid {
        SitePermissionSiteQuery::Skipped
    } else {
        SitePermissionSiteQuery::Ready(value)
    }
}

fn site_permission_site_is_current(page: &crate::browser::CurrentPageTarget, origin: &str) -> bool {
    site_permission_page_target_ui().as_ref() == Some(page)
        && SHELL.with(|slot| {
            slot.try_borrow().ok().is_some_and(|slot| {
                slot.as_ref()
                    .and_then(|shell| shell.site_permissions.current.as_ref())
                    .is_some_and(|(shown, snapshot)| shown == page && snapshot.origin == origin)
            })
        })
}

fn reset_site_permissions(sender: &NSButton) {
    let Some((window, surface)) = site_permission_controls() else {
        return;
    };
    let current = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.site_permissions.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && !shell.downloads.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.site_permissions.surface, &*surface)
        })?;
        if !std::ptr::eq(sender, &*shell.site_permissions.reset) {
            return None;
        }
        shell
            .site_permissions
            .current
            .as_ref()
            .filter(|(_, snapshot)| snapshot.can_reset())
            .cloned()
    });
    let Some((page, snapshot)) = current else {
        return;
    };
    if !site_permission_site_is_current(&page, &snapshot.origin) {
        return;
    }
    let result = crate::site_permission_runtime::reset_site(&snapshot, || {
        site_permission_site_is_current(&page, &snapshot.origin)
    });
    if site_permission_panel_matches(&window, &surface) {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.site_permissions.notice = Some(match result {
                    Ok(()) => "저장된 위치와 알림 설정을 초기화했습니다. 해당 사이트의 대기 요청은 취소했으며 열린 페이지는 다시 열어 확인하세요.".to_owned(),
                    Err(error) => error,
                });
            }
        });
        sync();
    }
}

fn set_download_notice(notice: Option<String>) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.downloads.notice = notice;
        }
    });
}

fn preserve_address_for_overlay() {
    // A toolbar click may already have ended the address field editor. Keep its
    // current text through native overlay updates without changing page state.
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            // Switching overlays must not adopt a hidden field as a new draft.
            if !shell.overview && !shell.overlay_is_open() && shell.address_draft.is_none() {
                shell.address_draft = shell
                    .tabs
                    .iter()
                    .find(|tab| Some(tab.id) == shell.active && tab.space == shell.active_space)
                    .map(|tab| AddressDraft {
                        tab: tab.id,
                        url: tab.url.clone(),
                        navigation_epoch: tab.navigation_epoch,
                    });
            }
        }
    });
}

fn open_downloads() {
    preserve_address_for_overlay();
    close_page_find(false, false);
    close_tab_search(false);
    close_bookmarks(false);
    close_site_permissions(false);
    let controls = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.downloads.open
            || (shell.file_selection_count == 0 && shell.file_selection_notice.is_some())
        {
            shell.downloads.notice = None;
        }
        shell.downloads.open = true;
        Some((shell.window.clone(), shell.downloads.close.clone()))
    });
    if let Some((window, close)) = controls {
        sync();
        window.makeFirstResponder(Some(&close));
    }
}

fn close_downloads(restore_focus: bool) {
    let window = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.downloads.open {
            return None;
        }
        shell.downloads.open = false;
        shell.downloads.notice = None;
        Some(shell.window.clone())
    });
    if let Some(window) = window {
        window.makeFirstResponder(None);
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.downloads.clear_rows();
            }
        });
        sync();
        apply_selection(restore_focus);
    }
}

fn download_row_action(sender: &NSButton, choose: bool) {
    let id: isize = unsafe { msg_send![sender, tag] };
    let Ok(id) = u32::try_from(id) else { return };
    if !SHELL.with(|slot| {
        slot.borrow().as_ref().is_some_and(|shell| {
            shell.downloads.open && shell.downloads.rows.iter().any(|row| row.id == id)
        })
    }) {
        return;
    }
    // Save panels and download callbacks may re-enter AppKit. No shell borrow
    // crosses the runtime call, and these app actions do not hand off a page.
    let result = if choose {
        crate::download_runtime::choose_path(id)
    } else {
        crate::download_runtime::cancel(id)
    };
    set_download_notice(result.err());
    sync();
}

fn restart_download(sender: &NSButton) {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.downloads.open)?;
        Some((shell.window.clone(), shell.downloads.surface.clone()))
    });
    let Some((window, surface)) = controls else {
        return;
    };
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || surface.window().as_deref() != Some(&*window)
        || surface.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return;
    }
    // Resolve the exact visible row after native getters, which may reenter.
    // A delayed Save or Finder action must never become a new network request.
    let id = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.downloads.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
        })?;
        shell
            .downloads
            .rows
            .iter()
            .find(|row| row.can_restart && std::ptr::eq(sender, &*row.restart))
            .map(|row| row.id)
    });
    let Some(id) = id else { return };
    // The runtime owns asynchronous acceptance and timeout notices. A local
    // success message would mask those later results in the panel.
    let notice = crate::download_runtime::restart(id).err();
    let current = SHELL.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        let Some(shell) = slot.as_mut().filter(|shell| {
            shell.downloads.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
                && shell
                    .downloads
                    .rows
                    .iter()
                    .any(|row| row.id == id && std::ptr::eq(sender, &*row.restart))
        }) else {
            return false;
        };
        shell.downloads.notice = notice;
        true
    });
    if current {
        sync();
    }
}

fn reveal_download_file(sender: &NSButton) {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.downloads.open)?;
        Some((shell.window.clone(), shell.downloads.surface.clone()))
    });
    let Some((window, surface)) = controls else {
        return;
    };
    // Finder is an explicit native action. Validate the live control after
    // AppKit getters, which may reenter without retaining a shell borrow.
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || surface.window().as_deref() != Some(&*window)
        || surface.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return;
    }
    let id = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.downloads.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
        })?;
        shell
            .downloads
            .rows
            .iter()
            .find(|row| row.can_reveal_file && std::ptr::eq(sender, &*row.reveal_file))
            .map(|row| row.id)
    });
    let Some(id) = id else { return };
    // The runtime revalidates the completed file. Keep the browser's current
    // space, tab, address draft and panel while Finder handles the request.
    let notice = match crate::download_runtime::reveal_file(id) {
        Ok(()) => "Finder에 파일 위치 표시를 요청했습니다.".to_owned(),
        Err(error) => error,
    };
    let current = SHELL.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        let Some(shell) = slot.as_mut().filter(|shell| {
            shell.downloads.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
                && shell
                    .downloads
                    .rows
                    .iter()
                    .any(|row| row.id == id && std::ptr::eq(sender, &*row.reveal_file))
        }) else {
            return false;
        };
        shell.downloads.notice = Some(notice);
        true
    });
    if current {
        sync();
    }
}

enum DownloadSource {
    File(u32),
    Permission(u64),
}

fn download_reveal_selection(snapshot: &ShellSnapshot, tab: i32) -> Option<&str> {
    (!snapshot.overview
        && snapshot.active_tab == Some(tab)
        && snapshot
            .tabs
            .iter()
            .any(|item| item.id == tab && item.space == snapshot.active_space))
    .then_some(snapshot.active_space.as_str())
}

fn reveal_download_source(sender: &NSButton, permission: bool) {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.downloads.open)?;
        Some((shell.window.clone(), shell.downloads.surface.clone()))
    });
    let Some((window, surface)) = controls else {
        return;
    };
    // Native getters may reenter. Resolve the current row only afterwards.
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || surface.window().as_deref() != Some(&*window)
        || surface.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return;
    }
    let source = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.downloads.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
        })?;
        if permission {
            shell
                .downloads
                .permissions
                .iter()
                .find(|row| std::ptr::eq(sender, &*row.reveal))
                .map(|row| DownloadSource::Permission(row.token))
        } else {
            shell
                .downloads
                .rows
                .iter()
                .find(|row| row.can_reveal && std::ptr::eq(sender, &*row.reveal))
                .map(|row| DownloadSource::File(row.id))
        }
    });
    let Some(source) = source else {
        return;
    };
    // File sources are captured when CEF accepts the file, while permission
    // sources identify the delivered request. Both runtimes revalidate before
    // selection; no shell borrow, dismissal or draft clearing precedes success.
    let result = match source {
        DownloadSource::File(id) => crate::download_runtime::reveal_source(id),
        DownloadSource::Permission(token) => {
            crate::download_permission_runtime::reveal_source(token)
        }
    };
    let tab = match result {
        Ok(tab) => tab,
        Err(error) => {
            if downloads_are_open() {
                set_download_notice(Some(error));
                sync();
            }
            return;
        }
    };
    let Some(selected) = crate::browser::shell_snapshot() else {
        return;
    };
    let Some(space) = download_reveal_selection(&selected, tab).map(str::to_owned) else {
        return;
    };
    if !window.isKeyWindow() || page_find_has_marked_text() {
        return;
    }
    let can_close = SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                shell.downloads.open
                    && !shell.search.open
                    && !shell.bookmarks.state.open
                    && !shell.overview
                    && shell.active == Some(tab)
                    && shell.active_space == space
                    && std::ptr::eq(&*shell.window, &*window)
            })
        })
    });
    if !can_close
        || !crate::browser::shell_snapshot().is_some_and(|snapshot| {
            download_reveal_selection(&snapshot, tab) == Some(space.as_str())
        })
    {
        return;
    }
    close_downloads(false);
    // Closing an overlay can reenter. Do not focus a tab that has since moved
    // or interrupt a new IME composition. Same-tab drafts remain intact.
    if !window.isKeyWindow() || page_find_has_marked_text() {
        return;
    }
    let can_focus = SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                !shell.overview
                    && !shell.overlay_is_open()
                    && shell.active == Some(tab)
                    && shell.active_space == space
                    && std::ptr::eq(&*shell.window, &*window)
            })
        })
    });
    if can_focus
        && crate::browser::shell_snapshot().is_some_and(|snapshot| {
            download_reveal_selection(&snapshot, tab) == Some(space.as_str())
        })
    {
        crate::browser::focus_tab(tab);
    }
}

fn download_permission_action(sender: &NSButton, allow: bool) {
    let token = SHELL.with(|slot| {
        let slot = slot.borrow();
        let panel = &slot.as_ref()?.downloads;
        if !panel.open {
            return None;
        }
        // A queued action from a removed row must not decide another request.
        panel.permissions.iter().find_map(|row| {
            let current = if allow { &*row.allow } else { &*row.cancel };
            std::ptr::eq(sender, current).then_some(row.token)
        })
    });
    let Some(token) = token else { return };
    // CEF may dismiss this prompt and refresh the shell synchronously.
    // Never hold a shell borrow while continuing the permission callback.
    let result = crate::download_permission_runtime::decide(token, allow);
    set_download_notice(result.err());
    sync();
}

fn file_selection_action(sender: &NSButton, choose: bool) {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.downloads.open)?;
        Some((shell.window.clone(), shell.downloads.surface.clone()))
    });
    let Some((window, surface)) = controls else {
        return;
    };
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || surface.window().as_deref() != Some(&*window)
        || surface.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return;
    }
    // Resolve the exact visible control after native getters. A delayed click
    // from a removed request cannot select files for a replacement request.
    let token = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| {
            shell.downloads.open
                && !shell.search.open
                && !shell.bookmarks.state.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
        })?;
        shell.downloads.file_selections.iter().find_map(|row| {
            let control = if choose { &*row.choose } else { &*row.cancel };
            (std::ptr::eq(sender, control) && (!choose || !row.choosing)).then_some(row.token)
        })
    });
    let Some(token) = token else { return };
    // Opening a sheet may synchronously revoke the row or refresh the shell.
    // Selection never reveals the source tab or changes task ownership.
    let notice = if choose {
        crate::file_selection_runtime::choose(token)
    } else {
        crate::file_selection_runtime::cancel(token)
    }
    .err();
    SHELL.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return;
        };
        if let Some(shell) = slot.as_mut().filter(|shell| {
            shell.downloads.open
                && std::ptr::eq(&*shell.window, &*window)
                && std::ptr::eq(&*shell.downloads.surface, &*surface)
        }) {
            shell.downloads.notice = notice;
        }
    });
    sync();
}

fn download_site_page_target(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    overview: bool,
    downloads_open: bool,
    other_overlay: bool,
) -> Option<crate::browser::CurrentPageTarget> {
    if !downloads_open || other_overlay {
        return None;
    }
    // The download overlay intentionally hides CEF. Its underlying selected
    // page, not an address draft or another space's row, is the setting target.
    current_page_target(tabs, space, active, overview, false)
}

fn download_site_page_target_ui() -> Option<crate::browser::CurrentPageTarget> {
    let (page, window, view, surface) = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let page = download_site_page_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.downloads.open,
            shell.search.open || shell.bookmarks.state.open,
        )?;
        Some((
            page.clone(),
            shell.window.clone(),
            shell.views.get(&page.tab)?.clone(),
            shell.downloads.surface.clone(),
        ))
    })?;
    if !window.isVisible()
        || window.isMiniaturized()
        || view.window().as_deref() != Some(&*window)
        || !window.isKeyWindow()
        || surface.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return None;
    }
    let current = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if !std::ptr::eq(&*shell.window, &*window)
            || !shell
                .views
                .get(&page.tab)
                .is_some_and(|current| std::ptr::eq(&**current, &*view))
        {
            return None;
        }
        download_site_page_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.downloads.open,
            shell.search.open || shell.bookmarks.state.open,
        )
    });
    if current.as_ref() != Some(&page) {
        return None;
    }
    crate::browser::validate_current_page_target(&page).ok()?;
    Some(page)
}

fn download_site_snapshot_ui() -> DownloadSiteQuery {
    if DOWNLOAD_SITE_QUERYING.with(|querying| querying.replace(true)) {
        return DownloadSiteQuery::Skipped;
    }
    let mut snapshot_invalid = false;
    let value = (|| {
        let window = SHELL.with(|slot| {
            let slot = slot.try_borrow().ok()?;
            let shell = slot.as_ref().filter(|shell| {
                shell.downloads.open && !shell.search.open && !shell.bookmarks.state.open
            })?;
            Some(shell.window.clone())
        })?;
        if !window.isVisible() || window.isMiniaturized() {
            return None;
        }
        // The shell may still show the previous selection until this sync's
        // render. Resolve the read-only query from fresh browser state instead.
        let Some(current) = crate::browser::shell_snapshot() else {
            snapshot_invalid = true;
            return None;
        };
        let page = download_site_page_target(
            &current.tabs,
            &current.active_space,
            current.active_tab,
            current.overview,
            true,
            false,
        )?;
        // CEF getters can reenter; no SHELL borrow crosses the runtime call.
        let snapshot = crate::download_permission_runtime::site_snapshot(&page);
        (downloads_are_open() && crate::browser::validate_current_page_target(&page).is_ok())
            .then_some((page, snapshot))
    })();
    DOWNLOAD_SITE_QUERYING.with(|querying| querying.set(false));
    if snapshot_invalid {
        DownloadSiteQuery::Skipped
    } else {
        DownloadSiteQuery::Ready(value)
    }
}

fn download_site_is_current(page: &crate::browser::CurrentPageTarget, origin: &str) -> bool {
    download_site_page_target_ui().as_ref() == Some(page)
        && SHELL.with(|slot| {
            slot.try_borrow().ok().is_some_and(|slot| {
                slot.as_ref()
                    .and_then(|shell| shell.downloads.site.current.as_ref())
                    .is_some_and(|(shown_page, snapshot)| {
                        shown_page == page && snapshot.origin == origin
                    })
            })
        })
}

fn reset_download_site(sender: &NSButton) {
    let current = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.downloads.open)?;
        let site = &shell.downloads.site;
        if !std::ptr::eq(sender, &*site.reset) {
            return None;
        }
        site.current
            .as_ref()
            .filter(|(_, snapshot)| {
                snapshot.setting == crate::download_permissions::SiteSetting::Allowed
            })
            .cloned()
    });
    let Some((page, snapshot)) = current else {
        return;
    };
    if !download_site_is_current(&page, &snapshot.origin) {
        return;
    }
    // A setting change is browser management, not page input or an AI handoff.
    let result = crate::download_permission_runtime::reset_site(&snapshot, || {
        download_site_is_current(&page, &snapshot.origin)
    });
    set_download_notice(Some(match result {
        Ok(_) => {
            "저장된 허용 설정을 초기화했습니다. 열린 페이지는 다시 열어 확인하세요.".to_owned()
        }
        Err(error) => error,
    }));
    sync();
}

impl FindBar {
    fn new(parent: &NSView, target: &ShellTarget, marker: MainThreadMarker) -> Self {
        let surface = surface_child(parent, SurfaceKind::Toolbar, 0.0, true, marker);
        surface.setHidden(true);
        let input_surface = surface_child(&surface, SurfaceKind::Address, 8.0, true, marker);
        let field =
            NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 18.0));
        field.setEditable(true);
        field.setSelectable(true);
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        field.setPlaceholderString(Some(&NSString::from_str("페이지에서 찾기")));
        field.setToolTip(Some(&NSString::from_str(
            "페이지에서 찾기 · 최대 200자 · Enter 다음, Shift Enter 이전",
        )));
        unsafe {
            field.setTarget(Some(target));
            field.setAction(Some(sel!(nextPageFind:)));
            let _: () = msg_send![&*field, setDelegate: target];
            let _: () =
                msg_send![&*field, setAccessibilityLabel: &*NSString::from_str("페이지에서 찾기")];
        }
        input_surface.addSubview(&field);
        let result = label(&surface, "", 11.0, false, marker);
        result.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let previous = button(
            &surface,
            "↑",
            target,
            sel!(previousPageFind:),
            "이전 일치 항목 (⇧⌘G)",
            marker,
        );
        let next = button(
            &surface,
            "↓",
            target,
            sel!(nextPageFind:),
            "다음 일치 항목 (⌘G)",
            marker,
        );
        let close = button(
            &surface,
            "×",
            target,
            sel!(closePageFind:),
            "페이지 찾기 닫기 (Esc)",
            marker,
        );
        set_symbol(&previous, "chevron.up", "", "↑", "이전 일치 항목 (⇧⌘G)");
        set_symbol(&next, "chevron.down", "", "↓", "다음 일치 항목 (⌘G)");
        set_symbol(&close, "xmark", "", "×", "페이지 찾기 닫기 (Esc)");
        Self {
            state: PageFindState::default(),
            surface,
            input_surface,
            field,
            result,
            previous,
            next,
            close,
        }
    }

    fn render(&self) {
        let state = &self.state;
        let text = if let Some(notice) = &state.notice {
            compact(notice, 18)
        } else if state.query.is_empty() {
            String::new()
        } else if state.final_update && state.count == Some(0) {
            "결과 없음".into()
        } else if let Some(count) = state.count {
            match state.ordinal {
                Some(ordinal) if ordinal > 0 => format!("{ordinal} / {count}"),
                _ if state.final_update => format!("{count}개"),
                _ => format!("… / {count}"),
            }
        } else if state.final_update {
            "결과 미정".into()
        } else {
            "찾는 중…".into()
        };
        self.result.setStringValue(&NSString::from_str(&text));
        self.result
            .setToolTip(state.notice.as_deref().map(NSString::from_str).as_deref());
        let can_step = !state.query.is_empty()
            && state.notice.is_none()
            && !(state.final_update && state.count == Some(0));
        self.previous.setEnabled(can_step);
        self.next.setEnabled(can_step);
    }

    fn layout(&self, width: f64) {
        let input_width = (width - 210.0).max(150.0);
        self.input_surface
            .setFrame(rect(12.0, 7.0, input_width, 30.0));
        self.field
            .setFrame(rect(10.0, 6.0, input_width - 20.0, 18.0));
        self.result.setFrame(rect(
            input_width + 22.0,
            13.0,
            (width - input_width - 128.0).max(1.0),
            18.0,
        ));
        self.previous.setFrame(rect(width - 96.0, 8.0, 28.0, 28.0));
        self.next.setFrame(rect(width - 66.0, 8.0, 28.0, 28.0));
        self.close.setFrame(rect(width - 36.0, 8.0, 28.0, 28.0));
    }
}

fn visible_find_target(shell: &Shell) -> Option<(i32, u64)> {
    if shell.overview || shell.overlay_is_open() {
        return None;
    }
    let id = shell.active?;
    if !shell.views.contains_key(&id) {
        return None;
    }
    shell
        .tabs
        .iter()
        .find(|tab| tab.id == id && !tab.crashed)
        .map(|tab| (tab.id, tab.navigation_epoch))
}

/// Only the native find editor blocks a CEF request to take keyboard focus.
/// The visible page remains an input target while its find bar is open.
pub(crate) fn page_find_is_editing() -> bool {
    let field = SHELL.with(|slot| {
        slot.try_borrow()
            .map(|slot| {
                slot.as_ref()
                    .filter(|shell| shell.find.state.open)
                    .map(|shell| shell.find.field.clone())
            })
            .map_err(|_| ())
    });
    match field {
        Ok(Some(field)) => unsafe {
            let editor: *mut AnyObject = msg_send![&*field, currentEditor];
            !editor.is_null()
        },
        Ok(None) => false,
        Err(()) => true,
    }
}

fn page_find_has_marked_text() -> bool {
    let window = SHELL.with(|slot| slot.borrow().as_ref().map(|shell| shell.window.clone()));
    let Some(responder) = window.and_then(|window| window.firstResponder()) else {
        return false;
    };
    // Find leaves the page visible: Esc/next/open must also respect a page or
    // address IME composition, not only the find field's shared native editor.
    unsafe {
        let supports: bool = msg_send![&*responder, respondsToSelector: sel!(hasMarkedText)];
        supports && msg_send![&*responder, hasMarkedText]
    }
}

fn focus_page_find() {
    let controls = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| shell.find.state.open)
            .map(|shell| (shell.window.clone(), shell.find.field.clone()))
    });
    if let Some((window, field)) = controls {
        window.makeFirstResponder(Some(&field));
        unsafe {
            let _: () = msg_send![&*field, selectText: std::ptr::null::<AnyObject>()];
        }
    }
}

fn open_page_find(require_remembered: bool) -> bool {
    let candidate = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let target = visible_find_target(shell)?;
        let query = if shell.find.state.target == Some(target) {
            shell.find.state.query.clone()
        } else {
            String::new()
        };
        if require_remembered && query.is_empty() {
            return None;
        }
        Some((target, query, shell.address.clone()))
    });
    let Some(((tab, epoch), query, address)) = candidate else {
        return false;
    };
    if page_find_has_marked_text() {
        return false;
    }
    let editing_address = unsafe {
        let editor: *mut AnyObject = msg_send![&*address, currentEditor];
        !editor.is_null()
    };
    if editing_address {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.address_draft =
                    shell
                        .tabs
                        .iter()
                        .find(|item| item.id == tab)
                        .map(|item| AddressDraft {
                            tab,
                            url: item.url.clone(),
                            navigation_epoch: item.navigation_epoch,
                        });
            }
        });
    }
    // Handoff can advance the tab epoch. The backend returns the accepted epoch.
    let epoch = match crate::browser::begin_page_find(tab, epoch) {
        Ok(epoch) => epoch,
        Err(error) => {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.notice = Some(error);
                }
            });
            refresh();
            return false;
        }
    };
    sync();
    let opened = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(shell) = slot.as_mut() else {
            return false;
        };
        if visible_find_target(shell) != Some((tab, epoch)) {
            return false;
        }
        if editing_address {
            // Opening find can hand off this same document and advance its
            // reference epoch. That alone must not discard an address draft.
            if let Some(draft) = shell
                .address_draft
                .as_mut()
                .filter(|draft| draft.tab == tab)
            {
                draft.navigation_epoch = epoch;
            }
        }
        shell.find.state.open = true;
        shell.find.state.target = Some((tab, epoch));
        shell.find.state.query = query;
        shell.find.state.notice = None;
        shell
            .find
            .field
            .setStringValue(&NSString::from_str(&shell.find.state.query));
        shell.find.render();
        true
    });
    if opened {
        layout();
        focus_page_find();
        if !require_remembered {
            request_page_find(true, false);
        }
    }
    opened
}

fn close_page_find(remember: bool, restore_focus: bool) {
    let editing = page_find_is_editing();
    let closed = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        let was_open = shell.find.state.open;
        let stop = shell.find.state.close(remember);
        if !remember {
            shell.find.field.setStringValue(&NSString::from_str(""));
        }
        Some((was_open, stop, shell.window.clone()))
    });
    let Some((was_open, stop, window)) = closed else {
        return;
    };
    if let Some((tab, token)) = stop {
        crate::browser::stop_page_find(tab, token);
    }
    if was_open {
        if editing {
            window.makeFirstResponder(None);
        }
        layout();
        if restore_focus {
            apply_selection(true);
        }
    }
}

fn page_find_text_changed() {
    if page_find_has_marked_text() {
        return;
    }
    let field = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| shell.find.state.open)
            .map(|shell| shell.find.field.clone())
    });
    let Some(field) = field else {
        return;
    };
    let text = field.stringValue().to_string();
    let bounded = tab_search::bounded_query(&text);
    let changed = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(shell) = slot.as_mut().filter(|shell| shell.find.state.open) else {
            return false;
        };
        if shell.find.state.query == bounded {
            return false;
        }
        shell.find.state.query = bounded.clone();
        true
    });
    if text != bounded {
        field.setStringValue(&NSString::from_str(&bounded));
    }
    if changed {
        request_page_find(true, false);
    }
}

fn step_page_find(forward: bool) {
    if page_find_has_marked_text() {
        return;
    }
    let open = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.find.state.open)
    });
    if !open && !open_page_find(true) {
        return;
    }
    let continuing = open
        && SHELL.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|shell| shell.find.state.token.is_some())
        });
    request_page_find(forward, continuing);
}

fn request_page_find(forward: bool, find_next: bool) {
    let request = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut().filter(|shell| shell.find.state.open)?;
        let (tab, epoch) = shell.find.state.target?;
        if visible_find_target(shell) != Some((tab, epoch)) {
            return None;
        }
        let state = &mut shell.find.state;
        if state.query.is_empty() {
            let stop = state.token.take().map(|token| (tab, token));
            state.count = None;
            state.ordinal = None;
            state.final_update = false;
            state.notice = None;
            shell.find.render();
            return Some((None, stop));
        }
        if state.query.chars().any(char::is_control) {
            let stop = state.token.take().map(|token| (tab, token));
            state.count = None;
            state.ordinal = None;
            state.final_update = false;
            state.notice = Some("줄바꿈과 제어 문자는 찾을 수 없습니다.".into());
            shell.find.render();
            return Some((None, stop));
        }
        let previous_token = state.token;
        let token = match state.request() {
            Some(token) => token,
            None => {
                state.notice = Some("찾기 요청을 더 만들 수 없습니다.".into());
                shell.find.render();
                return None;
            }
        };
        let request = (tab, epoch, token, state.query.clone(), previous_token);
        shell.find.render();
        Some((Some(request), None))
    });
    let Some((request, stop)) = request else {
        return;
    };
    if let Some((tab, token)) = stop {
        crate::browser::stop_page_find(tab, token);
    }
    let Some((tab, epoch, token, query, previous_token)) = request else {
        return;
    };
    if let Err(error) = crate::browser::find_in_page(tab, epoch, token, &query, forward, find_next)
    {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                if shell.find.state.open
                    && shell.find.state.target == Some((tab, epoch))
                    && shell.find.state.token == Some(token)
                {
                    shell.find.state.token = None;
                    shell.find.state.notice = Some(error);
                    shell.find.render();
                }
            }
        });
        // A rejection may happen before the backend replaces its old token.
        // Exact-match stops safely cover both acceptance boundaries.
        crate::browser::stop_page_find(tab, token);
        if let Some(previous) = previous_token {
            crate::browser::stop_page_find(tab, previous);
        }
    }
}

pub(crate) fn page_find_result(
    tab: i32,
    epoch: u64,
    token: u64,
    count: i32,
    ordinal: i32,
    final_update: bool,
) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            if shell
                .find
                .state
                .receive(tab, epoch, token, count, ordinal, final_update)
            {
                shell.find.render();
            }
        }
    });
}

fn focus_bookmark_field(name: bool) {
    let controls = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref().filter(|shell| shell.bookmarks.state.open)?;
        Some((
            shell.window.clone(),
            if name {
                shell.bookmarks.name.clone()
            } else {
                shell.bookmarks.query.clone()
            },
        ))
    });
    if let Some((window, field)) = controls {
        window.makeFirstResponder(Some(&field));
        unsafe {
            let _: () = msg_send![&*field, selectText: std::ptr::null::<AnyObject>()];
        }
    }
}

fn open_bookmarks(add_current: bool) {
    preserve_address_for_overlay();
    close_page_find(false, false);
    let tab = active_tab();
    close_tab_search(false);
    close_downloads(false);
    close_site_permissions(false);
    let existing_editor = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        let panel = &mut shell.bookmarks;
        if !panel.state.open {
            panel.state.open = true;
            panel.state.query.clear();
            panel.state.selected = None;
            panel.state.notice = None;
            panel.query.setStringValue(&NSString::from_str(""));
        }
        Some(panel.state.draft.is_some())
    });
    let Some(existing_editor) = existing_editor else {
        return;
    };
    sync();
    if !add_current || existing_editor {
        focus_bookmark_field(add_current && existing_editor);
        return;
    }
    // An invalid/blank page can reject preparation. Move focus off the hidden
    // CEF view before calling the backend, even when no editor will be opened.
    focus_bookmark_field(false);
    let request = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let panel = &mut slot.as_mut()?.bookmarks;
        if panel.busy || !panel.writable {
            return None;
        }
        if tab.is_none() {
            panel.state.notice = Some("먼저 저장할 웹페이지를 열어 주세요.".into());
            return None;
        }
        panel.state.begin(BookmarkOperation::Prepare)
    });
    if let (Some(request), Some(tab)) = (request, tab) {
        // prepare may complete synchronously; no shell borrow crosses this call.
        crate::bookmark_runtime::prepare(request, tab);
    } else {
        sync();
        focus_bookmark_field(false);
    }
}

fn close_bookmarks(restore_focus: bool) {
    let window = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        let panel = &mut shell.bookmarks;
        if !panel.state.open {
            return None;
        }
        panel.state.open = false;
        panel.state.dismiss_editor();
        panel.state.notice = None;
        panel.clear_rows();
        Some(shell.window.clone())
    });
    if let Some(window) = window {
        window.makeFirstResponder(None);
        sync();
        apply_selection(restore_focus);
    }
}

fn set_bookmark_editor(draft: crate::bookmark_runtime::Draft) {
    let controls = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let panel = &mut slot.as_mut()?.bookmarks;
        if !panel.state.open {
            return None;
        }
        let controls = (
            panel.name.clone(),
            panel.url.clone(),
            draft.title.clone(),
            draft.url.clone(),
        );
        panel.state.notice = None;
        panel.state.draft = Some(draft);
        Some(controls)
    });
    if let Some((name, url, title, address)) = controls {
        name.setStringValue(&NSString::from_str(&title));
        url.setStringValue(&NSString::from_str(&compact(&address, 110)));
        url.setToolTip(Some(&NSString::from_str(&address)));
        sync();
        focus_bookmark_field(true);
    }
}

/// Main-thread completion. Closing or replacing an editor invalidates its token;
/// the runtime still commits the original operation and updates its own cache.
pub(crate) fn bookmark_completed(
    request: u64,
    result: Result<crate::bookmark_runtime::Outcome, String>,
) {
    let operation = SHELL.with(|slot| {
        slot.borrow_mut()
            .as_mut()?
            .bookmarks
            .state
            .take_completion(request)
    });
    let Some(operation) = operation else {
        return;
    };
    match result {
        Ok(crate::bookmark_runtime::Outcome::Prepared(draft))
            if operation == BookmarkOperation::Prepare =>
        {
            set_bookmark_editor(draft);
            return;
        }
        Ok(crate::bookmark_runtime::Outcome::Changed {
            id,
            durability_warning,
            ..
        }) => {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    let panel = &mut shell.bookmarks;
                    if operation == BookmarkOperation::Save
                        || (operation == BookmarkOperation::Remove
                            && panel
                                .state
                                .draft
                                .as_ref()
                                .is_some_and(|draft| draft.id == Some(id)))
                    {
                        panel.state.draft = None;
                    }
                    panel.state.selected = Some(id);
                    panel.state.notice = durability_warning.or_else(|| {
                        Some(
                            if operation == BookmarkOperation::Remove {
                                "북마크를 삭제했습니다."
                            } else {
                                "북마크를 저장했습니다."
                            }
                            .into(),
                        )
                    });
                }
            });
        }
        Err(error) => {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.bookmarks.state.notice = Some(error);
                }
            });
        }
        _ => return,
    }
    sync();
    // A failed save retains the fixed URL and editable title. Completion must
    // not move focus away from a query the user started while the write ran.
}

fn cancel_bookmark_editor() {
    if bookmarks_have_marked_text() {
        return;
    }
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.bookmarks.state.dismiss_editor();
            shell.bookmarks.state.notice = None;
        }
    });
    sync();
    focus_bookmark_field(false);
}

fn save_bookmark() {
    if bookmarks_have_marked_text() {
        return;
    }
    let input = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let panel = &mut slot.as_mut()?.bookmarks;
        if !panel.state.open || panel.busy || !panel.writable {
            return None;
        }
        let draft = panel.state.draft.as_ref()?;
        let id = draft.id;
        let url = draft.url.clone();
        let title = panel.name.stringValue().to_string();
        let request = panel.state.begin(BookmarkOperation::Save)?;
        Some((request, id, url, title))
    });
    let Some((request, id, url, title)) = input else {
        return;
    };
    sync();
    focus_bookmark_field(false);
    if let Some(id) = id {
        crate::bookmark_runtime::rename(request, id, title);
    } else {
        crate::bookmark_runtime::add(request, url, title);
    }
}

fn open_bookmark(id: u64) {
    match crate::bookmark_runtime::open(id) {
        Ok(()) => close_bookmarks(true),
        Err(error) => {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.bookmarks.state.notice = Some(error);
                }
            });
            sync();
        }
    }
}

fn bookmark_row_action(sender: &NSButton, operation: Option<BookmarkOperation>) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let item = SHELL.with(|slot| {
        let slot = slot.borrow();
        let panel = &slot.as_ref()?.bookmarks;
        if !panel.state.open || (operation.is_some() && (panel.busy || !panel.writable)) {
            return None;
        }
        let row = panel.rows.get(usize::try_from(index).ok()?)?;
        let current = match operation {
            None => &*row.open,
            Some(BookmarkOperation::Prepare) => &*row.rename,
            Some(BookmarkOperation::Remove) => &*row.remove,
            _ => return None,
        };
        // A queued action from a replaced row must not act on its new index.
        std::ptr::eq(sender, current).then(|| row.item.clone())
    });
    let Some(item) = item else {
        return;
    };
    match operation {
        None => open_bookmark(item.id),
        Some(BookmarkOperation::Prepare) => set_bookmark_editor(crate::bookmark_runtime::Draft {
            id: Some(item.id),
            url: item.url,
            title: item.title,
        }),
        Some(BookmarkOperation::Remove) => {
            let request = SHELL.with(|slot| {
                slot.borrow_mut()
                    .as_mut()?
                    .bookmarks
                    .state
                    .begin(BookmarkOperation::Remove)
            });
            if let Some(request) = request {
                sync();
                crate::bookmark_runtime::remove(request, item.id);
            }
        }
        _ => {}
    }
}

fn bookmark_text_changed() {
    if bookmarks_have_marked_text() {
        return;
    }
    let fields = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| shell.bookmarks.state.open)
            .map(|shell| (shell.bookmarks.query.clone(), shell.bookmarks.name.clone()))
    });
    let Some((query, name)) = fields else {
        return;
    };
    let text = query.stringValue().to_string();
    let bounded = tab_search::bounded_query(&text);
    let title = name.stringValue().to_string();
    let bounded_title: String = title
        .chars()
        .take(crate::bookmarks::MAX_TITLE_CHARS)
        .collect();
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            if shell.bookmarks.state.query != bounded {
                shell.bookmarks.state.selected = None;
            }
            shell.bookmarks.state.query = bounded.clone();
        }
    });
    if text != bounded {
        query.setStringValue(&NSString::from_str(&bounded));
    }
    if title != bounded_title {
        name.setStringValue(&NSString::from_str(&bounded_title));
    }
    sync();
}

fn bookmarks_have_marked_text() -> bool {
    let fields = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|shell| (shell.bookmarks.query.clone(), shell.bookmarks.name.clone()))
    });
    fields
        .is_some_and(|(query, name)| field_has_marked_text(&query) || field_has_marked_text(&name))
}

fn field_has_marked_text(field: &NSTextField) -> bool {
    unsafe {
        let editor: *mut AnyObject = msg_send![field, currentEditor];
        !editor.is_null() && msg_send![editor, hasMarkedText]
    }
}

fn submit_bookmarks() {
    if bookmarks_have_marked_text() {
        return;
    }
    let id = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| shell.bookmarks.state.open)
            .and_then(|shell| shell.bookmarks.state.selected)
    });
    if let Some(id) = id {
        open_bookmark(id);
    }
}

fn move_bookmark_selection(forward: bool) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            let panel = &mut shell.bookmarks;
            if panel.rows.is_empty() {
                return;
            }
            let index = panel
                .rows
                .iter()
                .position(|row| Some(row.item.id) == panel.state.selected)
                .unwrap_or(0);
            let next = if forward {
                (index + 1).min(panel.rows.len() - 1)
            } else {
                index.saturating_sub(1)
            };
            panel.state.selected = Some(panel.rows[next].item.id);
        }
    });
    sync();
    let target = SHELL.with(|slot| {
        let slot = slot.borrow();
        let panel = &slot.as_ref()?.bookmarks;
        let row = panel
            .rows
            .iter()
            .find(|row| Some(row.item.id) == panel.state.selected)?;
        Some((panel.document.clone(), row.open.frame()))
    });
    if let Some((document, frame)) = target {
        document.scrollRectToVisible(frame);
    }
}

fn open_tab_search() {
    open_search(SearchMode::All);
}

fn open_search(mode: SearchMode) {
    if page_find_has_marked_text() {
        return;
    }
    preserve_address_for_overlay();
    close_page_find(false, false);
    close_bookmarks(false);
    close_downloads(false);
    close_site_permissions(false);
    let controls = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.search.open {
            shell.search_signature.clear();
        }
        shell.search.open(mode);
        Some((
            shell.window.clone(),
            shell.search_field.clone(),
            shell.search.query.clone(),
        ))
    });
    let Some((window, field, query)) = controls else {
        return;
    };
    sync();
    field.setStringValue(&NSString::from_str(&query));
    window.makeFirstResponder(Some(&field));
    unsafe {
        let _: () = msg_send![&*field, selectText: std::ptr::null::<AnyObject>()];
    }
}

fn toggle_search_mode() {
    if page_find_has_marked_text() {
        return;
    }
    let mode = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref().filter(|shell| shell.search.open)?;
        Some(if shell.search.mode == SearchMode::All {
            SearchMode::History
        } else {
            SearchMode::All
        })
    });
    if let Some(mode) = mode {
        open_search(mode);
    }
}

fn history_clear_action(sender: &NSButton, confirm: bool, cancel: bool) {
    if page_find_has_marked_text() {
        return;
    }
    let action = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot
            .as_mut()
            .filter(|shell| shell.search.open && shell.search.mode == SearchMode::History)?;
        let button = if cancel {
            &shell.history_cancel
        } else if confirm {
            &shell.history_delete
        } else {
            &shell.history_clear
        };
        if !std::ptr::eq(sender, &**button) || (!cancel && !shell.history_clear_available) {
            return None;
        }
        if cancel {
            if !shell.search.confirm_clear {
                return None;
            }
            shell.search.confirm_clear = false;
            shell.search.notice = None;
            return Some(false);
        }
        if confirm {
            return shell.search.confirm_clear.then_some(true);
        }
        if shell.search.confirm_clear {
            return None;
        }
        shell.search.confirm_clear = true;
        shell.search.notice = None;
        Some(false)
    });
    let Some(accepted) = action else {
        return;
    };
    if accepted {
        // Ok means accepted, not completed. The runtime publishes clearing and
        // the final result; it also invalidates old rows before asynchronous I/O.
        let result = crate::history_runtime::clear_all();
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.search.confirm_clear = false;
                shell.search.notice = result.err();
            }
        });
    }
    sync();
}

fn restore_search_input_if_needed() {
    let controls = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref().filter(|shell| shell.search.open)?;
        Some((shell.window.clone(), shell.search_field.clone()))
    });
    let Some((window, field)) = controls else {
        return;
    };
    if !window.isKeyWindow() {
        return;
    }
    // Rebuilding results or hiding an inline confirmation may remove the
    // focused button. Keep valid editors/controls, including their selection.
    let missing = window.firstResponder().is_none_or(|responder| {
        responder
            .downcast_ref::<NSWindow>()
            .is_some_and(|current| std::ptr::eq(current, &*window))
            || responder.downcast_ref::<NSView>().is_some_and(|view| {
                view.window().as_deref() != Some(&*window) || view.isHiddenOrHasHiddenAncestor()
            })
    });
    if missing
        && SHELL.with(|slot| {
            slot.borrow().as_ref().is_some_and(|shell| {
                shell.search.open && std::ptr::eq(&*shell.search_field, &*field)
            })
        })
    {
        window.makeFirstResponder(Some(&field));
    }
}

fn close_tab_search(restore_focus: bool) {
    let window = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.search.open {
            return None;
        }
        shell.search.close();
        shell.search_results.clear();
        shell.search_signature.clear();
        shell.clear_search_rows();
        Some(shell.window.clone())
    });
    if let Some(window) = window {
        window.makeFirstResponder(None);
        sync();
        apply_selection(restore_focus);
    }
}

fn search_text_changed() {
    if search_has_marked_text() {
        return;
    }
    let controls = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| shell.search.open)
            .map(|shell| (shell.search_field.clone(), shell.search.revision))
    });
    let Some((field, revision)) = controls else {
        return;
    };
    let text = field.stringValue().to_string();
    let bounded = tab_search::bounded_query(&text);
    let accepted = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(shell) = slot.as_mut().filter(|shell| {
            shell.search.open
                && shell.search.revision == revision
                && std::ptr::eq(&*shell.search_field, &*field)
        }) else {
            return false;
        };
        shell.search.set_query(&text);
        true
    });
    if !accepted {
        return;
    }
    if text != bounded {
        field.setStringValue(&NSString::from_str(&bounded));
    }
    sync();
}

fn move_search_selection(forward: bool) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut().filter(|shell| shell.search.open) {
            shell.search.move_selection(&shell.search_results, forward);
        }
    });
    sync();
    let target = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let row = shell
            .search_rows
            .iter()
            .find(|row| Some(row.id) == shell.search.selected)?;
        Some((shell.search_document.clone(), row.button.frame()))
    });
    if let Some((document, frame)) = target {
        document.scrollRectToVisible(frame);
    }
}

fn submit_tab_search() {
    if search_has_marked_text() {
        return;
    }
    let result = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|shell| search_submission(&shell.search))
    });
    if let Some(result) = result {
        activate_search_result(result);
    }
}

fn search_submission(state: &SearchState) -> Option<SearchResultId> {
    let result = (state.open && !state.confirm_clear)
        .then_some(state.selected)
        .flatten()?;
    if let SearchResultId::Query(revision) = result {
        if state.revision != revision || state.query_destination().ok()?.is_none() {
            return None;
        }
    }
    Some(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchQueryKey {
    Move(bool),
    Submit,
    Consume,
}

fn search_query_key(key: u16, query_focus: bool, confirm_clear: bool) -> Option<SearchQueryKey> {
    if !query_focus || !matches!(key, 36 | 76 | 125 | 126) {
        return None;
    }
    if confirm_clear {
        return Some(SearchQueryKey::Consume);
    }
    Some(match key {
        125 => SearchQueryKey::Move(true),
        126 => SearchQueryKey::Move(false),
        _ => SearchQueryKey::Submit,
    })
}

fn search_query_has_focus() -> bool {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref().filter(|shell| shell.search.open)?;
        Some((shell.window.clone(), shell.search_field.clone()))
    });
    let Some((window, field)) = controls else {
        return false;
    };
    if !window.isKeyWindow() {
        return false;
    }
    // AppKit shares a field editor. Its existence alone must not route a
    // focused button's Return or arrow keys through the query's selection.
    let focused = unsafe {
        let editor: *mut AnyObject = msg_send![&*field, currentEditor];
        let responder: *mut AnyObject = msg_send![&*window, firstResponder];
        !editor.is_null() && editor == responder
    };
    // Native getters run outside the borrow and may reenter the shell.
    focused
        && SHELL.with(|slot| {
            slot.try_borrow().ok().is_some_and(|slot| {
                slot.as_ref().is_some_and(|shell| {
                    shell.search.open
                        && std::ptr::eq(&*shell.window, &*window)
                        && std::ptr::eq(&*shell.search_field, &*field)
                })
            })
        })
}

fn select_search_result(sender: &NSButton) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let result = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref().filter(|shell| shell.search.open)?;
        let row = shell.search_rows.get(usize::try_from(index).ok()?)?;
        // Refresh can replace an index before a queued native action arrives.
        std::ptr::eq(sender, &*row.button).then_some(row.id)
    });
    if let Some(result) = result {
        activate_search_result(result);
    }
}

fn activate_search_result(result: SearchResultId) {
    // Both backends revalidate live IDs. No shell borrow crosses either call.
    let opened = match result {
        SearchResultId::Tab(tab) => crate::browser::shell_action(ShellAction::SelectSearchTab(tab)),
        SearchResultId::Bookmark(id) => crate::bookmark_runtime::open(id),
        SearchResultId::History(id) => crate::history_runtime::open(id),
        SearchResultId::Query(revision) => return activate_search_query(revision),
    };
    match opened {
        Ok(()) => close_tab_search(true),
        Err(error) => {
            sync();
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow().as_ref() {
                    let message = format!("이 결과를 열지 못했습니다: {error} · 다른 검색 결과를 선택하세요");
                    shell.search_hint.setStringValue(&NSString::from_str(&compact(&message, 108)));
                    shell.search_hint.setToolTip(Some(&NSString::from_str(&message)));
                    unsafe { let _: () = msg_send![&*shell.search_hint, setAccessibilityHelp: &*NSString::from_str(&message)]; }
                }
            });
        }
    }
}

fn search_query_is_current(
    target: &SearchQueryTarget,
    selection: &(String, Option<i32>, bool),
    window: &NSWindow,
    field: &NSTextField,
) -> bool {
    let current = || {
        SHELL.with(|slot| {
            slot.try_borrow().ok().is_some_and(|slot| {
                slot.as_ref().is_some_and(|shell| {
                    search_query_target_matches(&shell.search, target)
                        && shell.active_space == selection.0
                        && shell.active == selection.1
                        && shell.overview == selection.2
                        && !shell.bookmarks.state.open
                        && !shell.downloads.open
                        && !shell.site_permissions.open
                        && shell
                            .search_results
                            .contains(&SearchResultId::Query(target.revision))
                        && std::ptr::eq(&*shell.window, window)
                        && std::ptr::eq(&*shell.search_field, field)
                })
            })
        })
    };
    if !current()
        || !window.isKeyWindow()
        || field.window().as_deref() != Some(window)
        || field.isHiddenOrHasHiddenAncestor()
        || field.stringValue().to_string() != target.query
    {
        return false;
    }
    // Retain the shared field editor while asking AppKit about composition.
    // These native getters run without a shell borrow and can reenter.
    let editor: Option<Retained<AnyObject>> = unsafe { msg_send![field, currentEditor] };
    let composing = editor.is_some_and(|editor| unsafe { msg_send![&*editor, hasMarkedText] });
    !composing && current()
}

fn activate_search_query(revision: u64) {
    let controls = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let target = search_query_target(&shell.search, revision)?;
        Some((
            target,
            (shell.active_space.clone(), shell.active, shell.overview),
            shell.window.clone(),
            shell.search_field.clone(),
        ))
    });
    let Some((target, selection, window, field)) = controls else {
        return;
    };
    if !search_query_is_current(&target, &selection, &window, &field) {
        return;
    }
    let target = SHELL.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        select_query_submission(&mut slot.as_mut()?.search, target)
    });
    let Some(target) = target else { return };
    // Resolve only the current explicit submission. Rendering classifies the
    // row but never opens a URL, creates a tab, or requests an AI handoff.
    let Ok(Some((url, _))) = crate::address::resolve_with_kind(&target.query) else {
        return;
    };
    let opened = crate::browser::open_search_url(&url, || {
        search_query_is_current(&target, &selection, &window, &field)
    });
    let current = SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref().is_some_and(|shell| {
                search_query_target_matches(&shell.search, &target)
                    && std::ptr::eq(&*shell.window, &*window)
                    && std::ptr::eq(&*shell.search_field, &*field)
            })
        })
    });
    if !current {
        return;
    }
    match opened {
        Ok(()) => close_tab_search(true),
        Err(error) => {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.search.notice =
                        Some(format!("주소 또는 웹 검색을 열지 못했습니다: {error}"));
                }
            });
            sync();
        }
    }
}

fn search_has_marked_text() -> bool {
    let field = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|shell| shell.search_field.clone())
    });
    field.is_some_and(|field| unsafe {
        let editor: *mut AnyObject = msg_send![&*field, currentEditor];
        !editor.is_null() && msg_send![editor, hasMarkedText]
    })
}

pub fn select(id: i32) {
    run_action(Some(ShellAction::SelectTab(id)));
}

fn apply_selection(focus: bool) {
    let find_editing = page_find_is_editing();
    let state = SHELL.with(|slot| {
        slot.borrow().as_ref().map(|shell| {
            (
                shell.window.clone(),
                if shell.overview
                    || (shell.overlay_is_open() && !shell.page_is_covered_by_site_permissions())
                {
                    None
                } else {
                    shell.active
                },
                shell.overlay_is_open(),
                shell.start_page.visible,
                shell.dialog.shown.is_some() && !shell.overview,
                shell
                    .views
                    .iter()
                    .map(|(id, view)| (*id, view.clone()))
                    .collect::<Vec<_>>(),
            )
        })
    });
    let Some((window, active, search_open, start_visible, dialog_open, views)) = state else {
        return;
    };
    if focus && active.is_none() && !search_open && !find_editing {
        window.makeFirstResponder(None);
    }
    for (id, view) in views {
        view.setHidden(start_visible || active != Some(id));
    }
    if start_visible {
        if let Some(id) = active {
            // A first blank load can expose the native page without changing
            // selection. Preserve native editors, but release a now-hidden CEF
            // responder even when this sync was only a background load update.
            focus_start_page_if_visible(id);
        }
        return;
    }
    // Focus WebContents through CEF, after releasing all borrowed/retained views.
    // A presented dialog keeps its own controls focused; the blocked page cannot use focus.
    if focus && !search_open && !find_editing && !dialog_open {
        if let Some(id) = active {
            crate::browser::focus_tab(id);
        }
    }
}

/// Native layout updates child CEF views directly; OSR resize APIs do not apply.
fn layout() {
    let ui = SHELL.with(|slot| {
        slot.borrow().as_ref().map(|shell| {
            (
                shell.window.clone(),
                shell.root.clone(),
                shell.sidebar.clone(),
                shell.toolbar.clone(),
                shell.content.clone(),
                shell.overview_scroll.clone(),
                shell.sidebar_collapsed,
                shell.overview,
                shell.search.open,
                shell.search_surface.clone(),
                shell.bookmarks.state.open,
                shell.bookmarks.surface.clone(),
                shell.downloads.open,
                shell.downloads.surface.clone(),
                shell.site_permissions.open,
                shell.site_permissions.surface.clone(),
                shell.find.state.open,
                shell.find.surface.clone(),
                shell.recovery.target.is_some(),
                shell.recovery.surface.clone(),
                shell.dialog.shown.is_some(),
                shell.dialog.surface.clone(),
                shell.start_page.visible,
                shell.start_page.surface.clone(),
                shell.start_page.input.clone(),
                shell.views.values().cloned().collect::<Vec<_>>(),
            )
        })
    });
    let Some((
        window,
        root,
        sidebar,
        toolbar,
        content,
        overview_scroll,
        collapsed,
        overview,
        search_open,
        search_surface,
        bookmarks_open,
        bookmarks_surface,
        downloads_open,
        downloads_surface,
        site_permissions_open,
        site_permissions_surface,
        find_open,
        find_surface,
        recovery_open,
        recovery_surface,
        dialog_open,
        dialog_surface,
        start_page_open,
        start_page_surface,
        start_page_input,
        views,
    )) = ui
    else {
        return;
    };
    let size = root.bounds().size;
    let side = if collapsed { RAIL_WIDTH } else { SIDEBAR_WIDTH };
    let main_width = (size.width - side).max(1.0);
    let toolbar_bottom = (size.height - TOOLBAR_HEIGHT).max(1.0);
    let overlay_open = search_open || bookmarks_open || downloads_open || site_permissions_open;
    // Permission decisions must reach a still-visible document. The opaque
    // sibling permission surface covers CEF without Blink's hidden/unhidden
    // transition; every other overlay keeps the previous hiding behavior.
    let permission_covers_page =
        site_permissions_open && !overview && !search_open && !bookmarks_open && !downloads_open;
    let find_visible = find_open && !overview && !overlay_open;
    let recovery_visible = recovery_open && !overview && !overlay_open;
    let dialog_visible = dialog_open && !overview && !overlay_open;
    let start_page_visible = start_page_open && !overview && !overlay_open;
    let content_hidden =
        overview || (overlay_open && !permission_covers_page) || start_page_visible;
    let visibility_changed = sidebar.isHidden() != overlay_open
        || toolbar.isHidden() != overlay_open
        || content.isHidden() != content_hidden
        || overview_scroll.isHidden() != (!overview || overlay_open)
        || find_surface.isHidden() == find_visible
        || recovery_surface.isHidden() == recovery_visible
        || dialog_surface.isHidden() == dialog_visible
        || start_page_surface.isHidden() == start_page_visible
        || search_surface.isHidden() == search_open
        || bookmarks_surface.isHidden() == bookmarks_open
        || downloads_surface.isHidden() == downloads_open
        || site_permissions_surface.isHidden() == site_permissions_open;
    let vertical =
        page_vertical_layout(size.height, find_visible, recovery_visible, dialog_visible);
    let page_height = vertical.page_height;
    sidebar.setFrame(rect(0.0, 0.0, side, size.height));
    toolbar.setFrame(rect(side, toolbar_bottom, main_width, TOOLBAR_HEIGHT));
    find_surface.setFrame(rect(side, vertical.find_y, main_width, FIND_BAR_HEIGHT));
    find_surface.setHidden(!find_visible);
    recovery_surface.setFrame(rect(
        side,
        vertical.recovery_y,
        main_width,
        PAGE_RECOVERY_HEIGHT,
    ));
    if !recovery_visible {
        // A reused hidden button must not remain the keyboard destination. This
        // only clears responders inside this bar, never another native editor.
        clear_view_responders(&recovery_surface);
    }
    recovery_surface.setHidden(!recovery_visible);
    dialog_surface.setFrame(rect(
        side,
        vertical.dialog_y,
        main_width,
        PAGE_DIALOG_HEIGHT,
    ));
    if !dialog_visible {
        clear_view_responders(&dialog_surface);
    }
    dialog_surface.setHidden(!dialog_visible);
    sidebar.setHidden(overlay_open);
    toolbar.setHidden(overlay_open);
    content.setFrame(rect(side, 0.0, main_width, page_height));
    overview_scroll.setFrame(rect(side, 0.0, main_width, page_height));
    content.setHidden(content_hidden);
    start_page_surface.setFrame(rect(side, 0.0, main_width, page_height));
    if !start_page_visible {
        clear_start_page_responder(&start_page_surface, &start_page_input);
    }
    start_page_surface.setHidden(!start_page_visible);
    overview_scroll.setHidden(!overview || overlay_open);
    search_surface.setHidden(!search_open);
    bookmarks_surface.setHidden(!bookmarks_open);
    downloads_surface.setHidden(!downloads_open);
    site_permissions_surface.setHidden(!site_permissions_open);
    // Only the CEF hierarchy can re-enter browser callbacks during layout.
    // Its frames are set before borrowing the shell for native chrome controls.
    let bounds = content.bounds();
    for view in views {
        view.setFrame(bounds);
    }
    let resized_backgrounds = SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            let (search_changed, new_tab_changed) = shell.layout_chrome(size, side, main_width);
            if find_visible {
                shell.find.layout(main_width);
            }
            if recovery_visible {
                shell.recovery.layout(main_width);
            }
            if dialog_visible {
                shell.dialog.layout(main_width);
            }
            if start_page_visible {
                shell
                    .start_page
                    .layout(NSSize::new(main_width, page_height));
            }
            shell.layout_tabs();
            if shell.overview {
                shell.layout_cards();
            }
            if shell.search.open {
                shell.layout_search(size);
            }
            if shell.bookmarks.state.open {
                shell.bookmarks.layout(size);
            }
            if shell.downloads.open {
                shell.downloads.layout(size);
            }
            if shell.site_permissions.open {
                shell.site_permissions.layout(size);
            }
            [
                search_changed.then(|| shell.search_button_background.clone()),
                new_tab_changed.then(|| shell.new_tab_background.clone()),
            ]
        } else {
            [None, None]
        }
    });
    // A newly presented dialog takes keyboard focus once, and only while it is
    // really shown. Never steal an editing native text field (address, search).
    let dialog_focus = if dialog_visible {
        SHELL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let shell = slot.as_mut()?;
            if !std::mem::take(&mut shell.dialog.focus_pending) {
                return None;
            }
            // A native field editor (NSTextView, an NSText) is first responder.
            let editing = shell.window.firstResponder().is_some_and(|responder| {
                responder.respondsToSelector(sel!(isFieldEditor)) && {
                    let editor: bool = unsafe { msg_send![&*responder, isFieldEditor] };
                    editor
                }
            });
            if editing {
                return None;
            }
            let prompt = shell
                .dialog
                .shown
                .as_ref()
                .is_some_and(|dialog| dialog.kind == crate::dialogs::Kind::Prompt);
            Some((
                shell.window.clone(),
                prompt.then(|| shell.dialog.field.clone()),
                shell.dialog.surface.clone(),
            ))
        })
    } else {
        None
    };
    if visibility_changed {
        // Reused native controls become eligible without being added again.
        // Update traversal only after their visibility and final frames settle.
        window.recalculateKeyViewLoop();
    }
    // Complete only the resized decorative surfaces after all layout and shell
    // borrows finish. Displaying may synchronously invoke AppKit callbacks.
    for background in resized_backgrounds.into_iter().flatten() {
        if !background.isHiddenOrHasHiddenAncestor() {
            background.displayIfNeeded();
        }
    }
    validate_active_tab_drag();
    if let Some((window, field, primary)) = dialog_focus {
        window.recalculateKeyViewLoop();
        match field {
            Some(field) => {
                window.makeFirstResponder(Some(&field));
            }
            None => {
                window.makeFirstResponder(Some(&primary));
            }
        }
    }
}

impl Shell {
    fn layout_chrome(&self, size: NSSize, side: f64, main_width: f64) -> (bool, bool) {
        self.brand_icon
            .setFrame(rect(16.0, size.height - 40.0, 24.0, 24.0));
        self.brand.setFrame(rect(
            48.0,
            size.height - 42.0,
            (side - 100.0).max(1.0),
            28.0,
        ));
        self.sidebar_toggle.setFrame(rect(
            if self.sidebar_collapsed {
                22.0
            } else {
                side - 44.0
            },
            size.height - 44.0,
            32.0,
            32.0,
        ));
        let navigation_x = if self.sidebar_collapsed { 12.0 } else { 24.0 };
        let navigation_width = side - navigation_x * 2.0;
        let search_background_frame = rect(12.0, size.height - 100.0, side - 24.0, 40.0);
        let search_changed = self.search_button_background.frame() != search_background_frame;
        if search_changed {
            self.search_button_background
                .setFrame(search_background_frame);
            self.search_button_background.setNeedsDisplay(true);
        }
        self.search_button.setFrame(rect(
            navigation_x,
            size.height - 100.0,
            navigation_width,
            40.0,
        ));
        self.spaces_background
            .setFrame(rect(12.0, size.height - 144.0, side - 24.0, 40.0));
        self.spaces_button.setFrame(rect(
            navigation_x,
            size.height - 144.0,
            navigation_width,
            40.0,
        ));
        self.space_label
            .setFrame(rect(20.0, size.height - 176.0, side - 40.0, 20.0));
        self.tab_scroll.setFrame(rect(
            8.0,
            196.0,
            side - 16.0,
            (size.height - if self.sidebar_collapsed { 360.0 } else { 384.0 }).max(1.0),
        ));
        self.sidebar_separator
            .setFrame(rect(16.0, 188.0, side - 32.0, 1.0));
        self.site_permissions_button
            .setFrame(rect(navigation_x, 144.0, navigation_width, 36.0));
        self.bookmarks_button
            .setFrame(rect(navigation_x, 104.0, navigation_width, 36.0));
        self.downloads_button
            .setFrame(rect(navigation_x, 64.0, navigation_width, 36.0));
        let new_tab_background_frame = rect(12.0, 12.0, side - 24.0, 40.0);
        let new_tab_changed = self.new_tab_background.frame() != new_tab_background_frame;
        if new_tab_changed {
            self.new_tab_background.setFrame(new_tab_background_frame);
            self.new_tab_background.setNeedsDisplay(true);
        }
        self.new_tab.setFrame(rect(12.0, 12.0, side - 24.0, 40.0));
        self.back.setFrame(rect(8.0, 12.0, 32.0, 32.0));
        self.forward.setFrame(rect(42.0, 12.0, 32.0, 32.0));
        self.reload.setFrame(rect(76.0, 12.0, 32.0, 32.0));
        let frames = toolbar_frames(main_width, !self.stop.isHidden());
        self.address_surface.setFrame(frames.address);
        self.address_icon.setFrame(rect(12.0, 10.0, 16.0, 16.0));
        self.address.setFrame(rect(
            38.0,
            8.0,
            (frames.address.size.width - 50.0).max(1.0),
            20.0,
        ));
        self.status_surface.setFrame(frames.status);
        self.status
            .setFrame(rect(10.0, 4.0, frames.status.size.width - 20.0, 16.0));
        self.stop.setFrame(frames.stop);
        self.close_tab.setFrame(frames.close);
        (search_changed, new_tab_changed)
    }

    fn layout_tabs(&mut self) {
        let width = self.tab_scroll.contentSize().width.max(1.0);
        let height = self
            .tab_scroll
            .contentSize()
            .height
            .max(self.tab_buttons.len() as f64 * TAB_ROW_HEIGHT + 8.0);
        self.tab_document.setFrame(rect(0.0, 0.0, width, height));
        self.selected_tab_background.setHidden(true);
        for (index, row) in self.tab_buttons.iter().enumerate() {
            let y = 4.0 + index as f64 * TAB_ROW_HEIGHT;
            let frames = tab_row_frames(width, self.sidebar_collapsed, y);
            row.button.setFrame(frames.selection);
            row.mute.setFrame(frames.mute);
            row.close.setFrame(frames.close);
            row.icon.setFrame(frames.icon);
            row.title.setFrame(frames.title);
            if self.active == Some(row.id) {
                self.selected_tab_background.setFrame(frames.background);
                self.selected_tab_background.setHidden(self.overview);
                if self.reveal_active_tab {
                    row.button.scrollRectToVisible(row.button.bounds());
                }
            }
        }
        self.reveal_active_tab = false;
    }

    fn layout_cards(&self) {
        let viewport = self.overview_scroll.contentSize();
        let columns = if viewport.width >= 950.0 {
            3
        } else if viewport.width >= 590.0 {
            2
        } else {
            1
        };
        let rows = self.cards.len().max(1).div_ceil(columns);
        let gap = 18.0;
        let margin = 32.0;
        let width = ((viewport.width - margin * 2.0 - gap * (columns - 1) as f64) / columns as f64)
            .max(200.0);
        let preview_height = ((width - 20.0) * 0.60).clamp(150.0, 220.0);
        let card_height = preview_height + 98.0;
        let height = viewport
            .height
            .max(120.0 + rows as f64 * (card_height + gap));
        self.overview_document
            .setFrame(rect(0.0, 0.0, viewport.width, height));
        self.overview_title.setFrame(rect(margin, 27.0, 70.0, 34.0));
        self.overview_count_surface
            .setFrame(rect(margin + 69.0, 33.0, 44.0, 22.0));
        self.overview_count.setFrame(rect(8.0, 3.0, 28.0, 16.0));
        self.overview_hint
            .setFrame(rect(margin, 66.0, viewport.width - margin * 2.0, 20.0));
        self.overview_previous
            .setFrame(rect(viewport.width - 143.0, 31.0, 28.0, 28.0));
        self.overview_page_label
            .setFrame(rect(viewport.width - 106.0, 36.0, 44.0, 18.0));
        self.overview_next
            .setFrame(rect(viewport.width - 58.0, 31.0, 28.0, 28.0));
        for (index, card) in self.cards.iter().enumerate() {
            card.container.setFrame(rect(
                margin + (index % columns) as f64 * (width + gap),
                108.0 + (index / columns) as f64 * (card_height + gap),
                width,
                card_height,
            ));
            card.preview_surface
                .setFrame(rect(10.0, 88.0, width - 20.0, preview_height));
            layout_preview(card);
            card.placeholder.setFrame(rect(
                16.0,
                (preview_height - 20.0) / 2.0,
                width - 52.0,
                20.0,
            ));
            card.title.setFrame(rect(16.0, 60.0, width - 62.0, 21.0));
            card.detail.setFrame(rect(16.0, 39.0, width - 32.0, 17.0));
            card.badge.setFrame(rect(16.0, 12.0, 92.0, 20.0));
            card.badge_label.setFrame(rect(8.0, 3.0, 76.0, 15.0));
            card.open.setFrame(rect(width - 42.0, 57.0, 28.0, 28.0));
            card.preview_button
                .setFrame(rect(10.0, 88.0, width - 20.0, preview_height));
            card.stop.setFrame(rect(width - 72.0, 7.0, 58.0, 29.0));
        }
    }
}

/// Fill the existing thumbnail well without changing its aspect ratio. The
/// masked parent clips the bottom/side overflow, keeping the page's top visible.
fn layout_preview(card: &SpaceCard) {
    let bounds = card.preview_surface.bounds();
    let Some(image) = &card.preview else {
        card.image.setFrame(bounds);
        return;
    };
    let size = image.size();
    if size.width <= 0.0 || size.height <= 0.0 {
        return;
    }
    let scale = (bounds.size.width / size.width).max(bounds.size.height / size.height);
    let width = size.width * scale;
    let height = size.height * scale;
    card.image.setFrame(rect(
        (bounds.size.width - width) / 2.0,
        bounds.size.height - height,
        width,
        height,
    ));
}

fn page_spaces(forward: bool) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            let last = shell.spaces.len().saturating_sub(1) / PREVIEW_LIMIT;
            shell.overview_page = if forward {
                (shell.overview_page + 1).min(last)
            } else {
                shell.overview_page.saturating_sub(1)
            };
            shell.rebuild_cards();
        }
    });
    layout();
    let document = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|shell| shell.overview_document.clone())
    });
    if let Some(document) = document {
        document.scrollPoint(NSPoint::new(0.0, 0.0));
    }
    crate::browser::request_space_previews();
    crate::browser::notify_panel();
}

/// Only visible overview cards may request a bounded, on-demand thumbnail.
pub fn preview_candidates() -> Vec<(String, i32)> {
    let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
    if NSApp(marker).isHidden() {
        return Vec::new();
    }
    SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| {
                shell.overview
                    && !shell.overlay_is_open()
                    && shell.window.isVisible()
                    && !shell.window.isMiniaturized()
            })
            .map(|shell| {
                shell
                    .cards
                    .iter()
                    .filter(|card| card.preview.is_none() && card.url != "about:blank")
                    .filter_map(|card| card.tab.map(|tab| (card.space.clone(), tab)))
                    .take(PREVIEW_LIMIT)
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// A late capture cannot populate an old space, a different tab, or a hidden overview.
pub fn set_preview(space: &str, tab: i32, png: &[u8]) {
    if !bounded_preview_png(png) {
        return;
    }
    MainThreadMarker::new().expect("Browser shell requires the main thread");
    let accepts = SHELL.with(|slot| {
        slot.borrow().as_ref().is_some_and(|shell| {
            shell.overview
                && !shell.overlay_is_open()
                && shell
                    .cards
                    .iter()
                    .any(|card| card.space == space && card.tab == Some(tab))
        })
    });
    if !accepts {
        return;
    }
    let data = NSData::with_bytes(png);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    SHELL.with(|slot| {
        if let Some(card) = slot
            .borrow_mut()
            .as_mut()
            .filter(|shell| shell.overview && !shell.overlay_is_open())
            .and_then(|shell| {
                shell
                    .cards
                    .iter_mut()
                    .find(|card| card.space == space && card.tab == Some(tab))
            })
        {
            card.image.setImage(Some(&image));
            card.placeholder.setHidden(true);
            card.preview = Some(image);
            layout_preview(card);
        }
    });
}

fn bounded_preview_png(png: &[u8]) -> bool {
    png.len() >= 24
        && png.len() <= PREVIEW_BYTES_LIMIT
        && png[..8] == *b"\x89PNG\r\n\x1a\n"
        && png[12..16] == *b"IHDR"
        && (1..=640).contains(&u32::from_be_bytes(png[16..20].try_into().unwrap()))
        && (1..=480).contains(&u32::from_be_bytes(png[20..24].try_into().unwrap()))
}

pub fn show() {
    ensure_shell();
    let window = SHELL.with(|slot| slot.borrow().as_ref().unwrap().window.clone());
    if window.isMiniaturized() {
        window.deminiaturize(None);
    }
    #[allow(deprecated)]
    NSApp(MainThreadMarker::new().unwrap()).activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    sync();
}

pub(crate) fn file_selection_window() -> Option<Retained<NSWindow>> {
    SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        slot.as_ref().map(|shell| shell.window.clone())
    })
}

pub fn refresh() {
    let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
    let window = SHELL.with(|slot| slot.borrow().as_ref().map(|shell| shell.window.clone()));
    let visible = window.is_some_and(|window| window.isVisible() && !window.isMiniaturized());
    if visible && !NSApp(marker).isHidden() {
        sync();
    }
}

pub fn sync() {
    // Read a fresh browser snapshot after CEF setting queries, which may reenter
    // and change selection/navigation while the download panel is open.
    let site = download_site_snapshot_ui();
    let permission_site = site_permission_site_snapshot_ui();
    let Some(snapshot) = crate::browser::shell_snapshot() else {
        return;
    };
    let end_start_editor = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let next_target =
            start_page_target(&snapshot.tabs, &snapshot.active_space, snapshot.active_tab);
        let will_hide = snapshot.overview || shell.overlay_is_open() || next_target.is_none();
        (shell.start_page.target != next_target || (shell.start_page.visible && will_hide)).then(
            || {
                (
                    shell.start_page.surface.clone(),
                    shell.start_page.input.clone(),
                )
            },
        )
    });
    if let Some((surface, input)) = end_start_editor {
        // Commit the editor before render disables or replaces its field.
        // Same-document overlays keep the draft; a new target resets it below
        // without carrying the old document's IME marked range into another tab.
        clear_start_page_responder(&surface, &input);
    }
    let invalidate_find = SHELL.with(|slot| {
        let slot = slot.borrow();
        let Some(shell) = slot.as_ref() else {
            return false;
        };
        shell.find.state.target.is_some_and(|(tab, epoch)| {
            snapshot.overview
                || shell.overlay_is_open()
                || shell.active_space != snapshot.active_space
                || snapshot.active_tab != Some(tab)
                || !snapshot
                    .tabs
                    .iter()
                    .any(|item| item.id == tab && item.navigation_epoch == epoch && !item.crashed)
        })
    });
    if invalidate_find {
        close_page_find(false, false);
    }
    let (bookmark_query, history_query) = SHELL.with(|slot| {
        let slot = slot.borrow();
        let Some(shell) = slot.as_ref() else {
            return (None, None);
        };
        if shell.search.open {
            (
                (shell.search.mode == SearchMode::All).then(|| shell.search.query.clone()),
                Some(shell.search.query.clone()),
            )
        } else if shell.bookmarks.state.open {
            (Some(shell.bookmarks.state.query.clone()), None)
        } else {
            (None, None)
        }
    });
    let bookmarks = bookmark_query.map(|query| crate::bookmark_runtime::snapshot(&query));
    let history = history_query.map(|query| crate::history_runtime::snapshot(&query));
    let download_activity = crate::download_runtime::activity();
    let download_permission_count = crate::download_permission_runtime::pending_count();
    let file_selection_count = crate::file_selection_runtime::pending_count();
    let file_selection_notice = crate::file_selection_runtime::notice();
    let site_permission_count = crate::site_permission_runtime::pending_count();
    let site_permission_notice = crate::site_permission_runtime::notice();
    let site_permission_media = crate::site_permission_runtime::media_summary();
    let site_permissions =
        site_permissions_are_open().then(crate::site_permission_runtime::snapshot);
    let downloads = downloads_are_open().then(|| {
        (
            crate::download_runtime::snapshot(),
            crate::download_permission_runtime::snapshot(),
            crate::file_selection_runtime::snapshot(),
        )
    });
    let mut download_focus = None;
    let mut permission_focus = None;
    let mut tab_focus = None;
    let changed = SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            let changed =
                shell.active != snapshot.active_tab || shell.overview != snapshot.overview;
            shell.download_activity = download_activity;
            shell.download_permission_count = download_permission_count;
            shell.file_selection_count = file_selection_count;
            shell.file_selection_notice = file_selection_notice;
            shell.site_permission_count = site_permission_count;
            shell.site_permission_notice = site_permission_notice;
            shell.site_permission_media = site_permission_media;
            shell.render(snapshot);
            if std::mem::take(&mut shell.tab_focus_removed)
                && !changed
                && !shell.overview
                && !shell.overlay_is_open()
            {
                let control = shell
                    .tab_buttons
                    .iter()
                    .find(|row| Some(row.id) == shell.active)
                    .map(|row| row.button.clone())
                    .unwrap_or_else(|| shell.new_tab.clone());
                tab_focus = Some((shell.window.clone(), control));
            }
            if shell.search.open {
                if let Some(history) = history {
                    shell.rebuild_search_results(bookmarks, history);
                }
            } else if let Some(bookmarks) = bookmarks {
                if shell.bookmarks.state.open {
                    shell.bookmarks.render(bookmarks, &shell.target);
                }
            }
            if let Some((downloads, permissions, file_selections)) = downloads {
                let expected = download_site_page_target(
                    &shell.tabs,
                    &shell.active_space,
                    shell.active,
                    shell.overview,
                    shell.downloads.open,
                    shell.search.open || shell.bookmarks.state.open,
                );
                let site = site.for_target(
                    expected.as_ref(),
                    shell.downloads.site.current.as_ref().map(|(page, _)| page),
                );
                if shell.downloads.open
                    && shell.downloads.render(
                        downloads,
                        permissions,
                        file_selections,
                        shell.file_selection_notice.clone(),
                        site,
                        &shell.target,
                        &shell.window,
                    )
                {
                    download_focus = Some((shell.window.clone(), shell.downloads.close.clone()));
                }
            }
            if let Some(requests) = site_permissions {
                let expected = site_permission_page_target(shell);
                let site = permission_site.for_target(
                    expected.as_ref(),
                    shell
                        .site_permissions
                        .current
                        .as_ref()
                        .map(|(page, _)| page),
                );
                if shell.site_permissions.open
                    && shell.site_permissions.render(
                        requests,
                        shell.site_permission_notice.clone(),
                        shell.site_permission_media.clone(),
                        site,
                        &shell.target,
                        &shell.window,
                    )
                {
                    permission_focus =
                        Some((shell.window.clone(), shell.site_permissions.close.clone()));
                }
            }
            changed
        } else {
            false
        }
    });
    layout();
    apply_selection(changed);
    restore_search_input_if_needed();
    if let Some((window, control)) = tab_focus.filter(|(_, control)| {
        SHELL.with(|slot| {
            slot.borrow().as_ref().is_some_and(|shell| {
                !shell.overview
                    && !shell.overlay_is_open()
                    && shell
                        .tab_buttons
                        .iter()
                        .find(|row| Some(row.id) == shell.active)
                        .map(|row| std::ptr::eq(&*row.button, &**control))
                        .unwrap_or_else(|| std::ptr::eq(&*shell.new_tab, &**control))
            })
        })
    }) {
        window.makeFirstResponder(Some(&control));
    }
    if let Some((window, close)) = download_focus.filter(|_| downloads_are_open()) {
        window.makeFirstResponder(Some(&close));
    }
    if let Some((window, close)) = permission_focus.filter(|_| site_permissions_are_open()) {
        window.makeFirstResponder(Some(&close));
    }
    crate::browser::request_space_previews();
}

fn clear_start_page_responder(surface: &NSView, input: &NSTextField) {
    if let Some(window) = surface.window() {
        // The shared AppKit field editor need not be a descendant of its field.
        // Match that exact current editor before clearing any keyboard focus.
        let owns_editor = unsafe {
            let editor: *mut AnyObject = msg_send![input, currentEditor];
            let responder: *mut AnyObject = msg_send![&*window, firstResponder];
            !editor.is_null() && editor == responder
        };
        if owns_editor {
            window.makeFirstResponder(None);
        }
    }
    clear_view_responders(surface);
}

fn current_start_page_target(shell: &Shell) -> Option<StartPageTarget> {
    if !shell.start_page.visible || shell.overview || shell.overlay_is_open() {
        return None;
    }
    let target = start_page_target(&shell.tabs, &shell.active_space, shell.active)?;
    (shell.start_page.target.as_ref() == Some(&target)).then_some(target)
}

pub(crate) fn start_page_is_visible(tab: i32) -> bool {
    SHELL.with(|slot| {
        slot.borrow().as_ref().is_some_and(|shell| {
            current_start_page_target(shell).is_some_and(|target| target.tab == tab)
                && !shell.start_page.surface.isHidden()
        })
    })
}

pub(crate) fn focus_start_page_if_visible(tab: i32) -> bool {
    let controls = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let target = current_start_page_target(shell)?;
        if target.tab != tab || shell.start_page.surface.isHidden() {
            return None;
        }
        Some((
            shell.window.clone(),
            shell.address.clone(),
            shell.content.clone(),
            shell
                .start_page
                .submission
                .as_ref()
                .is_some_and(|submission| {
                    submission.tab == target.tab && submission.space == target.space
                }),
        ))
    });
    let Some((window, address, content, submitting)) = controls else {
        return false;
    };
    if submitting {
        // AI handoff synchronously refreshes a new epoch before Navigate begins.
        // Do not turn its temporary missing editor into an address edit session.
        return true;
    }
    // Selecting a native start page must never focus its hidden WebContents.
    // Keep an existing native editor/control; only replace a missing or hidden
    // responder with the usual toolbar address. The center input is click-first.
    if window.isVisible() && !window.isMiniaturized() {
        let native_responder = window.firstResponder().is_some_and(|responder| {
            responder.downcast_ref::<NSView>().is_some_and(|view| {
                !view.isHiddenOrHasHiddenAncestor() && !view.isDescendantOf(&content)
            })
        });
        if !native_responder {
            trace_focus("start-fallback-before", Some(tab), None, None);
            window.makeFirstResponder(Some(&address));
            trace_focus("start-fallback-after", Some(tab), None, None);
        }
    }
    true
}

fn start_page_action(sender: &AnyObject, bookmarks: bool) {
    let candidate = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let page = &shell.start_page;
        let matches = if bookmarks {
            sender
                .downcast_ref::<NSButton>()
                .is_some_and(|sender| std::ptr::eq(sender, &*page.bookmarks) && sender.isEnabled())
        } else {
            sender
                .downcast_ref::<NSButton>()
                .is_some_and(|sender| std::ptr::eq(sender, &*page.open) && sender.isEnabled())
                || sender
                    .downcast_ref::<NSTextField>()
                    .is_some_and(|sender| std::ptr::eq(sender, &*page.input) && sender.isEnabled())
        };
        if !matches
            || page.submission.is_some()
            || page.surface.isHidden()
            || field_has_marked_text(&page.input)
        {
            return None;
        }
        Some((
            current_start_page_target(shell)?,
            page.input.stringValue().to_string(),
        ))
    });
    let Some((target, input)) = candidate else {
        return;
    };
    // Both backends validate the live document before any owner handoff. Native
    // field state is never borrowed across browser callbacks or overlay actions.
    let empty_input = input.trim().is_empty();
    let result = if bookmarks || empty_input {
        crate::browser::validate_start_page_target(&target)
    } else {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.start_page.submission = Some(target.clone());
            }
        });
        let result = crate::browser::navigate_from_start_page(&target, &input);
        // End this synchronous scope on both success and failure. No shell
        // borrow crosses validation, owner handoff, or CEF navigation callbacks.
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.start_page.submission = None;
            }
        });
        result
    };
    if let Err(error) = result {
        let controls = SHELL.with(|slot| {
            let slot = slot.borrow();
            let shell = slot.as_ref()?;
            if current_start_page_target(shell).as_ref() != Some(&target) {
                return None;
            }
            let message = format!("열지 못했습니다: {error}");
            shell
                .start_page
                .hint
                .setStringValue(&NSString::from_str(&compact(&message, 120)));
            shell
                .start_page
                .hint
                .setToolTip(Some(&NSString::from_str(&message)));
            shell
                .start_page
                .hint
                .setTextColor(Some(&NSColor::systemRedColor()));
            Some((shell.window.clone(), shell.start_page.input.clone()))
        });
        refresh();
        if let Some((window, field)) = controls.filter(|_| {
            SHELL.with(|slot| {
                slot.borrow().as_ref().is_some_and(|shell| {
                    current_start_page_target(shell).as_ref() == Some(&target)
                        && !shell.start_page.surface.isHidden()
                })
            })
        }) {
            window.makeFirstResponder(Some(&field));
        }
        return;
    }
    if bookmarks {
        open_bookmarks(false);
    } else if empty_input {
        // Return can end the native editor. An empty submission keeps editing
        // this verified document instead of refreshing into toolbar focus.
        let controls = SHELL.with(|slot| {
            let slot = slot.borrow();
            let shell = slot.as_ref()?;
            (current_start_page_target(shell).as_ref() == Some(&target)
                && !shell.start_page.surface.isHidden())
            .then(|| (shell.window.clone(), shell.start_page.input.clone()))
        });
        if let Some((window, field)) = controls {
            window.makeFirstResponder(Some(&field));
        }
    } else {
        refresh();
        let Some(selected) = crate::browser::shell_snapshot() else {
            return;
        };
        let focus_page = selected.active_tab == Some(target.tab)
            && selected.active_space == target.space
            && !selected.overview
            && SHELL.with(|slot| {
                slot.borrow().as_ref().is_some_and(|shell| {
                    shell.active == Some(target.tab)
                        && shell.active_space == target.space
                        && !shell.overview
                        && !shell.overlay_is_open()
                        && !shell.start_page.visible
                })
            });
        if focus_page {
            crate::browser::focus_tab(target.tab);
        }
    }
}

/// The user pressed a button of the page dialog band (or Return in its field).
fn page_dialog_action(sender: &NSObject, accept: bool) {
    answer_page_dialog(accept, Some(sender));
}

/// Answer the shown dialog. A click is accepted only from the matching control
/// and only for the exact dialog and tab currently shown; the key path passes
/// `None` after `dialog_key_decision` approved the focus scope.
fn answer_page_dialog(accept: bool, sender: Option<&NSObject>) {
    let request = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let bar = &shell.dialog;
        let shown = bar.shown.as_ref()?;
        if bar.surface.isHidden() {
            return None;
        }
        if let Some(sender) = sender {
            let origin = std::ptr::from_ref(sender).cast::<u8>();
            let is = |view: *const u8| std::ptr::eq(origin, view);
            let valid = if accept {
                is(std::ptr::from_ref(&*bar.primary).cast())
                    || is(std::ptr::from_ref(&*bar.field).cast())
            } else {
                is(std::ptr::from_ref(&*bar.cancel).cast())
            };
            if !valid {
                return None;
            }
        }
        if !accept && !bar.cancel.isEnabled() {
            return None;
        }
        let live = visible_user_dialog_tab(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )?;
        if live != shown.tab {
            return None;
        }
        let text = (accept && shown.kind == crate::dialogs::Kind::Prompt)
            .then(|| bar.field.stringValue().to_string());
        Some((shown.tab, shown.token, text))
    });
    let Some((tab, token, text)) = request else {
        return;
    };
    // The callback continues here, outside every SHELL/STATE borrow.
    let result = crate::browser::answer_user_dialog(tab, token, accept, text);
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.dialog.draft = None;
            if result.is_err() {
                shell.notice = Some("대화상자가 이미 닫혀 응답하지 않았습니다.".into());
            }
        }
    });
    refresh();
    if result.is_ok() {
        crate::browser::focus_tab(tab);
    }
}

/// Return/Esc for the visible dialog strip, from `handle_shortcut`. True when
/// the key was consumed.
fn handle_dialog_key(event: &NSEvent) -> bool {
    let scope = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let shown = shell.dialog.shown.as_ref()?;
        let visible = !shell.dialog.surface.isHidden()
            && visible_user_dialog_tab(
                &shell.tabs,
                &shell.active_space,
                shell.active,
                shell.overview,
                shell.overlay_is_open(),
            ) == Some(shown.tab);
        let responder = shell.window.firstResponder();
        let in_scope = responder.as_ref().is_some_and(|responder| {
            responder.downcast_ref::<NSView>().is_some_and(|view| {
                view.isDescendantOf(&shell.dialog.surface)
                    || shell
                        .views
                        .get(&shown.tab)
                        .is_some_and(|page| view.isDescendantOf(page))
            })
        });
        let marked = unsafe {
            let editor: *mut AnyObject = msg_send![&*shell.dialog.field, currentEditor];
            !editor.is_null() && msg_send![editor, hasMarkedText]
        };
        Some((visible, in_scope, marked, shell.dialog.cancel.isEnabled()))
    });
    let Some((visible, in_scope, marked, cancel_enabled)) = scope else {
        return false;
    };
    let modifiers = event.modifierFlags();
    let unmodified = !modifiers.intersects(
        NSEventModifierFlags::Command
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Shift,
    );
    match dialog_key_decision(
        event.keyCode(),
        unmodified,
        visible,
        in_scope,
        marked,
        cancel_enabled,
    ) {
        Some(DialogKey::Primary) => {
            answer_page_dialog(true, None);
            true
        }
        Some(DialogKey::Cancel) => {
            answer_page_dialog(false, None);
            true
        }
        None => false,
    }
}

/// `focus_tab` entry: keep the keyboard on the dialog strip instead of the
/// blocked page. True when the strip handled focus.
pub(crate) fn focus_dialog_if_visible(tab: i32) -> bool {
    let state = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let shown = shell.dialog.shown.as_ref()?;
        if shell.dialog.surface.isHidden() {
            return None;
        }
        let responder_in_strip = shell.window.firstResponder().is_some_and(|responder| {
            responder
                .downcast_ref::<NSView>()
                .is_some_and(|view| view.isDescendantOf(&shell.dialog.surface))
        });
        let prompt = shown.kind == crate::dialogs::Kind::Prompt;
        Some((
            shown.tab,
            responder_in_strip,
            shell.window.clone(),
            shell.dialog.surface.clone(),
            prompt.then(|| shell.dialog.field.clone()),
        ))
    });
    let Some((strip_tab, in_strip, window, surface, field)) = state else {
        return false;
    };
    match dialog_focus_decision(Some(strip_tab), tab, in_strip) {
        DialogFocus::Page => false,
        DialogFocus::Keep => true,
        DialogFocus::Take => {
            match field {
                Some(field) => window.makeFirstResponder(Some(&field)),
                None => window.makeFirstResponder(Some(&surface)),
            };
            true
        }
    }
}

fn page_recovery_action(sender: &NSButton, retry: bool) {
    let target = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let bar = &shell.recovery;
        let button = if retry {
            &*bar.retry
        } else {
            &*bar.edit_address
        };
        if !std::ptr::eq(sender, button) || !sender.isEnabled() || bar.surface.isHidden() {
            return None;
        }
        let target = visible_page_recovery_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )?;
        (bar.target.as_ref() == Some(&target)).then_some(target)
    });
    let Some(target) = target else {
        return;
    };
    // The cached view can lag behind navigation or a task transfer. Both paths
    // resolve the live identity outside the shell borrow before taking action.
    let result = if retry {
        crate::browser::retry_failed_page(&target)
    } else {
        crate::browser::validate_failed_page_target(&target)
    };
    if result.is_err() {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                if shell.active == Some(target.tab) && shell.active_space == target.space {
                    shell.notice = Some(
                        if retry {
                            "페이지 상태가 바뀌어 새로 고침하지 않았습니다."
                        } else {
                            "페이지 상태가 바뀌어 주소 입력으로 이동하지 않았습니다."
                        }
                        .into(),
                    );
                }
            }
        });
        refresh();
        return;
    }
    if !retry {
        // Focusing the address alone never hands an AI task over to the user.
        focus_address();
        return;
    }
    let clear_find = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(shell) = slot.as_mut() else {
            return false;
        };
        if shell.active != Some(target.tab) || shell.active_space != target.space {
            return false;
        }
        shell.address_draft = None;
        shell.notice = None;
        shell
            .find
            .state
            .target
            .is_some_and(|(tab, _)| tab == target.tab)
    });
    if clear_find {
        close_page_find(false, false);
    }
    refresh();
}

fn tab_row_action(sender: &NSButton, close: bool) {
    let command = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        let tag: isize = unsafe { msg_send![sender, tag] };
        let id = i32::try_from(tag).ok()?;
        let row = shell.tab_buttons.iter().find(|row| row.id == id)?;
        let current = if close { &*row.close } else { &*row.button };
        if !std::ptr::eq(sender, current)
            || !sender.isEnabled()
            || (close && shell.overview)
            || !tab_row_is_current(
                id,
                &row.space,
                &shell.active_space,
                shell.overlay_is_open(),
                &shell.tabs,
            )
        {
            return None;
        }
        let space = row.space.clone();
        let preserve_address = close && shell.active != Some(id);
        if preserve_address && shell.address_draft.is_none() {
            // A mouse click may end the native editor before its action arrives.
            // Freeze its current text exactly as when opening the download panel.
            shell.address_draft = shell
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == shell.active)
                .map(|tab| AddressDraft {
                    tab: tab.id,
                    url: tab.url.clone(),
                    navigation_epoch: tab.navigation_epoch,
                });
        }
        shell.sidebar_address_handoff = preserve_address
            .then(|| shell.tabs.iter().find(|tab| Some(tab.id) == shell.active))
            .flatten()
            .filter(|tab| {
                tab.stop_scope
                    .as_ref()
                    .is_some_and(|scope| scope.task == space)
            })
            .map(|tab| SidebarAddressHandoff {
                tab: tab.id,
                space: space.clone(),
                url: tab.url.clone(),
                epoch: tab.navigation_epoch,
            });
        Some((id, space, preserve_address))
    });
    let Some((id, space, preserve_address)) = command else {
        return;
    };
    if !close {
        select(id);
    } else {
        // Resolve the live space before task handoff: adoption/release can have
        // changed it since this row's snapshot. CEF callbacks stay outside the
        // shell borrow, and the background-close path leaves its editor alone.
        let result = crate::browser::close_tab_from_sidebar(id, &space);
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.sidebar_address_handoff = None;
                match result {
                    Ok(()) if !preserve_address && shell.active == Some(id) => {
                        shell.address_draft = None;
                        shell.notice = None;
                    }
                    Err(error) => shell.notice = Some(format!("처리하지 못했습니다: {error}")),
                    _ => {}
                }
            }
        });
        refresh();
    }
}

fn tab_drag_target_matches(
    target: &crate::browser::TabDragTarget,
    tabs: &[ShellTab],
    space: &str,
    selected: Option<i32>,
    overview: bool,
    overlay: bool,
) -> bool {
    !overview
        && !overlay
        && target.space == space
        && target.selected == selected
        && target.order.len() > 1
        && target.order.contains(&target.tab)
        && tabs
            .iter()
            .filter(|tab| tab.space == space)
            .map(|tab| tab.id)
            .eq(target.order.iter().copied())
}

fn tab_drag_geometry(
    window: &NSWindow,
    scroll: &NSScrollView,
    document: &NSView,
    button: &NSButton,
    clip: &NSClipView,
) -> TabDragGeometry {
    TabDragGeometry {
        window: window.frame(),
        scroll: scroll.frame(),
        document: document.frame(),
        viewport: document.visibleRect(),
        source: button.frame(),
        clip_frame: clip.frame(),
        clip_bounds: clip.bounds(),
    }
}

fn tab_drag_geometry_matches(
    original: &TabDragGeometry,
    actual: &TabDragGeometry,
    viewport: NSRect,
    clip_bounds: NSRect,
) -> bool {
    original.window == actual.window
        && original.scroll == actual.scroll
        && original.document == actual.document
        && original.source == actual.source
        && original.clip_frame == actual.clip_frame
        && original.viewport.origin.x == viewport.origin.x
        && original.viewport.size == viewport.size
        && original.clip_bounds.origin.x == clip_bounds.origin.x
        && original.clip_bounds.size == clip_bounds.size
        && actual.viewport == viewport
        && actual.clip_bounds == clip_bounds
}

fn tab_drag_shell_matches(session: &TabDragSession) -> bool {
    SHELL.with(|slot| {
        let Ok(slot) = slot.try_borrow() else {
            return false;
        };
        let Some(shell) = slot.as_ref() else {
            return false;
        };
        std::ptr::eq(&*shell.window, &*session.window)
            && std::ptr::eq(&*shell.tab_scroll, &*session.scroll)
            && std::ptr::eq(&*shell.tab_document, &*session.document)
            && std::ptr::eq(&*shell.tab_drop_indicator, &*session.indicator)
            && shell.sidebar_collapsed == session.collapsed
            && shell.tab_buttons.iter().any(|row| {
                row.id == session.target.tab
                    && row.space == session.target.space
                    && std::ptr::eq(&*row.drag, &*session.recognizer)
                    && std::ptr::eq(&*row.button, &*session.button)
            })
            && tab_drag_target_matches(
                &session.target,
                &shell.tabs,
                &shell.active_space,
                shell.active,
                shell.overview,
                shell.overlay_is_open(),
            )
    })
}

fn tab_drag_is_stored(session: &Rc<TabDragSession>) -> bool {
    TAB_DRAG.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, session))
    })
}

fn tab_drag_current(session: &Rc<TabDragSession>, stored: bool) -> bool {
    tab_drag_current_at(
        session,
        stored,
        session.viewport.get(),
        session.clip_bounds.get(),
    )
}

fn tab_drag_current_at(
    session: &Rc<TabDragSession>,
    stored: bool,
    viewport: NSRect,
    clip_bounds: NSRect,
) -> bool {
    if (stored && !tab_drag_is_stored(session)) || !tab_drag_shell_matches(session) {
        return false;
    }
    // Native getters may reenter; neither the shell nor drag state is borrowed.
    let native = session.window.isKeyWindow()
        && session.window.isVisible()
        && !session.window.isMiniaturized()
        && session.button.isEnabled()
        && !session.document.isHiddenOrHasHiddenAncestor()
        && session
            .recognizer
            .view()
            .is_some_and(|view| std::ptr::eq::<NSView>(&*view, &***session.button))
        && session
            .button
            .window()
            .is_some_and(|window| std::ptr::eq(&*window, &*session.window))
        && std::ptr::eq(&*session.scroll.contentView(), &*session.clip)
        && session
            .clip
            .documentView()
            .is_some_and(|view| std::ptr::eq::<NSView>(&*view, &**session.document))
        && tab_drag_geometry_matches(
            &session.geometry,
            &tab_drag_geometry(
                &session.window,
                &session.scroll,
                &session.document,
                &session.button,
                &session.clip,
            ),
            viewport,
            clip_bounds,
        )
        && !page_find_has_marked_text();
    native
        && (!stored || tab_drag_is_stored(session))
        && tab_drag_shell_matches(session)
        && crate::browser::validate_tab_drag_target(&session.target).is_ok()
}

fn cancel_tab_drag() {
    TAB_DRAG_EPOCH.with(|epoch| epoch.set(epoch.get().saturating_add(1)));
    let session = TAB_DRAG.with(|slot| slot.borrow_mut().take());
    if let Some(session) = session {
        stop_tab_drag_timer(&session);
        if TAB_DRAG.with(|slot| slot.borrow().is_none()) {
            session.indicator.setHidden(true);
        }
    }
}

fn validate_active_tab_drag() {
    let session = TAB_DRAG.with(|slot| slot.borrow().clone());
    if TAB_DRAG_QUERYING.with(|busy| busy.replace(true)) {
        if session.is_some_and(|session| !tab_drag_shell_matches(&session)) {
            cancel_tab_drag();
        }
        return;
    }
    if session.is_some_and(|session| !tab_drag_current(&session, true)) {
        cancel_tab_drag();
    }
    TAB_DRAG_QUERYING.with(|busy| busy.set(false));
}

fn begin_tab_drag(sender: &TabDragRecognizer) {
    let Some(epoch) = TAB_DRAG_EPOCH.with(Cell::get).checked_add(1) else {
        return;
    };
    cancel_tab_drag();
    if epoch == u64::MAX || TAB_DRAG_EPOCH.with(Cell::get) != epoch {
        return;
    }
    let candidate = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let row = shell
            .tab_buttons
            .iter()
            .find(|row| std::ptr::eq(sender, &*row.drag))?;
        let target = crate::browser::TabDragTarget {
            tab: row.id,
            space: row.space.clone(),
            selected: shell.active,
            order: shell
                .tabs
                .iter()
                .filter(|tab| tab.space == row.space)
                .map(|tab| tab.id)
                .collect(),
        };
        if !tab_drag_target_matches(
            &target,
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        ) {
            return None;
        }
        Some((
            target,
            row.drag.clone(),
            row.button.clone(),
            shell.window.clone(),
            shell.tab_scroll.clone(),
            shell.tab_document.clone(),
            shell.tab_drop_indicator.clone(),
            shell.sidebar_collapsed,
        ))
    });
    let Some((target, recognizer, button, window, scroll, document, indicator, collapsed)) =
        candidate
    else {
        return;
    };
    let clip = scroll.contentView();
    let geometry = tab_drag_geometry(&window, &scroll, &document, &button, &clip);
    let session = Rc::new(TabDragSession {
        target,
        recognizer,
        button,
        window,
        scroll,
        clip,
        document,
        indicator,
        geometry,
        viewport: Cell::new(geometry.viewport),
        clip_bounds: Cell::new(geometry.clip_bounds),
        timer: RefCell::new(None),
        collapsed,
    });
    if tab_drag_current(&session, false) && TAB_DRAG_EPOCH.with(Cell::get) == epoch {
        TAB_DRAG.with(|slot| *slot.borrow_mut() = Some(session));
    }
}

fn tab_drag_point_inside(point: NSPoint, viewport: NSRect) -> bool {
    [
        point.x,
        point.y,
        viewport.origin.x,
        viewport.origin.y,
        viewport.size.width,
        viewport.size.height,
        viewport.origin.x + viewport.size.width,
        viewport.origin.y + viewport.size.height,
    ]
    .into_iter()
    .all(f64::is_finite)
        && viewport.size.width > 0.0
        && viewport.size.height > 0.0
        && point.x >= viewport.origin.x
        && point.x < viewport.origin.x + viewport.size.width
        && point.y >= viewport.origin.y
        && point.y < viewport.origin.y + viewport.size.height
}

/// Fixed 12-point steps in the inner 24-point edge bands, nominally at 30 Hz.
/// A central pointer, a short document, or a reached edge needs no timer.
fn tab_drag_scroll_origin(point: NSPoint, viewport: NSRect, document_height: f64) -> Option<f64> {
    if !tab_drag_point_inside(point, viewport)
        || !document_height.is_finite()
        || document_height <= viewport.size.height
        || viewport.origin.y < 0.0
    {
        return None;
    }
    let limit = document_height - viewport.size.height;
    if viewport.origin.y > limit {
        return None;
    }
    let band = 24.0f64.min(viewport.size.height / 2.0);
    let next = if point.y < viewport.origin.y + band {
        (viewport.origin.y - 12.0).max(0.0)
    } else if point.y >= viewport.origin.y + viewport.size.height - band {
        (viewport.origin.y + 12.0).min(limit)
    } else {
        return None;
    };
    (next != viewport.origin.y).then_some(next)
}

/// Only the constrained, requested vertical movement may become our new
/// expected viewport. A native getter cannot authorize another scroll or size.
fn tab_drag_scrolled_viewport(
    viewport: NSRect,
    current: NSRect,
    proposed: NSRect,
    constrained: NSRect,
) -> Option<NSRect> {
    let requested = proposed.origin.y - current.origin.y;
    let actual = constrained.origin.y - current.origin.y;
    if ![requested, actual, viewport.origin.y + actual]
        .into_iter()
        .all(f64::is_finite)
        || proposed.origin.x != current.origin.x
        || proposed.size != current.size
        || constrained.origin.x != current.origin.x
        || constrained.size != current.size
        || requested == 0.0
        || (requested > 0.0 && !(0.0..=requested).contains(&actual))
        || (requested < 0.0 && !(requested..=0.0).contains(&actual))
    {
        return None;
    }
    Some(NSRect::new(
        NSPoint::new(viewport.origin.x, viewport.origin.y + actual),
        viewport.size,
    ))
}

fn stop_tab_drag_timer(session: &TabDragSession) {
    let timer = session.timer.borrow_mut().take();
    // The run loop retains the timer, and the timer retains ShellTarget.
    // Invalidate releases both edges. ShellTarget owns neither this session
    // nor the timer, so there is no ownership cycle.
    if let Some(timer) = timer {
        timer.invalidate();
    }
}

fn tab_drag_timer_is_current(session: &Rc<TabDragSession>, timer: &NSTimer) -> bool {
    tab_drag_is_stored(session)
        && session
            .timer
            .borrow()
            .as_ref()
            .is_some_and(|current| std::ptr::eq(timer, &**current))
}

fn cancel_tab_drag_session(session: &Rc<TabDragSession>) {
    if tab_drag_is_stored(session) {
        cancel_tab_drag();
    } else {
        stop_tab_drag_timer(session);
    }
}

fn update_tab_drag_timer(session: &Rc<TabDragSession>) {
    let state = session.recognizer.state();
    let point = session
        .document
        .convertPoint_fromView(session.recognizer.ivars().location.get(), None);
    if !tab_drag_current(session, true) {
        cancel_tab_drag_session(session);
        return;
    }
    if !matches!(
        state,
        NSGestureRecognizerState::Began | NSGestureRecognizerState::Changed
    ) || tab_drag_scroll_origin(
        point,
        session.viewport.get(),
        session.geometry.document.size.height,
    )
    .is_none()
    {
        stop_tab_drag_timer(session);
        return;
    }
    if session.timer.borrow().is_some() {
        return;
    }
    let target = SHELL.with(|slot| {
        slot.try_borrow()
            .ok()?
            .as_ref()
            .map(|shell| shell.target.clone())
    });
    let Some(target) = target else {
        cancel_tab_drag_session(session);
        return;
    };
    // Unscheduled first: do not permit a callback before storing its identity.
    let interval = 1.0 / 30.0;
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            interval,
            &target,
            sel!(scrollDraggedTab:),
            None,
            true,
        )
    };
    timer.setTolerance(interval / 10.0);
    if !tab_drag_current(session, true) || session.timer.borrow().is_some() {
        timer.invalidate();
        return;
    }
    *session.timer.borrow_mut() = Some(timer.clone());
    let run_loop = NSRunLoop::mainRunLoop();
    if !tab_drag_timer_is_current(session, &timer) || !tab_drag_current(session, true) {
        stop_tab_drag_timer(session);
        return;
    }
    unsafe {
        run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes);
    }
    if !tab_drag_timer_is_current(session, &timer) || !tab_drag_current(session, true) {
        stop_tab_drag_timer(session);
        return;
    }
    // The same timer may belong to multiple modes. Register tracking explicitly
    // instead of depending on the application's current common-mode contents.
    unsafe {
        run_loop.addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
    }
    if !tab_drag_timer_is_current(session, &timer) || !tab_drag_current(session, true) {
        stop_tab_drag_timer(session);
    }
}

fn tab_drag_timer_fired(timer: &NSTimer) {
    let session = TAB_DRAG.with(|slot| slot.borrow().clone());
    let Some(session) = session.filter(|session| tab_drag_timer_is_current(session, timer)) else {
        // An obsolete callback must not stop a newer session's timer.
        timer.invalidate();
        return;
    };
    if TAB_DRAG_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    scroll_dragged_tab(&session, timer);
    TAB_DRAG_QUERYING.with(|busy| busy.set(false));
}

fn scroll_dragged_tab(session: &Rc<TabDragSession>, timer: &NSTimer) {
    let state = session.recognizer.state();
    if !tab_drag_timer_is_current(session, timer) {
        return;
    }
    if state == NSGestureRecognizerState::Ended {
        // The final action can be queued behind this tick. Preserve its drop.
        stop_tab_drag_timer(session);
        return;
    }
    if !matches!(
        state,
        NSGestureRecognizerState::Began | NSGestureRecognizerState::Changed
    ) || !tab_drag_current(session, true)
    {
        cancel_tab_drag_session(session);
        return;
    }
    let window_point = session.recognizer.ivars().location.get();
    let point = session.document.convertPoint_fromView(window_point, None);
    let viewport = session.viewport.get();
    if !tab_drag_timer_is_current(session, timer) || !tab_drag_current(session, true) {
        cancel_tab_drag_session(session);
        return;
    }
    if !tab_drag_point_inside(point, viewport) {
        cancel_tab_drag_session(session);
        return;
    }
    let Some(next_y) =
        tab_drag_scroll_origin(point, viewport, session.geometry.document.size.height)
    else {
        stop_tab_drag_timer(session);
        return;
    };
    let bounds = session.clip_bounds.get();
    let proposed = NSRect::new(
        NSPoint::new(
            bounds.origin.x,
            bounds.origin.y + next_y - viewport.origin.y,
        ),
        bounds.size,
    );
    let constrained = session.clip.constrainBoundsRect(proposed);
    let Some(next_viewport) = tab_drag_scrolled_viewport(viewport, bounds, proposed, constrained)
    else {
        cancel_tab_drag_session(session);
        return;
    };
    let state = session.recognizer.state();
    if !tab_drag_timer_is_current(session, timer) || !tab_drag_current(session, true) {
        cancel_tab_drag_session(session);
        return;
    }
    if state == NSGestureRecognizerState::Ended {
        stop_tab_drag_timer(session);
        return;
    }
    if !matches!(
        state,
        NSGestureRecognizerState::Began | NSGestureRecognizerState::Changed
    ) {
        cancel_tab_drag_session(session);
        return;
    }
    if session.recognizer.ivars().location.get() != window_point {
        return;
    }
    if next_viewport == viewport {
        stop_tab_drag_timer(session);
        return;
    }
    session.clip.scrollToPoint(constrained.origin);
    if !tab_drag_timer_is_current(session, timer)
        || !tab_drag_current_at(session, true, next_viewport, constrained)
    {
        cancel_tab_drag_session(session);
        return;
    }
    // Only this verified native scroll advances expected geometry. Reentrant
    // cancellation cannot restore an old session or accept an arbitrary offset.
    session.viewport.set(next_viewport);
    session.clip_bounds.set(constrained);
    session.scroll.reflectScrolledClipView(&session.clip);
    if !tab_drag_timer_is_current(session, timer) || !tab_drag_current(session, true) {
        cancel_tab_drag_session(session);
        return;
    }
    update_tab_drag(&session.recognizer, false);
}

/// Return an insertion anchor and its line using actual native row frames.
/// Binary search reads O(log n) frames without collecting or cloning tab lists.
fn tab_drag_drop(
    order: &[i32],
    source: i32,
    point: NSPoint,
    viewport: NSRect,
    mut frame: impl FnMut(i32) -> Option<NSRect>,
) -> Option<(Option<i32>, f64)> {
    let source_index = order.iter().position(|id| *id == source)?;
    if order.len() < 2 || !tab_drag_point_inside(point, viewport) {
        return None;
    }
    let mut row_frame = |id| {
        let value = frame(id)?;
        (value.origin.y.is_finite() && value.size.height.is_finite() && value.size.height > 0.0)
            .then_some(value)
    };
    let (mut low, mut high) = (0, order.len() - 1);
    while low < high {
        let middle = low + (high - low) / 2;
        let id = order[middle + usize::from(middle >= source_index)];
        let bounds = row_frame(id)?;
        if point.y < bounds.origin.y + bounds.size.height / 2.0 {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    if low < order.len() - 1 {
        let id = order[low + usize::from(low >= source_index)];
        Some((Some(id), row_frame(id)?.origin.y - 1.0))
    } else {
        let bounds = row_frame(*order.last()?)?;
        Some((None, bounds.origin.y + bounds.size.height + 1.0))
    }
}

fn tab_drag_action(sender: &TabDragRecognizer) {
    if TAB_DRAG_QUERYING.with(|busy| busy.replace(true)) {
        cancel_tab_drag();
        return;
    }
    let epoch = TAB_DRAG_EPOCH.with(Cell::get);
    let state = sender.state();
    if TAB_DRAG_EPOCH.with(Cell::get) != epoch {
        TAB_DRAG_QUERYING.with(|busy| busy.set(false));
        return;
    }
    if state == NSGestureRecognizerState::Began {
        if sender.ivars().epoch.get() != epoch {
            TAB_DRAG_QUERYING.with(|busy| busy.set(false));
            return;
        }
        begin_tab_drag(sender);
    }
    if matches!(
        state,
        NSGestureRecognizerState::Began
            | NSGestureRecognizerState::Changed
            | NSGestureRecognizerState::Ended
    ) {
        update_tab_drag(sender, state == NSGestureRecognizerState::Ended);
    } else if state == NSGestureRecognizerState::Cancelled {
        cancel_tab_drag();
    }
    // Failed sends no action. No drag state is retained until Began.
    TAB_DRAG_QUERYING.with(|busy| busy.set(false));
}

fn update_tab_drag(sender: &TabDragRecognizer, ended: bool) {
    let session = TAB_DRAG.with(|slot| slot.borrow().clone());
    let Some(session) = session.filter(|session| std::ptr::eq(sender, &*session.recognizer)) else {
        return;
    };
    if ended {
        stop_tab_drag_timer(&session);
    }
    if !tab_drag_current(&session, true) {
        cancel_tab_drag();
        return;
    }
    let point = sender.locationInView(Some(&session.document));
    let drop = tab_drag_drop(
        &session.target.order,
        session.target.tab,
        point,
        session.viewport.get(),
        |id| {
            let button = SHELL.with(|slot| {
                let slot = slot.try_borrow().ok()?;
                let shell = slot.as_ref()?;
                shell
                    .tab_buttons
                    .iter()
                    .find(|row| row.id == id && row.space == session.target.space)
                    .map(|row| row.button.clone())
            })?;
            Some(button.frame())
        },
    );
    let Some((before, y)) = drop.filter(|_| tab_drag_current(&session, true)) else {
        cancel_tab_drag();
        return;
    };
    if ended {
        // Consume before the pure backend change; no native call intervenes.
        TAB_DRAG.with(|slot| {
            slot.borrow_mut().take();
        });
        let result = crate::browser::reorder_tab_by_drag(&session.target, before);
        session.indicator.setHidden(true);
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.notice = result.as_ref().err().cloned();
            }
        });
        // Keep the existing selection and scroll position, including when the
        // dragged row is a background tab. No editor or handoff action is used.
        if result != Ok(false) {
            crate::browser::notify_panel();
        }
    } else {
        session.indicator.setFrame(rect(
            2.0,
            y,
            (session.geometry.document.size.width - 4.0).max(0.0),
            2.0,
        ));
        if !tab_drag_is_stored(&session) {
            session.indicator.setHidden(true);
            return;
        }
        session.indicator.setHidden(false);
        if !tab_drag_current(&session, true) {
            cancel_tab_drag_session(&session);
        } else {
            update_tab_drag_timer(&session);
        }
    }
}

fn navigation_control_target(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    overview: bool,
    overlay: bool,
) -> Option<crate::browser::NavigationControlTarget> {
    if overview || overlay {
        return None;
    }
    let tab = tabs
        .iter()
        .find(|tab| Some(tab.id) == active && tab.space == space)?;
    Some(crate::browser::NavigationControlTarget {
        tab: tab.id,
        space: tab.space.clone(),
        navigation_epoch: tab.navigation_epoch,
        stop: tab.loading,
    })
}

fn navigation_control_page_matches(
    target: &crate::browser::NavigationControlTarget,
    url: &str,
    tab: Option<&ShellTab>,
    space: &str,
    active: Option<i32>,
    overview: bool,
) -> bool {
    !overview
        && active == Some(target.tab)
        && space == target.space
        && tab
            .is_some_and(|tab| tab.id == target.tab && tab.space == target.space && tab.url == url)
}

fn rebase_navigation_address(
    draft: &mut AddressDraft,
    target: &crate::browser::NavigationControlTarget,
    url: &str,
    tab: &ShellTab,
    outcome: &crate::browser::NavigationControlOutcome,
    receipt_current: bool,
) -> bool {
    if !receipt_current
        || !outcome.preserve_address
        || draft.tab != target.tab
        || draft.url != url
        || draft.navigation_epoch != target.navigation_epoch
        || tab.id != target.tab
        || tab.space != target.space
        || tab.url != url
        || tab.navigation_epoch < target.navigation_epoch
        || tab.navigation_epoch > outcome.navigation_epoch
    {
        return false;
    }
    // A snapshot can be skipped after a loading-only callback. The live receipt
    // proves the document/owner even when the displayed epoch is still older.
    draft.navigation_epoch = outcome.navigation_epoch;
    true
}

fn navigation_address_outcome(
    result: &Result<
        crate::browser::NavigationControlOutcome,
        crate::browser::NavigationControlError,
    >,
) -> Option<&crate::browser::NavigationControlOutcome> {
    match result {
        Ok(outcome) => Some(outcome),
        Err(error) => error.preserved_address.as_ref(),
    }
}

fn navigation_control_call_has_page(shell: &Shell, call: &NavigationControlCall) -> bool {
    std::ptr::eq(&*shell.window, &*call.window)
        && std::ptr::eq(&*shell.address, &*call.address)
        && std::ptr::eq(&*shell.reload, &*call.reload)
        && navigation_control_page_matches(
            &call.target,
            &call.url,
            shell.tabs.iter().find(|tab| Some(tab.id) == shell.active),
            &shell.active_space,
            shell.active,
            shell.overview,
        )
}

fn navigation_control_address_is_current(call: &Rc<NavigationControlCall>) -> bool {
    // Losing focus or opening an overlay blocks the command, but does not by
    // itself replace this address. Keep the exact call and control identities.
    NAVIGATION_CONTROL_CALL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, call))
    }) && SHELL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref()
                .is_some_and(|shell| navigation_control_call_has_page(shell, call))
        })
    })
}

fn navigation_control_native_ready(call: &NavigationControlCall) -> bool {
    // Getters can dispatch nested callbacks. The runtime samples CEF loading
    // after this preflight and uses only the pure predicate after that sample.
    call.window.isKeyWindow()
        && call.window.isVisible()
        && !call.window.isMiniaturized()
        && call.reload.window().as_deref() == Some(&*call.window)
        && !call.reload.isHiddenOrHasHiddenAncestor()
        && call.reload.isEnabled()
        && call.address.window().as_deref() == Some(&*call.window)
        && !page_find_has_marked_text()
}

fn navigation_control_call_is_current(call: &NavigationControlCall, exact: bool) -> bool {
    // No native/CEF getters after the runtime's final loading observation.
    SHELL.with(|slot| {
        let Ok(slot) = slot.try_borrow() else {
            return false;
        };
        let Some(shell) = slot.as_ref() else {
            return false;
        };
        let tab = shell.tabs.iter().find(|tab| Some(tab.id) == shell.active);
        navigation_control_call_has_page(shell, call)
            && !shell.overlay_is_open()
            && (!exact
                || tab.is_some_and(|tab| {
                    tab.navigation_epoch == call.target.navigation_epoch
                        && tab.loading == call.target.stop
                }))
    })
}

fn capture_navigation_control() -> Option<NavigationControlCall> {
    if NAVIGATION_CONTROL_CALL.with(|slot| slot.borrow().is_some()) {
        return None;
    }
    let call = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let target = navigation_control_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )?;
        let tab = shell.tabs.iter().find(|tab| tab.id == target.tab)?;
        let preserve_address = shell.address_draft.as_ref().is_none_or(|draft| {
            draft.tab == target.tab
                && draft.url == tab.url
                && draft.navigation_epoch == target.navigation_epoch
        });
        Some(NavigationControlCall {
            target,
            url: tab.url.clone(),
            window: shell.window.clone(),
            address: shell.address.clone(),
            reload: shell.reload.clone(),
            preserve_address,
        })
    })?;
    (navigation_control_native_ready(&call) && navigation_control_call_is_current(&call, true))
        .then_some(call)
}

fn prepare_navigation_control_menu(item: Retained<NSMenuItem>) {
    if NAVIGATION_CONTROL_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    let previous = NAVIGATION_CONTROL_MENU.with(|slot| slot.borrow_mut().take());
    drop(previous);
    let value = capture_navigation_control().map(|call| NavigationControlMenu {
        item: item.clone(),
        target: call.target,
    });
    item.setTitle(&NSString::from_str(
        if value.as_ref().is_some_and(|value| value.target.stop) {
            "로딩 중지"
        } else {
            "새로 고침"
        },
    ));
    NAVIGATION_CONTROL_MENU.with(|slot| *slot.borrow_mut() = value);
    NAVIGATION_CONTROL_QUERYING.with(|busy| busy.set(false));
}

fn validate_navigation_control_menu(item: &NSMenuItem) -> bool {
    if NAVIGATION_CONTROL_QUERYING.with(|busy| busy.replace(true)) {
        return false;
    }
    let target = NAVIGATION_CONTROL_MENU.with(|slot| {
        slot.try_borrow()
            .ok()?
            .as_ref()
            .filter(|value| std::ptr::eq(item, &*value.item))
            .map(|value| value.target.clone())
    });
    let valid = target.is_some_and(|target| {
        capture_navigation_control().is_some_and(|call| call.target == target)
    });
    NAVIGATION_CONTROL_QUERYING.with(|busy| busy.set(false));
    valid
}

fn navigation_control_action(sender: Option<&AnyObject>) {
    let Some(call) = capture_navigation_control() else {
        return;
    };
    if let Some(sender) = sender {
        if let Some(button) = sender.downcast_ref::<NSButton>() {
            if !std::ptr::eq(button, &*call.reload) {
                return;
            }
        } else if let Some(item) = sender.downcast_ref::<NSMenuItem>() {
            let target = NAVIGATION_CONTROL_MENU.with(|slot| {
                slot.try_borrow()
                    .ok()?
                    .as_ref()
                    .filter(|value| std::ptr::eq(item, &*value.item))
                    .map(|value| value.target.clone())
            });
            // An open menu never replaces its displayed intent with a new one.
            if target.as_ref() != Some(&call.target) || !item.isEnabled() {
                return;
            }
        } else {
            return;
        }
    }
    if !navigation_control_native_ready(&call) || !navigation_control_call_is_current(&call, true) {
        return;
    }
    let call = Rc::new(call);
    let installed = NAVIGATION_CONTROL_CALL.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return false;
        }
        *slot = Some(call.clone());
        true
    });
    if !installed {
        return;
    }
    let cleanup = NavigationControlCleanup(call.clone());
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            // A button click can end editing before validation. Preserve that
            // field's context on an early failure, without copying its text.
            if shell.address_draft.is_none() {
                shell.address_draft = Some(AddressDraft {
                    tab: call.target.tab,
                    url: call.url.clone(),
                    navigation_epoch: call.target.navigation_epoch,
                });
            }
        }
    });
    let result = crate::browser::control_navigation(
        &call.target,
        || navigation_control_native_ready(&call),
        || {
            navigation_control_call_is_current(&call, false)
                && NAVIGATION_CONTROL_CALL.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_some_and(|current| Rc::ptr_eq(current, &call))
                })
        },
    );
    let address_receipt = navigation_address_outcome(&result).and_then(|outcome| {
        crate::browser::capture_navigation_address_receipt(&call.target, &call.url, outcome)
    });
    // Keep the local hold across native snapshot queries. A rejected snapshot
    // must not discard a draft when the same document still matches below.
    sync();
    let receipt_current = address_receipt
        .as_ref()
        .is_some_and(crate::browser::navigation_address_receipt_current);
    let command_current = navigation_control_call_is_current(&call, false);
    if navigation_control_address_is_current(&call) {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                let tab = shell.tabs.iter().find(|tab| Some(tab.id) == shell.active);
                if call.preserve_address {
                    if let (Some(outcome), Some(draft), Some(tab)) = (
                        navigation_address_outcome(&result),
                        &mut shell.address_draft,
                        tab,
                    ) {
                        rebase_navigation_address(
                            draft,
                            &call.target,
                            &call.url,
                            tab,
                            outcome,
                            receipt_current,
                        );
                    }
                }
                match (&result, tab) {
                    (Ok(outcome), Some(tab)) => {
                        if command_current
                            && tab.navigation_epoch == outcome.navigation_epoch
                            && outcome.navigation_epoch != call.target.navigation_epoch
                        {
                            shell.notice = None;
                        }
                    }
                    (Err(error), Some(_)) if command_current => {
                        // The same call, controls, tab, space and URL still match.
                        // Reporting rejection needs no address-rebase receipt:
                        // an early stale-epoch failure has no such receipt.
                        shell.notice = Some(format!("처리하지 못했습니다: {}", error.message));
                    }
                    _ => {}
                }
            }
        });
    }
    // Drop this exact call before rendering. No hold survives a return/error,
    // and a reentrant newer call can never be cleared by this cleanup.
    drop(cleanup);
    refresh();
}

fn run_action(action: Option<ShellAction>) {
    let Some(action) = action else {
        return;
    };
    // Explicit native chrome/menu commands dismiss the search surface first.
    // Search result activation has its own validated path below.
    if search_is_open() {
        close_tab_search(false);
        close_bookmarks(false);
        close_downloads(false);
        close_site_permissions(false);
    }
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.address_draft = None;
        }
    });
    // No shell RefCell borrow may cross a browser action or its CEF callbacks.
    let result = crate::browser::shell_action(action);
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.notice = result
                .err()
                .map(|error| format!("처리하지 못했습니다: {error}"));
        }
    });
    refresh();
}

fn tab_order_menu_direction(item: &NSMenuItem) -> Option<bool> {
    if item.action() == Some(sel!(moveTabUp:)) {
        Some(false)
    } else if item.action() == Some(sel!(moveTabDown:)) {
        Some(true)
    } else {
        None
    }
}

fn tab_order_target(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    overview: bool,
    overlay: bool,
    forward: bool,
) -> Option<crate::browser::TabOrderTarget> {
    if overview || overlay {
        return None;
    }
    let tab = active?;
    let visible = tabs.iter().filter(|item| item.space == space);
    let index = visible.clone().position(|item| item.id == tab)?;
    let neighbor_index = if forward {
        index.checked_add(1)?
    } else {
        index.checked_sub(1)?
    };
    Some(crate::browser::TabOrderTarget {
        tab,
        space: space.to_owned(),
        neighbor: visible.clone().nth(neighbor_index)?.id,
        forward,
    })
}

fn tab_order_target_ui(forward: bool) -> Option<crate::browser::TabOrderTarget> {
    let window = SHELL.with(|slot| {
        slot.try_borrow()
            .ok()?
            .as_ref()
            .map(|shell| shell.window.clone())
    })?;
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || page_find_has_marked_text()
    {
        return None;
    }
    // Resolve only after native getters, which may dispatch nested callbacks.
    let target = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if !std::ptr::eq(&*shell.window, &*window) {
            return None;
        }
        tab_order_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
            forward,
        )
    })?;
    crate::browser::validate_tab_order_target(&target).ok()?;
    Some(target)
}

fn prepare_tab_order_menu(item: Retained<NSMenuItem>, forward: bool) {
    if TAB_ORDER_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    let previous = TAB_ORDER_MENU.with(|slot| slot.borrow_mut()[usize::from(forward)].take());
    drop(previous);
    item.setToolTip(Some(&NSString::from_str(
        "같은 공간에서 순서만 바꿉니다. 선택한 페이지와 AI 제어권은 유지하며, 재시작 후 순서는 복원하지 않습니다.",
    )));
    let value = tab_order_target_ui(forward).map(|target| TabOrderMenu { item, target });
    TAB_ORDER_MENU.with(|slot| slot.borrow_mut()[usize::from(forward)] = value);
    TAB_ORDER_QUERYING.with(|busy| busy.set(false));
}

fn validate_tab_order_menu(item: &NSMenuItem, forward: bool) -> bool {
    if TAB_ORDER_QUERYING.with(|busy| busy.replace(true)) {
        return false;
    }
    let value = TAB_ORDER_MENU.with(|slot| {
        slot.try_borrow().ok()?[usize::from(forward)]
            .as_ref()
            .filter(|value| std::ptr::eq(item, &*value.item))
            .cloned()
    });
    let valid =
        value.is_some_and(|value| tab_order_target_ui(forward).as_ref() == Some(&value.target));
    TAB_ORDER_QUERYING.with(|busy| busy.set(false));
    valid
}

fn reorder_tab(sender: &NSMenuItem, forward: bool) {
    if TAB_ORDER_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    let value = TAB_ORDER_MENU.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        let value = &mut slot[usize::from(forward)];
        if !value
            .as_ref()
            .is_some_and(|value| std::ptr::eq(sender, &*value.item))
        {
            return None;
        }
        value.take()
    });
    if let Some(value) =
        value.filter(|value| tab_order_target_ui(forward).as_ref() == Some(&value.target))
    {
        let result = crate::browser::reorder_tab_from_sidebar(&value.target);
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                if result.is_ok() {
                    shell.reveal_active_tab = true;
                }
                shell.notice = result.err();
            }
        });
        // Keep responder and drafts intact; the existing coalesced refresh
        // reuses row controls and updates their position/accessibility labels.
        crate::browser::notify_panel();
    }
    TAB_ORDER_QUERYING.with(|busy| busy.set(false));
}

fn tab_audio_page(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    overview: bool,
    overlay: bool,
) -> Option<(i32, String, u64)> {
    if overview || overlay {
        return None;
    }
    let tab = tabs
        .iter()
        .find(|tab| Some(tab.id) == active && tab.space == space && !tab.crashed)?;
    Some((tab.id, tab.space.clone(), tab.navigation_epoch))
}

fn tab_audio_page_ui() -> Option<(i32, String, u64)> {
    let (page, window, view) = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let page = tab_audio_page(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )?;
        Some((
            page.clone(),
            shell.window.clone(),
            shell.views.get(&page.0)?.clone(),
        ))
    })?;
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || view.window().as_deref() != Some(&*window)
        || view.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return None;
    }
    // AppKit getters may reenter; no old selection survives this second lookup.
    SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if !std::ptr::eq(&*shell.window, &*window)
            || !shell
                .views
                .get(&page.0)
                .is_some_and(|live| std::ptr::eq(&**live, &*view))
        {
            return None;
        }
        (tab_audio_page(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )
        .as_ref()
            == Some(&page))
        .then_some(page)
    })
}

fn tab_audio_target_is_current(target: &crate::browser::TabAudioTarget) -> bool {
    tab_audio_page_ui().is_some_and(|(tab, space, epoch)| {
        tab == target.tab
            && space == target.space
            && epoch == target.navigation_epoch
            && crate::browser::capture_tab_audio_target(tab, &space, epoch).as_ref() == Ok(target)
    })
}

fn tab_audio_row_page(shell: &Shell, sender: &NSButton, id: i32) -> Option<(i32, String, u64)> {
    if shell.sidebar_collapsed || shell.overview || shell.overlay_is_open() {
        return None;
    }
    let row = shell
        .tab_buttons
        .iter()
        .find(|row| row.id == id && std::ptr::eq(&*row.mute, sender))?;
    if row.space != shell.active_space {
        return None;
    }
    let tab = shell
        .tabs
        .iter()
        .find(|tab| tab.id == row.id && tab.space == row.space && !tab.crashed)?;
    Some((tab.id, tab.space.clone(), tab.navigation_epoch))
}

fn tab_audio_row_ui(sender: &NSButton) -> Option<(i32, String, u64)> {
    let tag: isize = unsafe { msg_send![sender, tag] };
    let id = i32::try_from(tag).ok()?;
    let (page, selected, window, document) = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        Some((
            tab_audio_row_page(shell, sender, id)?,
            shell.active,
            shell.window.clone(),
            shell.tab_document.clone(),
        ))
    })?;
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || !sender.isEnabled()
        || sender.isHiddenOrHasHiddenAncestor()
        || sender.window().as_deref() != Some(&*window)
        || document.window().as_deref() != Some(&*window)
        || !sender.isDescendantOf(&document)
        || page_find_has_marked_text()
    {
        return None;
    }
    // The page itself may be hidden: only its current visible sidebar control
    // grants this action. Recheck identity after the AppKit getters can reenter.
    SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        (std::ptr::eq(&*shell.window, &*window)
            && std::ptr::eq(&*shell.tab_document, &*document)
            && shell.active == selected
            && tab_audio_row_page(shell, sender, id).as_ref() == Some(&page))
        .then_some(page)
    })
}

fn tab_audio_row_target_is_current(
    sender: &NSButton,
    target: &crate::browser::TabAudioTarget,
) -> bool {
    tab_audio_row_ui(sender).is_some_and(|(tab, space, epoch)| {
        tab == target.tab
            && space == target.space
            && epoch == target.navigation_epoch
            && crate::browser::capture_tab_audio_row_target(tab, &space, epoch).as_ref()
                == Ok(target)
    })
}

fn toggle_tab_row_audio(sender: &NSButton) {
    if TAB_AUDIO_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    let result = (|| {
        let sender = SHELL.with(|slot| {
            let slot = slot.try_borrow().ok()?;
            slot.as_ref()?
                .tab_buttons
                .iter()
                .find(|row| std::ptr::eq(&*row.mute, sender))
                .map(|row| row.mute.clone())
        })?;
        let (tab, space, epoch) = tab_audio_row_ui(&sender)?;
        let result = (|| {
            let target = crate::browser::capture_tab_audio_row_target(tab, &space, epoch)?;
            // A mouse action can finish address editing before it arrives.
            // Preserve that draft before the getter's coalesced cache update.
            preserve_address_for_overlay();
            let expected = crate::browser::read_tab_audio(&target)?;
            crate::browser::set_tab_audio_muted(&target, expected, !expected, || {
                tab_audio_row_target_is_current(&sender, &target)
            })
        })();
        Some(result)
    })();
    if let Some(Err(error)) = result {
        SHELL.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                if let Some(shell) = slot.as_mut() {
                    shell.notice = Some(error);
                }
            }
        });
        crate::browser::notify_panel();
    }
    // No tab selection, page focus, navigation or AI handoff is performed.
    TAB_AUDIO_QUERYING.with(|busy| busy.set(false));
}

fn restore_hidden_tab_mute_focus() {
    let window = SHELL.with(|slot| slot.borrow().as_ref().map(|shell| shell.window.clone()));
    let Some(window) = window else {
        return;
    };
    let Some(responder) = window.firstResponder() else {
        return;
    };
    let Some(button) = responder.downcast_ref::<NSButton>() else {
        return;
    };
    let fallback = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        (std::ptr::eq(&*shell.window, &*window)
            && shell.sidebar_collapsed
            && !shell.overview
            && !shell.overlay_is_open()
            && shell
                .tab_buttons
                .iter()
                .any(|row| std::ptr::eq(&*row.mute, button)))
        .then(|| shell.sidebar_toggle.clone())
    });
    if let Some(fallback) = fallback {
        if window.isKeyWindow()
            && window.isVisible()
            && !window.isMiniaturized()
            && fallback.window().as_deref() == Some(&*window)
            && window.firstResponder().as_deref() == Some(&*responder)
            && button.isHiddenOrHasHiddenAncestor()
            && !fallback.isHiddenOrHasHiddenAncestor()
            && fallback.isEnabled()
        {
            window.makeFirstResponder(Some(&fallback));
        }
    }
}

fn prepare_tab_audio_menu(item: Retained<NSMenuItem>) {
    if TAB_AUDIO_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    // A menu open owns one target and expected setting. Later validation must
    // never retarget an already open menu to a newly selected page.
    let previous = TAB_AUDIO_MENU.with(|slot| slot.borrow_mut().take());
    drop(previous);
    item.setToolTip(Some(&NSString::from_str(
        "체크 표시는 이 탭의 음소거 설정을 뜻합니다. AI 제어권과 탭 선택을 유지합니다.",
    )));
    let value = (|| {
        let (tab, space, epoch) = tab_audio_page_ui()?;
        let target = crate::browser::capture_tab_audio_target(tab, &space, epoch).ok()?;
        let expected = crate::browser::read_tab_audio(&target).ok()?;
        tab_audio_target_is_current(&target).then_some(TabAudioMenu {
            item,
            target,
            expected,
        })
    })();
    TAB_AUDIO_MENU.with(|slot| *slot.borrow_mut() = value);
    TAB_AUDIO_QUERYING.with(|busy| busy.set(false));
}

fn validate_tab_audio_menu(item: &NSMenuItem) -> bool {
    if TAB_AUDIO_QUERYING.with(|busy| busy.replace(true)) {
        return false;
    }
    let value = TAB_AUDIO_MENU.with(|slot| {
        slot.try_borrow()
            .ok()?
            .as_ref()
            .filter(|value| std::ptr::eq(item, &*value.item))
            .cloned()
    });
    let value = value.filter(|value| tab_audio_target_is_current(&value.target));
    // Unknown and stale states are disabled, never presented as playing audio.
    item.setState(if value.as_ref().is_some_and(|value| value.expected) {
        1
    } else {
        0
    });
    TAB_AUDIO_QUERYING.with(|busy| busy.set(false));
    value.is_some()
}

fn toggle_tab_audio(sender: &NSMenuItem) {
    if TAB_AUDIO_QUERYING.with(|busy| busy.replace(true)) {
        return;
    }
    let value = TAB_AUDIO_MENU.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        if !slot
            .as_ref()
            .is_some_and(|value| std::ptr::eq(sender, &*value.item))
        {
            return None;
        }
        slot.take()
    });
    if let Some(value) = value {
        let result = crate::browser::set_tab_audio_muted(
            &value.target,
            value.expected,
            !value.expected,
            || tab_audio_target_is_current(&value.target),
        );
        if let Err(error) = result {
            SHELL.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    if let Some(shell) = slot.as_mut() {
                        shell.notice = Some(error);
                    }
                }
            });
            crate::browser::notify_panel();
        }
    }
    // No selection, responder, editor draft or AI ownership is changed here.
    TAB_AUDIO_QUERYING.with(|busy| busy.set(false));
}

fn current_page_target(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    overview: bool,
    overlay: bool,
) -> Option<crate::browser::CurrentPageTarget> {
    if overview || overlay {
        return None;
    }
    let tab = tabs
        .iter()
        .find(|tab| Some(tab.id) == active && tab.space == space)?;
    if tab.crashed || crate::address::normalize_url(&tab.url).is_err() {
        return None;
    }
    Some(crate::browser::CurrentPageTarget {
        tab: tab.id,
        space: tab.space.clone(),
        navigation_epoch: tab.navigation_epoch,
        url: tab.url.clone(),
    })
}

fn current_page_target_ui() -> Option<crate::browser::CurrentPageTarget> {
    let (target, window, view) = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        let target = current_page_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )?;
        let view = shell.views.get(&target.tab)?.clone();
        Some((target, shell.window.clone(), view))
    })?;
    // Native getters may reenter; retain the views and release SHELL first.
    if !window.isKeyWindow()
        || !window.isVisible()
        || window.isMiniaturized()
        || view.window().as_deref() != Some(&*window)
        || view.isHiddenOrHasHiddenAncestor()
        || page_find_has_marked_text()
    {
        return None;
    }
    let current = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if !std::ptr::eq(&*shell.window, &*window)
            || !shell
                .views
                .get(&target.tab)
                .is_some_and(|current| std::ptr::eq(&**current, &*view))
        {
            return None;
        }
        current_page_target(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
        )
    });
    if current.as_ref() != Some(&target) {
        return None;
    }
    crate::browser::validate_current_page_target(&target).ok()?;
    Some(target)
}

pub(crate) fn current_page_target_is_current(target: &crate::browser::CurrentPageTarget) -> bool {
    current_page_target_ui().as_ref() == Some(target)
}

fn open_current_page_in_user_tab() {
    let Some(target) = current_page_target_ui() else {
        return;
    };
    // This only reads the source page. Unlike NewTab, it does not hand off AI
    // control or clear an editor before the new user tab has been selected.
    let result = crate::browser::open_current_page_in_user_tab(&target);
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.notice = result.err();
        }
    });
    refresh();
}

fn can_reopen_closed_tab_ui() -> bool {
    SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| !shell.overlay_is_open())
    }) && !page_find_has_marked_text()
        && crate::browser::can_reopen_closed_page()
}

fn reopen_closed_tab() {
    if !can_reopen_closed_tab_ui() {
        return;
    }
    // Unlike generic navigation, a failed reopen must preserve the address
    // draft and find editor. Successful selection uses the normal sync path.
    let result = crate::browser::reopen_closed_page();
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            if result.is_ok() {
                shell.address_draft = None;
            }
            shell.notice = result.err();
        }
    });
    refresh();
}

fn is_reopen_shortcut(key: &str, modifiers: NSEventModifierFlags) -> bool {
    key == "t"
        && modifiers.contains(NSEventModifierFlags::Command | NSEventModifierFlags::Shift)
        && !modifiers.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option)
}

fn numbered_tab_shortcut(key: &str, modifiers: NSEventModifierFlags) -> Option<usize> {
    if !modifiers.contains(NSEventModifierFlags::Command)
        || modifiers.intersects(
            NSEventModifierFlags::Shift
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        )
    {
        return None;
    }
    match key.as_bytes() {
        [digit @ b'1'..=b'9'] => Some(usize::from(*digit - b'0')),
        _ => None,
    }
}

fn numbered_tab_target(
    tabs: &[ShellTab],
    space: &str,
    number: usize,
    active: Option<i32>,
    overview: bool,
    overlay: bool,
) -> Option<i32> {
    // Match the sidebar's current-space order without collecting another list.
    let mut visible = tabs.iter().filter(|tab| tab.space == space);
    let tab = match number {
        1..=8 => visible.nth(number - 1),
        9 => visible.next_back(),
        _ => None,
    }?;
    // Re-selecting the visible page must not end editing or clear its draft.
    (Some(tab.id) != active || overview || overlay).then_some(tab.id)
}

fn cycle_tab_shortcut(key: &str, modifiers: NSEventModifierFlags) -> Option<bool> {
    if modifiers.contains(NSEventModifierFlags::Control)
        && !modifiers.intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Option)
        && matches!(key, "\t" | "\u{19}")
    {
        return Some(!modifiers.contains(NSEventModifierFlags::Shift));
    }
    if modifiers.contains(NSEventModifierFlags::Command)
        && !modifiers.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option)
    {
        return match key {
            "{" => Some(false),
            "}" => Some(true),
            "[" if modifiers.contains(NSEventModifierFlags::Shift) => Some(false),
            "]" if modifiers.contains(NSEventModifierFlags::Shift) => Some(true),
            _ => None,
        };
    }
    None
}

fn cycle_tab_target(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    forward: bool,
) -> Option<i32> {
    let visible = tabs.iter().filter(|tab| tab.space == space);
    let index = visible.clone().position(|tab| Some(tab.id) == active)?;
    let target = if forward {
        visible
            .clone()
            .nth(index + 1)
            .or_else(|| visible.clone().next())
    } else {
        index
            .checked_sub(1)
            .and_then(|previous| visible.clone().nth(previous))
            .or_else(|| visible.clone().next_back())
    }?;
    // A one-tab space has no neighbour, even when an overlay is open.
    (Some(target.id) != active).then_some(target.id)
}

fn tab_selection_target(resolve: impl FnOnce(&Shell) -> Option<i32>) -> Option<(i32, String)> {
    let window = SHELL.with(|slot| {
        slot.try_borrow()
            .ok()?
            .as_ref()
            .map(|shell| shell.window.clone())
    })?;
    if !window.isKeyWindow() || page_find_has_marked_text() {
        return None;
    }
    // Native getters can reenter. Resolve the current selection afterwards,
    // with no AppKit or CEF call crossing this borrow.
    SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if !std::ptr::eq(&*shell.window, &*window) {
            return None;
        }
        resolve(shell).map(|tab| (tab, shell.active_space.clone()))
    })
}

fn select_tab_command(resolve: impl FnOnce(&Shell) -> Option<i32>) {
    let Some((tab, space)) = tab_selection_target(resolve) else {
        return;
    };
    if crate::browser::select_tab_from_sidebar(tab, &space).is_err()
        || !SHELL.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|shell| shell.active == Some(tab) && shell.active_space == space)
        })
    {
        return;
    }
    // SelectTab syncs the new page first. A stale target must not discard edits.
    close_tab_search(false);
    close_bookmarks(false);
    close_downloads(false);
    close_site_permissions(false);
    let selected = SHELL.with(|slot| {
        slot.borrow().as_ref().is_some_and(|shell| {
            shell.active == Some(tab)
                && shell.active_space == space
                && !shell.overview
                && !shell.overlay_is_open()
        })
    });
    if selected {
        crate::browser::focus_tab(tab);
    }
}

fn resolve_shortcut_key(
    modifiers: NSEventModifierFlags,
    translated: impl FnOnce(NSEventModifierFlags) -> Option<String>,
    unmodified: impl FnOnce() -> Option<String>,
) -> Option<String> {
    // Command shortcuts can retain non-Latin text in charactersIgnoringModifiers.
    // Keep ordinary input and Control/Option combinations on their original path.
    let key = if modifiers.contains(NSEventModifierFlags::Command)
        && !modifiers.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option)
    {
        translated(NSEventModifierFlags::Command | (modifiers & NSEventModifierFlags::Shift))
    } else {
        None
    };
    key.or_else(unmodified).map(|key| key.to_lowercase())
}

fn shortcut_key(event: &NSEvent) -> Option<String> {
    resolve_shortcut_key(
        event.modifierFlags(),
        |modifiers| {
            event
                .charactersByApplyingModifiers(modifiers)
                .map(|text| text.to_string())
        },
        || {
            event
                .charactersIgnoringModifiers()
                .map(|text| text.to_string())
        },
    )
}

pub fn shutdown() {
    cancel_tab_drag();
    let navigation_menu = NAVIGATION_CONTROL_MENU.with(|slot| slot.borrow_mut().take());
    drop(navigation_menu);
    let navigation_call = NAVIGATION_CONTROL_CALL.with(|slot| slot.borrow_mut().take());
    drop(navigation_call);
    let audio_menu = TAB_AUDIO_MENU.with(|slot| slot.borrow_mut().take());
    drop(audio_menu);
    let order_menu =
        TAB_ORDER_MENU.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), [None, None]));
    drop(order_menu);
    close_page_find(false, false);
    let shell = SHELL.with(|slot| slot.borrow_mut().take());
    if let Some(shell) = shell {
        // Remove menu targets before releasing the shell's retained target.
        NSApp(MainThreadMarker::new().expect("Browser shell requires the main thread"))
            .setMainMenu(None);
        shell.window.setDelegate(None);
        shell.window.setInitialFirstResponder(None);
        shell.window.makeFirstResponder(None);
        shell.window.close();
        // Close need not release a retained NSWindow's contentView. Explicitly
        // detach both ownership edges while callbacks see SHELL == None.
        shell.window.setContentView(None);
        for view in shell.views.values() {
            view.removeFromSuperview();
        }
        drop(shell);
        SYMBOLS.with(|cache| cache.borrow_mut().clear());
    }
}

/// App-level shortcuts apply only while our browser window is key.
fn is_page_input_type(kind: NSEventType, delta_x: f64, delta_y: f64) -> bool {
    matches!(
        kind,
        NSEventType::LeftMouseDown
            | NSEventType::RightMouseDown
            | NSEventType::OtherMouseDown
            | NSEventType::KeyDown
    ) || (kind == NSEventType::ScrollWheel && (delta_x != 0.0 || delta_y != 0.0))
}

fn is_menu_only_shortcut(key: &str, modifiers: NSEventModifierFlags) -> bool {
    modifiers.contains(NSEventModifierFlags::Command)
        && !modifiers.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option)
        && if modifiers.contains(NSEventModifierFlags::Shift) {
            matches!(key, "b" | "s")
        } else {
            key == "q"
        }
}

fn is_page_edit_action(action: Sel) -> bool {
    [
        sel!(undo:),
        sel!(redo:),
        sel!(cut:),
        sel!(copy:),
        sel!(paste:),
        sel!(selectAll:),
    ]
    .contains(&action)
}

fn visible_page_input_target() -> Option<(i32, Retained<NSWindow>, Retained<NSView>)> {
    // AppKit calls and handoff must not cross a shell RefCell borrow.
    let (tab, window, view) = SHELL.with(|slot| {
        let slot = slot.try_borrow().ok()?;
        let shell = slot.as_ref()?;
        if shell.overview || shell.overlay_is_open() {
            return None;
        }
        let tab = shell.active?;
        Some((tab, shell.window.clone(), shell.views.get(&tab)?.clone()))
    })?;
    if view.window().as_deref() != Some(&*window)
        || !window.isVisible()
        || window.isMiniaturized()
        || view.isHiddenOrHasHiddenAncestor()
    {
        return None;
    }
    Some((tab, window, view))
}

fn context_menu_page(
    tabs: &[ShellTab],
    space: &str,
    active: Option<i32>,
    overview: bool,
    overlay: bool,
    start_page_visible: bool,
    tab: i32,
) -> bool {
    !overview
        && !overlay
        && !start_page_visible
        && active == Some(tab)
        && tabs
            .iter()
            .any(|item| item.id == tab && item.space == space && !item.crashed)
}

/// A CEF context menu may outlive selection or an overlay change. Validate its
/// native visibility again at command time without changing focus or drafts.
#[cfg(not(test))]
pub(crate) fn context_menu_target_is_visible(tab: i32) -> bool {
    let Some((selected, window, view)) = visible_page_input_target() else {
        return false;
    };
    if selected != tab || !window.isKeyWindow() {
        return false;
    }
    // All native getters ran without a SHELL borrow. They may have dispatched
    // callbacks, so the retained window/view must still be the current pair.
    SHELL.with(|slot| {
        let Ok(slot) = slot.try_borrow() else {
            return false;
        };
        let Some(shell) = slot.as_ref() else {
            return false;
        };
        std::ptr::eq(&*shell.window, &*window)
            && shell
                .views
                .get(&tab)
                .is_some_and(|current| std::ptr::eq(&**current, &*view))
            && context_menu_page(
                &shell.tabs,
                &shell.active_space,
                shell.active,
                shell.overview,
                shell.overlay_is_open(),
                shell.start_page.visible,
                tab,
            )
    })
}

#[cfg(not(test))]
pub(crate) fn context_menu_rejected(tab: i32) {
    context_menu_notice(
        tab,
        "페이지나 제어권이 바뀌어 메뉴를 실행하지 않았습니다. 메뉴를 다시 열어 주세요.",
    );
}

#[cfg(not(test))]
pub(crate) fn context_link_failed(tab: i32) {
    context_menu_notice(tab, "링크를 새 탭에서 열지 못했습니다. 다시 시도해 주세요.");
}

#[cfg(not(test))]
fn context_menu_notice(tab: i32, message: &str) {
    let changed = SHELL.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        let Some(shell) = slot.as_mut() else {
            return false;
        };
        if !context_menu_page(
            &shell.tabs,
            &shell.active_space,
            shell.active,
            shell.overview,
            shell.overlay_is_open(),
            shell.start_page.visible,
            tab,
        ) {
            return false;
        }
        shell.notice = Some(message.into());
        true
    });
    if changed {
        crate::browser::notify_panel();
    }
}

fn page_has_keyboard_focus(window: &NSWindow, view: &NSView) -> bool {
    window.isKeyWindow()
        && window.firstResponder().is_some_and(|responder| {
            responder
                .downcast_ref::<NSView>()
                .is_some_and(|responder| responder.isDescendantOf(view))
        })
}

fn handoff_observed_page_input(tab: i32) {
    if let Err(error) = crate::browser::handoff_page_input(tab) {
        SHELL.with(|slot| {
            if let Some(shell) = slot.borrow_mut().as_mut() {
                shell.notice = Some(format!("AI 중지를 완료하지 못했습니다: {error}"));
            }
        });
        crate::browser::notify_panel();
    }
}

/// Mouse-selected Edit menus do not produce a key event in the page. Observe
/// their original action without replacing nil targets or menu validation.
pub fn observe_page_menu_action(action: Sel, sender: Option<&AnyObject>) {
    if !is_page_edit_action(action)
        || sender
            .and_then(|sender| sender.downcast_ref::<NSMenuItem>())
            .is_none()
        || MainThreadMarker::new().is_none()
    {
        return;
    }
    if let Some((tab, window, view)) = visible_page_input_target() {
        if page_has_keyboard_focus(&window, &view) {
            handoff_observed_page_input(tab);
        }
    }
}

/// AppKit input is distinct from our DOM automation, but is not proof of a
/// physical device: OS automation may also send these events. Never consume it.
pub fn observe_page_input(event: &NSEvent) {
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    let kind = event.r#type();
    // Only validated autoscroll advances drag geometry. Manual wheel scrolling
    // cancels rather than assigning an anchor from an obsolete row location.
    if kind == NSEventType::ScrollWheel {
        cancel_tab_drag();
    }
    let (delta_x, delta_y) = if kind == NSEventType::ScrollWheel {
        (event.scrollingDeltaX(), event.scrollingDeltaY())
    } else {
        (0.0, 0.0)
    };
    if !is_page_input_type(kind, delta_x, delta_y) {
        return;
    }
    if kind == NSEventType::KeyDown
        && shortcut_key(event).is_some_and(|key| is_menu_only_shortcut(&key, event.modifierFlags()))
    {
        // These are consumed later by NSMenu, after NSApplication.sendEvent:.
        return;
    }
    let Some((tab, window, view)) = visible_page_input_target() else {
        return;
    };
    if event.window(marker).as_deref() != Some(&*window) {
        return;
    }
    let targets_page = if kind == NSEventType::KeyDown {
        page_has_keyboard_focus(&window, &view)
    } else {
        // The retained CEF view is attached to our live window on the main thread.
        unsafe { view.superview() }.is_some_and(|parent| {
            // hitTest expects coordinates in the receiver's superview.
            let point = parent.convertPoint_fromView(event.locationInWindow(), None);
            view.hitTest(point)
                .is_some_and(|hit| hit.isDescendantOf(&view))
        })
    };
    if targets_page {
        if matches!(
            kind,
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown
        ) && page_find_is_editing()
        {
            // A real hit on the visible page explicitly leaves the find editor.
            // This releases its CEF focus guard before the original mouse event
            // is dispatched; scroll and asynchronous focus requests do not.
            window.makeFirstResponder(None);
        }
        handoff_observed_page_input(tab);
    }
}

pub fn handle_shortcut(event: &NSEvent) -> bool {
    if event.r#type() != NSEventType::KeyDown {
        return false;
    }
    let dragging = TAB_DRAG.with(|slot| slot.borrow().is_some());
    cancel_tab_drag();
    if dragging && event.keyCode() == 53 {
        return true;
    }
    let window = SHELL.with(|slot| slot.borrow().as_ref().map(|shell| shell.window.clone()));
    if !window.is_some_and(|window| window.isKeyWindow()) {
        return false;
    }
    if handle_dialog_key(event) {
        return true;
    }
    let Some(key) = shortcut_key(event) else {
        return false;
    };
    let modifiers = event.modifierFlags();
    // Consume reserved tab chords before page-input handoff, including missing
    // positions and repeats. Validate live membership before changing any UI.
    if let Some(number) = numbered_tab_shortcut(&key, modifiers) {
        if !event.isARepeat() {
            select_tab_command(|shell| {
                numbered_tab_target(
                    &shell.tabs,
                    &shell.active_space,
                    number,
                    shell.active,
                    shell.overview,
                    shell.overlay_is_open(),
                )
            });
        }
        return true;
    }
    if let Some(forward) = cycle_tab_shortcut(&key, modifiers) {
        if !event.isARepeat() {
            cycle_tab(forward);
        }
        return true;
    }
    // Consume this chord before page-input handoff, even when reopening is disabled.
    if is_reopen_shortcut(&key, modifiers) {
        if !event.isARepeat() {
            reopen_closed_tab();
        }
        return true;
    }
    let command = modifiers.contains(NSEventModifierFlags::Command)
        && !modifiers.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option);
    // Consume before overlay dismissal or page-input handoff. Hidden-page and
    // composing-text cases remain unchanged, including repeated key events.
    if command && key == "r" {
        if !event.isARepeat() {
            navigation_control_action(None);
        }
        return true;
    }
    if command && key == "f" && !modifiers.contains(NSEventModifierFlags::Shift) {
        if !event.isARepeat() {
            open_page_find(false);
        }
        return true;
    }
    if command && key == "g" {
        if !event.isARepeat() {
            step_page_find(!modifiers.contains(NSEventModifierFlags::Shift));
        }
        return true;
    }
    let find_open = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.find.state.open)
    });
    if find_open
        && !modifiers.intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        )
        && !page_find_has_marked_text()
    {
        match event.keyCode() {
            53 => {
                close_page_find(true, true);
                return true;
            }
            36 | 76 if page_find_is_editing() => {
                step_page_find(!modifiers.contains(NSEventModifierFlags::Shift));
                return true;
            }
            _ => {}
        }
    }
    if command && modifiers.contains(NSEventModifierFlags::Shift) && key == "a" {
        if !event.isARepeat() {
            open_tab_search();
        }
        return true;
    }
    if command && !modifiers.contains(NSEventModifierFlags::Shift) && key == "d" {
        if !event.isARepeat() {
            open_bookmarks(true);
        }
        return true;
    }
    if site_permissions_are_open() {
        if event.keyCode() == 53
            && !modifiers.intersects(
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Option,
            )
            && !page_find_has_marked_text()
        {
            close_site_permissions(true);
            return true;
        }
        if command
            && (matches!(key.as_str(), "l" | "t" | "w" | "[" | "]")
                || is_menu_only_shortcut(&key, modifiers))
        {
            close_site_permissions(false);
        }
    }
    if downloads_are_open() {
        if event.keyCode() == 53
            && !modifiers.intersects(
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Option,
            )
            && !page_find_has_marked_text()
        {
            close_downloads(true);
            return true;
        }
        if command
            && (matches!(key.as_str(), "l" | "t" | "w" | "[" | "]")
                || is_menu_only_shortcut(&key, modifiers))
        {
            close_downloads(false);
        }
    }
    if bookmarks_are_open() {
        if !modifiers.intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        ) && !bookmarks_have_marked_text()
        {
            let (editing, query_focus, name_focus) = SHELL.with(|slot| {
                let slot = slot.borrow();
                let panel = &slot.as_ref().unwrap().bookmarks;
                let query: *mut AnyObject = unsafe { msg_send![&*panel.query, currentEditor] };
                let name: *mut AnyObject = unsafe { msg_send![&*panel.name, currentEditor] };
                (
                    panel.state.draft.is_some(),
                    !query.is_null(),
                    !name.is_null(),
                )
            });
            match event.keyCode() {
                53 => {
                    if editing {
                        cancel_bookmark_editor();
                    } else {
                        close_bookmarks(true);
                    }
                    return true;
                }
                125 if query_focus => {
                    move_bookmark_selection(true);
                    return true;
                }
                126 if query_focus => {
                    move_bookmark_selection(false);
                    return true;
                }
                36 | 76 if name_focus => {
                    save_bookmark();
                    return true;
                }
                36 | 76 if query_focus => {
                    submit_bookmarks();
                    return true;
                }
                _ => {}
            }
        }
        if command
            && (matches!(key.as_str(), "l" | "t" | "w" | "[" | "]")
                || is_menu_only_shortcut(&key, modifiers))
        {
            close_bookmarks(false);
        }
    }
    if SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.search.open)
    }) {
        if !modifiers.intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        ) && !search_has_marked_text()
        {
            let search_key = event.keyCode();
            match search_key {
                53 => {
                    let cancelled = SHELL.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let Some(shell) = slot.as_mut() else {
                            return false;
                        };
                        if !shell.search.confirm_clear {
                            return false;
                        }
                        shell.search.confirm_clear = false;
                        shell.search.notice = None;
                        true
                    });
                    if cancelled {
                        sync();
                    } else {
                        close_tab_search(true);
                    }
                    return true;
                }
                _ => {}
            }
            if matches!(search_key, 36 | 76 | 125 | 126) {
                let query_focus = search_query_has_focus();
                let action = SHELL.with(|slot| {
                    let slot = slot.borrow();
                    let shell = slot.as_ref().filter(|shell| shell.search.open)?;
                    search_query_key(search_key, query_focus, shell.search.confirm_clear)
                });
                if let Some(action) = action {
                    match action {
                        SearchQueryKey::Move(forward) => move_search_selection(forward),
                        SearchQueryKey::Submit => submit_tab_search(),
                        SearchQueryKey::Consume => {}
                    }
                    return true;
                }
            }
        }
        if command
            && (matches!(key.as_str(), "l" | "t" | "w" | "[" | "]")
                || is_menu_only_shortcut(&key, modifiers))
        {
            close_tab_search(false);
        }
    }
    if !command || !matches!(key.as_str(), "l" | "t" | "w" | "[" | "]") {
        return false;
    }
    if event.isARepeat() {
        return true;
    }
    match key.as_str() {
        "l" => focus_address(),
        "t" => run_action(Some(ShellAction::NewTab)),
        "w" => run_action(active_tab().map(ShellAction::CloseTab)),
        "[" => run_action(active_tab().map(ShellAction::Back)),
        "]" => run_action(active_tab().map(ShellAction::Forward)),
        _ => unreachable!(),
    }
    true
}

pub(crate) fn focus_address() {
    if search_is_open() {
        close_tab_search(false);
        close_bookmarks(false);
        close_downloads(false);
        close_site_permissions(false);
    }
    let controls = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|shell| (shell.window.clone(), shell.address.clone()))
    });
    if let Some((window, address)) = controls {
        window.makeFirstResponder(Some(&address));
        unsafe {
            let _: () = msg_send![&*address, selectText: std::ptr::null::<AnyObject>()];
        }
    }
}

/// Standard AppKit key equivalents also work when Chromium owns the responder.
fn install_menu(target: &ShellTarget, marker: MainThreadMarker) {
    let main = NSMenu::initWithTitle(NSMenu::alloc(marker), &NSString::from_str("Naru"));
    let app = submenu(&main, "Naru", marker);
    menu_item(
        &app,
        "Naru 종료",
        Some(sel!(terminate:)),
        "q",
        None,
        false,
        marker,
    );

    let file = submenu(&main, "파일", marker);
    menu_item(
        &file,
        "새 탭",
        Some(sel!(newTab:)),
        "t",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &file,
        "현재 주소를 내 공간 새 탭에서 열기",
        Some(sel!(openCurrentPageInUserTab:)),
        "",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &file,
        "탭 닫기",
        Some(sel!(closeTab:)),
        "w",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &file,
        "최근 닫은 탭 다시 열기",
        Some(sel!(reopenClosedTab:)),
        "t",
        Some(target),
        true,
        marker,
    );

    let bookmarks = submenu(&main, "북마크", marker);
    menu_item(
        &bookmarks,
        "현재 페이지 추가 또는 이름 편집",
        Some(sel!(addBookmark:)),
        "d",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &bookmarks,
        "북마크 보기",
        Some(sel!(showBookmarks:)),
        "",
        Some(target),
        false,
        marker,
    );

    let edit = submenu(&main, "편집", marker);
    // nil targets intentionally use the active field/web content responder chain.
    menu_item(
        &edit,
        "실행 취소",
        Some(sel!(undo:)),
        "z",
        None,
        false,
        marker,
    );
    menu_item(
        &edit,
        "다시 실행",
        Some(sel!(redo:)),
        "z",
        None,
        true,
        marker,
    );
    menu_item(
        &edit,
        "잘라내기",
        Some(sel!(cut:)),
        "x",
        None,
        false,
        marker,
    );
    menu_item(&edit, "복사", Some(sel!(copy:)), "c", None, false, marker);
    menu_item(
        &edit,
        "붙여넣기",
        Some(sel!(paste:)),
        "v",
        None,
        false,
        marker,
    );
    menu_item(
        &edit,
        "전체 선택",
        Some(sel!(selectAll:)),
        "a",
        None,
        false,
        marker,
    );
    menu_item(
        &edit,
        "페이지에서 찾기",
        Some(sel!(showPageFind:)),
        "f",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &edit,
        "다음 찾기",
        Some(sel!(nextPageFind:)),
        "g",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &edit,
        "이전 찾기",
        Some(sel!(previousPageFind:)),
        "g",
        Some(target),
        true,
        marker,
    );

    let view = submenu(&main, "보기", marker);
    view.setDelegate(Some(ProtocolObject::from_ref(target)));
    for (title, action) in [
        ("현재 탭 위로 이동", sel!(moveTabUp:)),
        ("현재 탭 아래로 이동", sel!(moveTabDown:)),
    ] {
        menu_item(&view, title, Some(action), "", Some(target), false, marker);
    }
    menu_item(
        &view,
        "현재 탭 음소거",
        Some(sel!(toggleTabAudio:)),
        "",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &view,
        "이전 탭",
        Some(sel!(previousTab:)),
        "[",
        Some(target),
        true,
        marker,
    );
    menu_item(
        &view,
        "다음 탭",
        Some(sel!(nextTab:)),
        "]",
        Some(target),
        true,
        marker,
    );
    menu_item(
        &view,
        "사이트 권한",
        Some(sel!(showSitePermissions:)),
        "",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &view,
        "다운로드",
        Some(sel!(showDownloads:)),
        "",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &view,
        "통합 검색",
        Some(sel!(showTabSearch:)),
        "a",
        Some(target),
        true,
        marker,
    );
    menu_item(
        &view,
        "방문 기록",
        Some(sel!(showHistory:)),
        "",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &view,
        "주소 선택",
        Some(sel!(focusAddress:)),
        "l",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &view,
        "새로 고침 / 로딩 중지",
        Some(sel!(reloadTab:)),
        "r",
        Some(target),
        false,
        marker,
    );
    menu_item(
        &view,
        "공간 보기",
        Some(sel!(showSpaces:)),
        "s",
        Some(target),
        true,
        marker,
    );
    menu_item(
        &view,
        "사이드바 접기 / 펼치기",
        Some(sel!(toggleSidebar:)),
        "b",
        Some(target),
        true,
        marker,
    );
    NSApp(marker).setMainMenu(Some(&main));
}

fn submenu(parent: &NSMenu, title: &str, marker: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(marker), &NSString::from_str(title));
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(marker),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        )
    };
    item.setSubmenu(Some(&menu));
    parent.addItem(&item);
    menu
}

fn menu_item(
    menu: &NSMenu,
    title: &str,
    action: Option<Sel>,
    key: &str,
    target: Option<&ShellTarget>,
    shift: bool,
    marker: MainThreadMarker,
) {
    let item = unsafe {
        let item = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(marker),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        );
        item.setTarget(target.map(|target| &***target));
        item
    };
    let modifiers = if shift {
        NSEventModifierFlags::Command | NSEventModifierFlags::Shift
    } else {
        NSEventModifierFlags::Command
    };
    item.setKeyEquivalentModifierMask(modifiers);
    menu.addItem(&item);
}

fn cycle_tab(forward: bool) {
    select_tab_command(|shell| {
        cycle_tab_target(&shell.tabs, &shell.active_space, shell.active, forward)
    });
}

fn view_pointer(view: &NSView) -> *mut c_void {
    (view as *const NSView).cast_mut().cast()
}
fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
fn child_view(parent: &NSView, marker: MainThreadMarker) -> Retained<NSView> {
    let view = NSView::initWithFrame(NSView::alloc(marker), rect(0.0, 0.0, 1.0, 1.0));
    parent.addSubview(&view);
    view
}
fn button(
    parent: &NSView,
    title: &str,
    target: &ShellTarget,
    action: Sel,
    hint: &str,
    marker: MainThreadMarker,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            marker,
        )
    };
    button.setToolTip(Some(&NSString::from_str(hint)));
    button.setBordered(false);
    button.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    unsafe {
        let _: () = msg_send![&*button, setAccessibilityLabel: &*NSString::from_str(hint)];
    }
    parent.addSubview(&button);
    button
}
fn scroll_view(
    parent: &NSView,
    document: &NSView,
    marker: MainThreadMarker,
) -> Retained<NSScrollView> {
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(marker), rect(0.0, 0.0, 1.0, 1.0));
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(false);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(document));
    parent.addSubview(&scroll);
    scroll
}

fn label(
    parent: &NSView,
    title: &str,
    size: f64,
    bold: bool,
    marker: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(title), marker);
    let font = if bold {
        medium_font(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
    label.setTextColor(Some(&NSColor::labelColor()));
    unsafe {
        let _: () = msg_send![&*label, setLineBreakMode: 4isize];
        let _: () = msg_send![&*label, setMaximumNumberOfLines: 1isize];
    }
    parent.addSubview(&label);
    label
}

fn medium_font(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightMedium })
}

fn accent_color() -> Retained<NSColor> {
    // System text colors and blue adapt to appearance without retaining a
    // per-control palette or rebuilding controls when the theme changes.
    NSColor::systemBlueColor()
}

fn is_dark(view: &NSView) -> bool {
    let names = unsafe { NSArray::from_slice(&[NSAppearanceNameDarkAqua, NSAppearanceNameAqua]) };
    view.effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&names)
        .is_some_and(|name| unsafe { &*name == NSAppearanceNameDarkAqua })
}

fn rgb(hex: u32) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        ((hex >> 16) & 255) as f64 / 255.0,
        ((hex >> 8) & 255) as f64 / 255.0,
        (hex & 255) as f64 / 255.0,
        1.0,
    )
}

fn surface_color(kind: SurfaceKind, dark: bool) -> Retained<NSColor> {
    let (light, dark_color) = match kind {
        SurfaceKind::Window => (0xF9FAFB, 0x17191E),
        SurfaceKind::Sidebar => (0xF2F4F6, 0x1D2027),
        SurfaceKind::Toolbar => (0xFFFFFF, 0x20232B),
        SurfaceKind::Card | SurfaceKind::SelectedCard => (0xFFFFFF, 0x292D36),
        SurfaceKind::Selection => (0xE7F0FF, 0x243C60),
        SurfaceKind::Preview => (0xECEFF3, 0x20242C),
        SurfaceKind::Address => (0xF2F4F6, 0x2B303A),
        SurfaceKind::Badge => (0xF2F4F6, 0x2E333E),
        SurfaceKind::AccentBadge => (0xE7F0FF, 0x243C60),
        SurfaceKind::Primary => (0x1764D9, 0x246FE4),
        SurfaceKind::Separator => (0xE2E6EB, 0x343A46),
    };
    rgb(if dark { dark_color } else { light })
}

fn symbol_image(name: &'static str) -> Option<Retained<NSImage>> {
    if let Some(image) = SYMBOLS.with(|cache| cache.borrow().get(name).cloned()) {
        return Some(image);
    }
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        13.0,
        unsafe { NSFontWeightMedium },
        NSImageSymbolScale::Medium,
    );
    let image = image
        .imageWithSymbolConfiguration(&configuration)
        .unwrap_or(image);
    image.setTemplate(true);
    SYMBOLS.with(|cache| {
        cache.borrow_mut().insert(name, image.clone());
    });
    Some(image)
}

fn set_symbol(button: &NSButton, name: &'static str, title: &str, fallback: &str, hint: &str) {
    if let Some(image) = symbol_image(name) {
        button.setImage(Some(&image));
        button.setImagePosition(if title.is_empty() {
            NSCellImagePosition::ImageOnly
        } else {
            NSCellImagePosition::ImageLeading
        });
        button.setTitle(&NSString::from_str(if title.is_empty() {
            ""
        } else {
            title
        }));
    } else {
        button.setImage(None);
        button.setTitle(&NSString::from_str(fallback));
    }
    button.setToolTip(Some(&NSString::from_str(hint)));
    unsafe {
        let _: () = msg_send![button, setAccessibilityLabel: &*NSString::from_str(hint)];
    }
}

fn symbol_view(
    parent: &NSView,
    name: &'static str,
    hint: &str,
    marker: MainThreadMarker,
) -> Retained<NSImageView> {
    let view = NSImageView::initWithFrame(NSImageView::alloc(marker), rect(0.0, 0.0, 16.0, 16.0));
    view.setImage(symbol_image(name).as_deref());
    view.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    view.setToolTip(Some(&NSString::from_str(hint)));
    unsafe {
        let _: () = msg_send![&*view, setAccessibilityElement: false];
    }
    parent.addSubview(&view);
    view
}

fn align_left(button: &NSButton) {
    unsafe {
        let _: () = msg_send![button, setAlignment: 0isize];
    }
}
fn align_center(button: &NSButton) {
    unsafe {
        let _: () = msg_send![button, setAlignment: 1isize];
    }
}

fn surface_child(
    parent: &NSView,
    kind: SurfaceKind,
    radius: f64,
    border: bool,
    marker: MainThreadMarker,
) -> Retained<SurfaceView> {
    let view = SurfaceView::new(marker, kind, radius, border);
    parent.addSubview(&view);
    view
}

fn space_stop_scope(space: &crate::spaces::SpaceView) -> Option<TaskScope> {
    (space.kind == "task" && space.status == "active").then(|| TaskScope {
        task: space.id.clone(),
        revision: space.revision,
    })
}

fn space_resume_scope(space: &crate::spaces::SpaceView) -> Option<TaskScope> {
    (space.kind == "task" && space.status == "paused").then(|| TaskScope {
        task: space.id.clone(),
        revision: space.revision,
    })
}

fn space_status(kind: &str, status: &str) -> &'static str {
    if kind == "user" {
        return "직접 탐색";
    }
    match status {
        "active" => "AI 제어 중",
        "paused" => "AI 중지됨",
        "finishing" => "마무리 중",
        "finished" | "done" => "완료",
        _ => "AI 공간",
    }
}
fn compact(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut text: String = chars
        .by_ref()
        .take(limit)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if chars.next().is_some() {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered_tab_fixture(id: i32, space: &str) -> ShellTab {
        ShellTab {
            id,
            title: String::new(),
            url: "about:blank".into(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_muted: None,
            crashed: false,
            failure: None,
            start_page: false,
            task_status: String::new(),
            dialog_pending: false,
            dialog_user: false,
            stop_scope: None,
            space: space.into(),
            navigation_epoch: 1,
        }
    }

    #[test]
    fn tab_audio_target_requires_selected_live_space_without_overlay() {
        let mut tabs = vec![
            numbered_tab_fixture(1, "user"),
            numbered_tab_fixture(2, "task-1"),
        ];
        tabs[1].navigation_epoch = 7;
        let original = tab_audio_page(&tabs, "task-1", Some(2), false, false).unwrap();
        assert_eq!(original, (2, "task-1".into(), 7));
        assert!(tab_audio_page(&tabs, "user", Some(2), false, false).is_none());
        assert!(tab_audio_page(&tabs, "task-1", Some(2), true, false).is_none());
        assert!(tab_audio_page(&tabs, "task-1", Some(2), false, true).is_none());
        assert!(tab_audio_page(&tabs, "task-1", None, false, false).is_none());
        tabs[1].navigation_epoch = 8;
        assert_ne!(
            tab_audio_page(&tabs, "task-1", Some(2), false, false),
            Some(original)
        );
        tabs[1].crashed = true;
        assert!(tab_audio_page(&tabs, "task-1", Some(2), false, false).is_none());
        tabs.pop();
        assert!(tab_audio_page(&tabs, "task-1", Some(2), false, false).is_none());
        // Audio is a tab setting: this resolver has no HTTP-only URL restriction.
        assert_eq!(
            tab_audio_page(&tabs, "user", Some(1), false, false),
            Some((1, "user".into(), 1))
        );
    }

    #[test]
    fn tab_audio_icon_reports_only_a_known_muted_setting() {
        for ai_active in [false, true] {
            for loading in [false, true] {
                assert_eq!(
                    tab_status_symbol(None, Some(true), ai_active, loading),
                    "speaker.slash"
                );
                for muted in [None, Some(false)] {
                    assert_eq!(
                        tab_status_symbol(None, muted, ai_active, loading),
                        if ai_active {
                            "sparkles"
                        } else if loading {
                            "arrow.triangle.2.circlepath"
                        } else {
                            "globe"
                        },
                    );
                }
            }
        }
    }

    #[test]
    fn background_tab_failure_changes_the_row_signature_without_other_display_changes() {
        let mut tab = numbered_tab_fixture(2, "user");
        let signature =
            |tab: &ShellTab| tab_rows_signature("user", Some(1), false, false, false, &[tab]);
        let healthy = signature(&tab);
        tab.failure = Some(PageFailure::Load { code: -105 });
        let failed_load = signature(&tab);
        assert_ne!(failed_load, healthy);
        tab.failure = Some(PageFailure::Renderer);
        assert_ne!(signature(&tab), healthy);
        assert_ne!(signature(&tab), failed_load);
        // Recovery can suppress the failure icon while renderer_exit remains.
        // The row still needs to disable its audio control for that renderer.
        tab.crashed = true;
        tab.failure = None;
        assert_ne!(signature(&tab), healthy);
        tab.crashed = false;
        assert_eq!(signature(&tab), healthy);
    }

    #[test]
    fn tab_failure_icon_takes_priority_over_muting_ai_and_loading() {
        for failure in [PageFailure::Load { code: -105 }, PageFailure::Renderer] {
            for muted in [None, Some(false), Some(true)] {
                for ai_active in [false, true] {
                    for loading in [false, true] {
                        assert_eq!(
                            tab_status_symbol(Some(failure), muted, ai_active, loading),
                            "exclamationmark.triangle"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn tab_failure_description_preserves_task_loading_and_muted_status() {
        let mut tab = numbered_tab_fixture(2, "task-1");
        tab.task_status = "자료 조사, AI 제어 중".into();
        tab.loading = true;
        tab.audio_muted = Some(true);
        for (failure, message) in [
            (PageFailure::Load { code: -105 }, "페이지를 열지 못했습니다"),
            (PageFailure::Renderer, "페이지가 종료됐습니다"),
        ] {
            tab.failure = Some(failure);
            assert_eq!(
                tab_status_description(&tab),
                format!("자료 조사, AI 제어 중, {message}, 불러오는 중, 음소거 설정됨")
            );
        }
        tab.failure = None;
        assert_eq!(
            tab_status_description(&tab),
            "자료 조사, AI 제어 중, 불러오는 중, 음소거 설정됨"
        );
        tab.task_status = "직접 사용 중".into();
        tab.loading = false;
        tab.audio_muted = Some(false);
        assert_eq!(tab_status_description(&tab), "직접 사용 중");
    }

    #[test]
    fn pending_dialog_is_described_without_changing_other_status() {
        let mut tab = numbered_tab_fixture(2, "task-1");
        tab.task_status = "자료 조사, AI 제어 중".into();
        tab.dialog_pending = true;
        assert_eq!(
            tab_status_description(&tab),
            "자료 조사, AI 제어 중, 대화상자 응답 대기"
        );
        tab.loading = true;
        assert_eq!(
            tab_status_description(&tab),
            "자료 조사, AI 제어 중, 대화상자 응답 대기, 불러오는 중"
        );
    }

    #[test]
    fn current_page_target_captures_the_selected_space_and_exact_navigation() {
        let mut tabs = vec![
            numbered_tab_fixture(1, "user"),
            numbered_tab_fixture(2, "task-1"),
        ];
        tabs[0].url = "https://example.test/user".into();
        tabs[1].url = "https://example.test/page?q=one#section".into();
        tabs[1].navigation_epoch = 7;
        let target = current_page_target(&tabs, "task-1", Some(2), false, false).unwrap();
        assert_eq!(target.tab, 2);
        assert_eq!(target.space, "task-1");
        assert_eq!(target.navigation_epoch, 7);
        assert_eq!(target.url, tabs[1].url);

        tabs[0].title = "background update".into();
        assert_eq!(
            current_page_target(&tabs, "task-1", Some(2), false, false),
            Some(target.clone())
        );
        tabs[1].navigation_epoch += 1;
        assert_ne!(
            current_page_target(&tabs, "task-1", Some(2), false, false),
            Some(target.clone())
        );
        tabs[1].navigation_epoch = target.navigation_epoch;
        tabs[1].url.push_str("-changed");
        assert_ne!(
            current_page_target(&tabs, "task-1", Some(2), false, false),
            Some(target)
        );
    }

    #[test]
    fn current_page_target_rejects_hidden_selection_crash_and_unsupported_addresses() {
        let mut tabs = vec![numbered_tab_fixture(2, "user")];
        tabs[0].url = "https://example.test/page".into();
        for (space, active, overview, overlay) in [
            ("task-1", Some(2), false, false),
            ("user", Some(3), false, false),
            ("user", None, false, false),
            ("user", Some(2), true, false),
            ("user", Some(2), false, true),
        ] {
            assert!(current_page_target(&tabs, space, active, overview, overlay).is_none());
        }
        tabs[0].crashed = true;
        assert!(current_page_target(&tabs, "user", Some(2), false, false).is_none());
        tabs[0].crashed = false;
        for url in [
            "about:blank",
            "file:///tmp/example",
            "data:text/html,test",
            "javascript:void(0)",
            "https://name:secret@example.test/",
            "https://",
        ] {
            tabs[0].url = url.into();
            assert!(current_page_target(&tabs, "user", Some(2), false, false).is_none());
        }
        tabs[0].url = "http://127.0.0.1:1234/page".into();
        assert!(current_page_target(&tabs, "user", Some(2), false, false).is_some());
    }

    #[test]
    fn download_reveal_verifies_the_returned_tab_and_live_space() {
        let mut snapshot = ShellSnapshot {
            tabs: vec![
                numbered_tab_fixture(2, "task-1"),
                numbered_tab_fixture(3, "user"),
            ],
            spaces: vec![],
            active_space: "task-1".into(),
            active_tab: Some(2),
            overview: false,
        };
        assert_eq!(download_reveal_selection(&snapshot, 2), Some("task-1"));
        // A stale return must not close the panel for another selected tab.
        assert!(download_reveal_selection(&snapshot, 3).is_none());
        snapshot.active_space = "user".into();
        assert!(download_reveal_selection(&snapshot, 2).is_none());
        snapshot.active_tab = Some(3);
        assert_eq!(download_reveal_selection(&snapshot, 3), Some("user"));
        snapshot.overview = true;
        assert!(download_reveal_selection(&snapshot, 3).is_none());
        snapshot.overview = false;
        snapshot.tabs.retain(|tab| tab.id != 3);
        assert!(download_reveal_selection(&snapshot, 3).is_none());
    }

    #[test]
    fn download_source_buttons_fit_both_row_kinds_at_narrow_content_widths() {
        // Includes the 584pt minimum content minus panel margins and a legacy
        // vertical scroller. File rows share the primary frame between Save As
        // and Finder. This checks geometry, not native rendering.
        for width in [480.0, 497.0, 512.0, 800.0] {
            for permission in [false, true] {
                let [reveal, primary, cancel] = download_action_frames(width, permission);
                assert!(reveal.origin.x + reveal.size.width < primary.origin.x);
                assert!(primary.origin.x + primary.size.width < cancel.origin.x);
                for frame in [reveal, primary, cancel] {
                    assert!(frame.origin.x >= 0.0);
                    assert!(frame.origin.x + frame.size.width <= width);
                    assert_eq!(frame.origin.y, 10.0);
                    assert_eq!(frame.size.height, 30.0);
                }
            }
        }
    }

    #[test]
    fn download_site_target_requires_its_overlay_and_tracks_the_underlying_page() {
        let mut tabs = vec![
            numbered_tab_fixture(2, "task-1"),
            numbered_tab_fixture(3, "user"),
        ];
        tabs[0].url = "https://example.test/source".into();
        tabs[0].navigation_epoch = 7;
        tabs[1].url = "https://other.test/page".into();
        let target =
            download_site_page_target(&tabs, "task-1", Some(2), false, true, false).unwrap();
        assert_eq!(
            target,
            current_page_target(&tabs, "task-1", Some(2), false, false).unwrap()
        );
        // Other spaces, closed panels and overlapping native views cannot
        // turn the displayed site's button into a different setting target.
        for (space, active, overview, open, other) in [
            ("task-1", Some(2), false, false, false),
            ("task-1", Some(2), false, true, true),
            ("task-1", Some(2), true, true, false),
            ("user", Some(2), false, true, false),
            ("task-1", Some(3), false, true, false),
            ("task-1", None, false, true, false),
        ] {
            assert!(
                download_site_page_target(&tabs, space, active, overview, open, other).is_none()
            );
        }
        tabs[1].title = "background refresh".into();
        assert_eq!(
            download_site_page_target(&tabs, "task-1", Some(2), false, true, false),
            Some(target.clone())
        );
        tabs[0].navigation_epoch += 1;
        assert_ne!(
            download_site_page_target(&tabs, "task-1", Some(2), false, true, false),
            Some(target.clone())
        );
        tabs[0].navigation_epoch = target.navigation_epoch;
        tabs[0].url.push_str("?changed");
        assert_ne!(
            download_site_page_target(&tabs, "task-1", Some(2), false, true, false),
            Some(target)
        );
        tabs[0].crashed = true;
        assert!(download_site_page_target(&tabs, "task-1", Some(2), false, true, false).is_none());
        tabs[0].crashed = false;
        tabs[0].url = "about:blank".into();
        assert!(download_site_page_target(&tabs, "task-1", Some(2), false, true, false).is_none());
    }

    #[test]
    fn download_site_nested_query_keeps_only_the_same_displayed_target() {
        let page = crate::browser::CurrentPageTarget {
            tab: 2,
            space: "user".into(),
            navigation_epoch: 7,
            url: "https://example.test/page".into(),
        };
        let mut next = page.clone();
        next.navigation_epoch += 1;
        assert!(matches!(
            DownloadSiteQuery::Skipped.for_target(Some(&page), Some(&page)),
            DownloadSiteQuery::Skipped,
        ));
        for expected in [None, Some(&next)] {
            assert!(matches!(
                DownloadSiteQuery::Skipped.for_target(expected, Some(&page)),
                DownloadSiteQuery::Ready(None),
            ));
        }
        // A completed query with no target is a real invalidation, not a skip.
        assert!(matches!(
            DownloadSiteQuery::Ready(None).for_target(Some(&page), Some(&page)),
            DownloadSiteQuery::Ready(None),
        ));
        let failed = || DownloadSiteQuery::Ready(Some((page.clone(), Err("unavailable".into()))));
        assert!(matches!(
            failed().for_target(Some(&page), Some(&page)),
            DownloadSiteQuery::Ready(Some(_)),
        ));
        assert!(matches!(
            failed().for_target(Some(&next), Some(&page)),
            DownloadSiteQuery::Ready(None),
        ));
    }

    #[test]
    fn start_page_target_requires_verified_idle_blank_and_tracks_navigation_identity() {
        let mut tabs = vec![ShellTab {
            id: 1,
            title: String::new(),
            url: "about:blank".into(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_muted: None,
            crashed: false,
            failure: None,
            start_page: true,
            task_status: String::new(),
            dialog_pending: false,
            dialog_user: false,
            stop_scope: None,
            space: "user".into(),
            navigation_epoch: 3,
        }];
        let target = start_page_target(&tabs, "user", Some(1)).unwrap();
        assert!(start_page_target(&tabs, "task-1", Some(1)).is_none());
        assert!(start_page_target(&tabs, "user", Some(2)).is_none());
        assert!(start_page_target(&tabs, "user", None).is_none());
        for unavailable in [
            |tab: &mut ShellTab| tab.start_page = false,
            |tab: &mut ShellTab| tab.loading = true,
            |tab: &mut ShellTab| tab.crashed = true,
            |tab: &mut ShellTab| tab.failure = Some(PageFailure::Renderer),
            |tab: &mut ShellTab| tab.url = "https://example.test/".into(),
        ] {
            unavailable(&mut tabs[0]);
            assert!(start_page_target(&tabs, "user", Some(1)).is_none());
            tabs[0].start_page = true;
            tabs[0].loading = false;
            tabs[0].crashed = false;
            tabs[0].failure = None;
            tabs[0].url = "about:blank".into();
        }
        tabs[0].title = "background title change".into();
        assert_eq!(
            start_page_target(&tabs, "user", Some(1)),
            Some(target.clone())
        );
        tabs[0].navigation_epoch += 1;
        assert_ne!(start_page_target(&tabs, "user", Some(1)), Some(target));
    }

    #[test]
    fn start_page_form_fits_the_smallest_native_content_area() {
        for size in [
            NSSize::new(584.0, 324.0),
            NSSize::new(724.0, 464.0),
            NSSize::new(964.0, 744.0),
        ] {
            let frames = start_page_frames(size);
            assert!(frames.input.size.width >= 420.0);
            assert_eq!(frames.input.origin.y, frames.open.origin.y);
            assert!(frames.input.origin.x + frames.input.size.width + 12.0 <= frames.open.origin.x);
            assert!(
                frames.bookmarks.origin.y + frames.bookmarks.size.height < frames.hint.origin.y
            );
            assert!(frames.hint.origin.y + frames.hint.size.height < frames.input.origin.y);
            assert!(frames.input.origin.y + frames.input.size.height < frames.subtitle.origin.y);
            for frame in [
                frames.title,
                frames.subtitle,
                frames.input,
                frames.open,
                frames.hint,
                frames.bookmarks,
            ] {
                assert!(frame.origin.x >= 24.0 && frame.origin.y >= 0.0);
                assert!(frame.origin.x + frame.size.width <= size.width - 24.0);
                assert!(frame.origin.y + frame.size.height <= size.height - 24.0);
            }
        }
    }

    #[test]
    fn page_recovery_is_bound_to_the_visible_tab_and_exact_failure() {
        let mut tabs = vec![ShellTab {
            id: 7,
            title: "".into(),
            url: "https://example.invalid/".into(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_muted: None,
            crashed: false,
            failure: Some(PageFailure::Load { code: -102 }),
            start_page: false,
            task_status: String::new(),
            dialog_pending: false,
            dialog_user: false,
            stop_scope: None,
            space: "task-1".into(),
            navigation_epoch: 5,
        }];
        let shown = visible_page_recovery_target(&tabs, "task-1", Some(7), false, false).unwrap();
        assert_eq!(shown.tab, 7);
        assert_eq!(shown.navigation_epoch, 5);
        for (space, tab, overview, overlay) in [
            ("user", Some(7), false, false),
            ("task-1", Some(8), false, false),
            ("task-1", None, false, false),
            ("task-1", Some(7), true, false),
            ("task-1", Some(7), false, true),
        ] {
            assert!(visible_page_recovery_target(&tabs, space, tab, overview, overlay).is_none());
        }
        tabs[0].navigation_epoch += 1;
        assert_ne!(
            visible_page_recovery_target(&tabs, "task-1", Some(7), false, false),
            Some(shown.clone())
        );
        tabs[0].navigation_epoch = shown.navigation_epoch;
        tabs[0].failure = Some(PageFailure::Load { code: -105 });
        assert_ne!(
            visible_page_recovery_target(&tabs, "task-1", Some(7), false, false),
            Some(shown)
        );
        tabs[0].failure = None;
        assert!(visible_page_recovery_target(&tabs, "task-1", Some(7), false, false).is_none());
    }

    #[test]
    fn recovery_and_find_reserve_separate_native_bands_above_the_page() {
        for height in [520.0, 800.0] {
            for (find, recovery) in [(false, false), (true, false), (false, true), (true, true)] {
                let layout = page_vertical_layout(height, find, recovery, false);
                let find_height = if find { FIND_BAR_HEIGHT } else { 0.0 };
                let recovery_height = if recovery { PAGE_RECOVERY_HEIGHT } else { 0.0 };
                assert!(layout.page_height >= 324.0);
                assert_eq!(layout.recovery_y, layout.page_height);
                assert_eq!(layout.find_y, layout.recovery_y + recovery_height);
                assert_eq!(layout.find_y + find_height, height - TOOLBAR_HEIGHT);
            }
        }
    }

    #[test]
    fn dialog_band_sits_between_the_page_and_the_other_bands() {
        for height in [520.0, 800.0] {
            let layout = page_vertical_layout(height, true, true, true);
            assert_eq!(layout.dialog_y, layout.page_height);
            assert_eq!(layout.recovery_y, layout.dialog_y + PAGE_DIALOG_HEIGHT);
            assert_eq!(layout.find_y, layout.recovery_y + PAGE_RECOVERY_HEIGHT);
            assert_eq!(layout.find_y + FIND_BAR_HEIGHT, height - TOOLBAR_HEIGHT);
            let only = page_vertical_layout(height, false, false, true);
            assert_eq!(only.dialog_y + PAGE_DIALOG_HEIGHT, height - TOOLBAR_HEIGHT);
            assert!(only.page_height >= 1.0);
            assert_eq!(
                only.page_height,
                (height - TOOLBAR_HEIGHT - PAGE_DIALOG_HEIGHT).max(1.0)
            );
        }
    }

    #[test]
    fn user_owned_dialog_is_presented_only_for_the_shown_tab() {
        let mut tabs = vec![
            numbered_tab_fixture(7, "task-1"),
            numbered_tab_fixture(8, "task-1"),
        ];
        // AI-owned (not handed over) dialogs never get the native panel.
        tabs[0].dialog_pending = true;
        assert_eq!(
            visible_user_dialog_tab(&tabs, "task-1", Some(7), false, false),
            None
        );
        tabs[0].dialog_user = true;
        assert_eq!(
            visible_user_dialog_tab(&tabs, "task-1", Some(7), false, false),
            Some(7)
        );
        // Not the selected tab, another space, overview or an overlay: not shown, no focus.
        assert_eq!(
            visible_user_dialog_tab(&tabs, "task-1", Some(8), false, false),
            None
        );
        assert_eq!(
            visible_user_dialog_tab(&tabs, "user", Some(7), false, false),
            None
        );
        assert_eq!(
            visible_user_dialog_tab(&tabs, "task-1", Some(7), true, false),
            None
        );
        assert_eq!(
            visible_user_dialog_tab(&tabs, "task-1", Some(7), false, true),
            None
        );
    }

    #[test]
    fn dialog_focus_stays_on_the_strip_for_its_own_tab_only() {
        assert_eq!(dialog_focus_decision(None, 7, false), DialogFocus::Page);
        assert_eq!(dialog_focus_decision(Some(7), 8, false), DialogFocus::Page);
        assert_eq!(dialog_focus_decision(Some(7), 7, false), DialogFocus::Take);
        assert_eq!(dialog_focus_decision(Some(7), 7, true), DialogFocus::Keep);
    }

    #[test]
    fn dialog_keys_apply_only_inside_the_strip_or_its_page() {
        use DialogKey::{Cancel, Primary};
        let decide = |code, plain, visible, scope, marked, cancel| {
            dialog_key_decision(code, plain, visible, scope, marked, cancel)
        };
        assert_eq!(decide(36, true, true, true, false, true), Some(Primary));
        assert_eq!(decide(76, true, true, true, false, false), Some(Primary));
        assert_eq!(decide(53, true, true, true, false, true), Some(Cancel));
        // Alert has no cancel button: Esc does nothing.
        assert_eq!(decide(53, true, true, true, false, false), None);
        // Address bar or another field (focus outside the strip and page).
        assert_eq!(decide(36, true, true, false, false, true), None);
        assert_eq!(decide(53, true, true, false, false, true), None);
        // Hidden strip, modifiers, IME composition, other keys.
        assert_eq!(decide(36, true, false, true, false, true), None);
        assert_eq!(decide(36, false, true, true, false, true), None);
        assert_eq!(decide(36, true, true, true, true, true), None);
        assert_eq!(decide(48, true, true, true, false, true), None);
    }

    #[test]
    fn dialog_buttons_follow_the_dialog_kind() {
        use crate::dialogs::Kind;
        assert_eq!(dialog_button_titles(Kind::Alert), ("확인", None));
        assert_eq!(dialog_button_titles(Kind::Confirm), ("확인", Some("취소")));
        assert_eq!(dialog_button_titles(Kind::Prompt), ("확인", Some("취소")));
        assert_eq!(
            dialog_button_titles(Kind::BeforeUnload),
            ("떠나기", Some("머무르기"))
        );
    }

    #[test]
    fn toolbar_keeps_address_status_and_actions_separate_at_minimum_window_width() {
        for main_width in [
            800.0 - SIDEBAR_WIDTH,
            800.0 - RAIL_WIDTH,
            1180.0 - SIDEBAR_WIDTH,
        ] {
            for show_stop in [false, true] {
                let frames = toolbar_frames(main_width, show_stop);
                assert!(frames.address.size.width >= 240.0);
                assert!(frames.address.origin.x >= 108.0 + 12.0);
                assert!(
                    frames.address.origin.x + frames.address.size.width + 12.0
                        <= frames.status.origin.x
                );
                let last = if show_stop {
                    frames.stop
                } else {
                    frames.status
                };
                assert!(last.origin.x + last.size.width + 8.0 <= frames.close.origin.x);
                if show_stop {
                    assert!(
                        frames.status.origin.x + frames.status.size.width + 8.0
                            <= frames.stop.origin.x
                    );
                }
                assert_eq!(
                    frames.close.origin.x + frames.close.size.width + 8.0,
                    main_width
                );
                for frame in [frames.address, frames.status, frames.stop, frames.close] {
                    assert!(frame.origin.y >= 0.0);
                    assert!(frame.origin.y + frame.size.height <= TOOLBAR_HEIGHT);
                }
            }
        }
    }

    #[test]
    fn navigation_controls_capture_visible_intent_and_reject_hidden_pages() {
        let mut tabs = vec![
            numbered_tab_fixture(7, "task-a"),
            numbered_tab_fixture(8, "user"),
        ];
        tabs[0].loading = true;
        let stop = navigation_control_target(&tabs, "task-a", Some(7), false, false).unwrap();
        assert!(stop.stop);
        assert_eq!(stop.tab, 7);
        assert_eq!(stop.navigation_epoch, tabs[0].navigation_epoch);
        tabs[0].loading = false;
        let reload = navigation_control_target(&tabs, "task-a", Some(7), false, false).unwrap();
        assert!(!reload.stop);
        assert_ne!(stop, reload); // A displayed stop intent cannot become reload.
        tabs[0].navigation_epoch += 1;
        assert_ne!(
            navigation_control_target(&tabs, "task-a", Some(7), false, false),
            Some(reload),
        );
        for (overview, overlay) in [(true, false), (false, true), (true, true)] {
            assert!(
                navigation_control_target(&tabs, "task-a", Some(7), overview, overlay).is_none()
            );
        }
        assert!(navigation_control_target(&tabs, "user", Some(7), false, false).is_none());
        assert!(navigation_control_target(&tabs, "task-a", None, false, false).is_none());
    }

    #[test]
    fn stop_address_hold_follows_only_its_current_page() {
        let mut tab = numbered_tab_fixture(7, "task-a");
        tab.loading = true;
        let target =
            navigation_control_target(std::slice::from_ref(&tab), "task-a", Some(7), false, false)
                .unwrap();
        let url = tab.url.clone();
        // The synchronous hold admits ownership/stop epochs; only the runtime
        // outcome below may make that protection survive the call.
        tab.navigation_epoch += 2;
        assert!(navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(7),
            false
        ));
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "user",
            Some(7),
            false
        ));
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(8),
            false
        ));
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(7),
            true
        ));
        tab.url = "https://example.invalid/new".into();
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(7),
            false
        ));
    }

    #[test]
    fn navigation_address_rebase_needs_live_receipt_when_snapshot_is_skipped() {
        let mut tab = numbered_tab_fixture(7, "task-a");
        tab.loading = true;
        let mut target =
            navigation_control_target(std::slice::from_ref(&tab), "task-a", Some(7), false, false)
                .unwrap();
        let url = tab.url.clone();
        let original = || AddressDraft {
            tab: 7,
            url: url.clone(),
            navigation_epoch: 1,
        };
        let mut outcome = crate::browser::NavigationControlOutcome {
            navigation_epoch: 3,
            preserve_address: true,
        };
        // A loading-only callback can reject the native snapshot, leaving E
        // or handoff's E+1 displayed. The same receipt also handles a fresh E+2.
        for displayed_epoch in [1, 2, 3] {
            tab.navigation_epoch = displayed_epoch;
            let mut draft = original();
            assert!(navigation_control_page_matches(
                &target,
                &url,
                Some(&tab),
                "task-a",
                Some(7),
                false
            ));
            assert!(!rebase_navigation_address(
                &mut draft, &target, &url, &tab, &outcome, false
            ));
            assert_eq!(draft.navigation_epoch, 1);
            assert!(rebase_navigation_address(
                &mut draft, &target, &url, &tab, &outcome, true
            ));
            assert_eq!(tab.navigation_epoch, displayed_epoch);
            assert!(!draft.superseded(7, &url, 3, false));
            assert!(!rebase_navigation_address(
                &mut draft, &target, &url, &tab, &outcome, true
            ));
            assert!(draft.superseded(7, &url, 4, false));
        }
        outcome.preserve_address = false;
        let mut rejected = original();
        assert!(!rebase_navigation_address(
            &mut rejected,
            &target,
            &url,
            &tab,
            &outcome,
            true
        ));
        assert_eq!(rejected.navigation_epoch, 1);
        outcome.preserve_address = true;
        outcome.navigation_epoch = 2;
        assert!(!rebase_navigation_address(
            &mut original(),
            &target,
            &url,
            &tab,
            &outcome,
            true
        ));
        outcome.navigation_epoch = 3;
        tab.url.push_str("/changed");
        assert!(!rebase_navigation_address(
            &mut original(),
            &target,
            &url,
            &tab,
            &outcome,
            true
        ));
        tab.url = url.clone();
        target.stop = false;
        outcome.preserve_address = false; // A successful reload never supplies a receipt.
        assert!(!rebase_navigation_address(
            &mut original(),
            &target,
            &url,
            &tab,
            &outcome,
            true
        ));
        // An actual reload navigation still expires the old address context.
        assert!(!original().superseded(7, &url, 1, false));
        assert!(original().superseded(7, &url, 2, false));
    }

    #[test]
    fn handoff_failure_keeps_address_only_with_exact_runtime_evidence() {
        let mut tab = numbered_tab_fixture(7, "task-a");
        tab.loading = true;
        let target =
            navigation_control_target(std::slice::from_ref(&tab), "task-a", Some(7), false, false)
                .unwrap();
        let url = tab.url.clone();
        let original = || AddressDraft {
            tab: 7,
            url: url.clone(),
            navigation_epoch: target.navigation_epoch,
        };
        let failed = Err(crate::browser::NavigationControlError {
            message: "창 포커스가 바뀌었습니다.".into(),
            preserved_address: Some(crate::browser::NavigationControlOutcome {
                navigation_epoch: target.navigation_epoch + 1,
                preserve_address: true,
            }),
        });
        let outcome = navigation_address_outcome(&failed).unwrap();
        let mut draft = original();
        // A missing or changed runtime receipt rejects the old displayed page.
        assert!(!rebase_navigation_address(
            &mut draft, &target, &url, &tab, outcome, false
        ));
        // A valid receipt preserves the draft even when sync skipped rendering
        // after an unrelated loading callback and the handoff epoch is uncached.
        assert!(rebase_navigation_address(
            &mut draft, &target, &url, &tab, outcome, true
        ));
        assert!(!draft.superseded(7, &url, outcome.navigation_epoch, false));
        tab.navigation_epoch = outcome.navigation_epoch;

        let changed = Err(crate::browser::NavigationControlError {
            message: "문서나 제어권이 바뀌었습니다.".into(),
            preserved_address: None,
        });
        assert!(navigation_address_outcome(&changed).is_none());
        assert!(original().superseded(7, &url, tab.navigation_epoch, false));
        let reload_target = crate::browser::NavigationControlTarget {
            stop: false,
            ..target.clone()
        };
        let mut reload_draft = original();
        assert!(rebase_navigation_address(
            &mut reload_draft,
            &reload_target,
            &url,
            &tab,
            outcome,
            true,
        ));
        assert!(!reload_draft.superseded(7, &url, tab.navigation_epoch, false));
        // New edit metadata and a later same-URL page cannot be revived using
        // the old call's failure evidence.
        assert!(!rebase_navigation_address(
            &mut draft, &target, &url, &tab, outcome, true
        ));
        tab.navigation_epoch += 1;
        assert!(!rebase_navigation_address(
            &mut original(),
            &target,
            &url,
            &tab,
            outcome,
            true
        ));
    }

    #[test]
    fn opening_overlay_blocks_navigation_but_preserves_same_address_identity() {
        let mut tab = numbered_tab_fixture(7, "task-a");
        tab.loading = true;
        let target =
            navigation_control_target(std::slice::from_ref(&tab), "task-a", Some(7), false, false)
                .unwrap();
        let url = tab.url.clone();
        assert!(
            navigation_control_target(std::slice::from_ref(&tab), "task-a", Some(7), false, true)
                .is_none()
        );
        assert!(navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(7),
            false
        ));
        let failed = Err(crate::browser::NavigationControlError {
            message: "패널이 열렸습니다.".into(),
            preserved_address: Some(crate::browser::NavigationControlOutcome {
                navigation_epoch: target.navigation_epoch + 1,
                preserve_address: true,
            }),
        });
        let mut draft = AddressDraft {
            tab: 7,
            url: url.clone(),
            navigation_epoch: target.navigation_epoch,
        };
        tab.navigation_epoch += 1;
        assert!(rebase_navigation_address(
            &mut draft,
            &target,
            &url,
            &tab,
            navigation_address_outcome(&failed).unwrap(),
            true
        ));
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "user",
            Some(7),
            false
        ));
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(8),
            false
        ));
        assert!(!navigation_control_page_matches(
            &target,
            &url,
            Some(&tab),
            "task-a",
            Some(7),
            true
        ));
    }

    #[test]
    fn sidebar_close_keeps_draft_only_for_the_single_handoff_epoch() {
        let handoff = SidebarAddressHandoff {
            tab: 7,
            space: "task-1".into(),
            url: "https://example.invalid/one".into(),
            epoch: 4,
        };
        let original_draft = || AddressDraft {
            tab: handoff.tab,
            url: handoff.url.clone(),
            navigation_epoch: handoff.epoch,
        };
        let mut snapshot = ShellSnapshot {
            tabs: vec![ShellTab {
                id: 7,
                title: "초안 보존".into(),
                url: handoff.url.clone(),
                can_go_back: false,
                can_go_forward: false,
                loading: false,
                audio_muted: None,
                crashed: false,
                failure: None,
                start_page: false,
                task_status: String::new(),
                dialog_pending: false,
                dialog_user: false,
                stop_scope: None,
                space: handoff.space.clone(),
                navigation_epoch: 5,
            }],
            spaces: vec![],
            active_space: handoff.space.clone(),
            active_tab: Some(7),
            overview: false,
        };
        let mut draft = original_draft();
        assert!(handoff.rebase(&mut draft, &snapshot));
        assert!(!draft.superseded(7, &handoff.url, 5, false));
        // The exception is consumed: a same-URL reload still replaces a draft.
        snapshot.tabs[0].navigation_epoch = 6;
        assert!(!handoff.rebase(&mut draft, &snapshot));
        assert!(draft.superseded(7, &handoff.url, 6, false));
        assert!(!handoff.rebase(&mut original_draft(), &snapshot));
        snapshot.tabs[0].navigation_epoch = 5;
        snapshot.tabs[0].stop_scope = Some(TaskScope {
            task: "task-1".into(),
            revision: 1,
        });
        assert!(!handoff.rebase(&mut original_draft(), &snapshot));
        snapshot.tabs[0].stop_scope = None;
        snapshot.tabs[0].url.push_str("/different");
        assert!(!handoff.rebase(&mut original_draft(), &snapshot));
        snapshot.tabs[0].url = handoff.url.clone();
        snapshot.active_tab = Some(8);
        assert!(!handoff.rebase(&mut original_draft(), &snapshot));
        snapshot.active_tab = Some(7);
        snapshot.active_space = "user".into();
        assert!(!handoff.rebase(&mut original_draft(), &snapshot));
        snapshot.active_space = handoff.space.clone();
        let mut stale = original_draft();
        stale.navigation_epoch -= 1;
        assert!(!handoff.rebase(&mut stale, &snapshot));
        snapshot.tabs.clear();
        assert!(!handoff.rebase(&mut original_draft(), &snapshot));
    }

    #[test]
    fn tab_drag_autoscroll_runs_only_inside_available_edge_bands() {
        let viewport = rect(0.0, 120.0, 200.0, 100.0);
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 121.0), viewport, 500.0),
            Some(108.0)
        );
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 219.0), viewport, 500.0),
            Some(132.0)
        );
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 144.0), viewport, 500.0),
            None
        );
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 195.0), viewport, 500.0),
            None
        );
        for point in [
            NSPoint::new(-1.0, 121.0),
            NSPoint::new(200.0, 219.0),
            NSPoint::new(10.0, 119.0),
            NSPoint::new(10.0, 220.0),
            NSPoint::new(f64::NAN, 121.0),
        ] {
            assert_eq!(tab_drag_scroll_origin(point, viewport, 500.0), None);
        }
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 1.0), rect(0.0, 0.0, 200.0, 100.0), 500.0),
            None
        );
        assert_eq!(
            tab_drag_scroll_origin(
                NSPoint::new(10.0, 499.0),
                rect(0.0, 400.0, 200.0, 100.0),
                500.0
            ),
            None
        );
        assert_eq!(
            tab_drag_scroll_origin(
                NSPoint::new(10.0, 99.0),
                rect(0.0, 0.0, 200.0, 100.0),
                100.0
            ),
            None
        );
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 219.0), viewport, f64::INFINITY),
            None
        );
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 1.0), rect(0.0, 0.0, 200.0, 0.0), 500.0),
            None
        );
    }

    #[test]
    fn tab_drag_autoscroll_stationary_pointer_reaches_bounds_and_can_reverse() {
        let mut viewport = rect(0.0, 0.0, 200.0, 100.0);
        let mut point = NSPoint::new(20.0, 99.0);
        let mut ticks = 0;
        while let Some(next) = tab_drag_scroll_origin(point, viewport, 401.0) {
            point.y += next - viewport.origin.y;
            viewport.origin.y = next;
            ticks += 1;
            assert!(ticks < 30);
        }
        assert_eq!(viewport.origin.y, 301.0);
        assert_eq!(point.y, 400.0);
        assert_eq!(ticks, 26);
        // Returning to the center stops; moving to the opposite edge restarts.
        point.y = viewport.origin.y + 50.0;
        assert_eq!(tab_drag_scroll_origin(point, viewport, 401.0), None);
        point.y = viewport.origin.y + 1.0;
        assert_eq!(tab_drag_scroll_origin(point, viewport, 401.0), Some(289.0));
        assert_eq!(
            tab_drag_scroll_origin(NSPoint::new(10.0, 19.0), rect(0.0, 0.0, 60.0, 20.0), 100.0),
            Some(12.0)
        );
    }

    #[test]
    fn tab_drag_autoscroll_accepts_only_the_constrained_vertical_delta() {
        let viewport = rect(0.0, 100.0, 200.0, 100.0);
        let bounds = viewport;
        let proposed = rect(0.0, 112.0, 200.0, 100.0);
        assert_eq!(
            tab_drag_scrolled_viewport(viewport, bounds, proposed, proposed),
            Some(proposed)
        );
        let clamped = rect(0.0, 105.5, 200.0, 100.0);
        assert_eq!(
            tab_drag_scrolled_viewport(viewport, bounds, proposed, clamped),
            Some(clamped)
        );
        assert_eq!(
            tab_drag_scrolled_viewport(viewport, bounds, proposed, bounds),
            Some(viewport)
        );
        let upward = rect(0.0, 88.0, 200.0, 100.0);
        assert_eq!(
            tab_drag_scrolled_viewport(viewport, bounds, upward, upward),
            Some(upward)
        );
        for constrained in [
            rect(1.0, 112.0, 200.0, 100.0),
            rect(0.0, 112.0, 199.0, 100.0),
            rect(0.0, 113.0, 200.0, 100.0),
            rect(0.0, 99.0, 200.0, 100.0),
            rect(0.0, f64::NAN, 200.0, 100.0),
        ] {
            assert!(tab_drag_scrolled_viewport(viewport, bounds, proposed, constrained).is_none());
        }
        assert!(tab_drag_scrolled_viewport(viewport, bounds, bounds, bounds).is_none());
        assert!(
            tab_drag_scrolled_viewport(viewport, bounds, rect(1.0, 112.0, 200.0, 100.0), proposed)
                .is_none()
        );
    }

    #[test]
    fn tab_drag_geometry_accepts_expected_scroll_but_not_other_native_changes() {
        let original = TabDragGeometry {
            window: rect(10.0, 20.0, 800.0, 600.0),
            scroll: rect(0.0, 0.0, 200.0, 100.0),
            document: rect(0.0, 0.0, 200.0, 500.0),
            source: rect(2.0, 4.0, 174.0, 36.0),
            viewport: rect(0.0, 0.0, 200.0, 100.0),
            clip_frame: rect(0.0, 0.0, 200.0, 100.0),
            clip_bounds: rect(0.0, 0.0, 200.0, 100.0),
        };
        let mut scrolled = original;
        scrolled.viewport.origin.y = 12.0;
        scrolled.clip_bounds.origin.y = 12.0;
        assert!(tab_drag_geometry_matches(
            &original,
            &scrolled,
            scrolled.viewport,
            scrolled.clip_bounds
        ));
        assert!(!tab_drag_geometry_matches(
            &original,
            &scrolled,
            original.viewport,
            original.clip_bounds
        ));
        for mutation in 0..7 {
            let mut changed = scrolled;
            match mutation {
                0 => changed.viewport.origin.x += 1.0,
                1 => changed.viewport.size.height -= 1.0,
                2 => changed.source.origin.y += 1.0,
                3 => changed.document.size.height += 1.0,
                4 => changed.window.origin.x += 1.0,
                5 => changed.clip_frame.size.width -= 1.0,
                _ => changed.clip_bounds.origin.x += 1.0,
            }
            assert!(!tab_drag_geometry_matches(
                &original,
                &changed,
                scrolled.viewport,
                scrolled.clip_bounds
            ));
        }
        // Even a forged expected value cannot authorize horizontal/size changes.
        let horizontal = rect(1.0, 12.0, 200.0, 100.0);
        assert!(!tab_drag_geometry_matches(
            &original, &scrolled, horizontal, horizontal
        ));
    }

    #[test]
    fn tab_drag_threshold_preserves_small_click_motion() {
        let origin = NSPoint::new(12.0, 20.0);
        for point in [origin, NSPoint::new(13.0, 20.0), NSPoint::new(15.0, 24.0)] {
            assert!(!tab_drag_threshold(origin, point));
        }
        assert!(tab_drag_threshold(origin, NSPoint::new(12.0, 26.0)));
        assert!(tab_drag_threshold(origin, NSPoint::new(6.0, 20.0)));
        assert!(!tab_drag_threshold(origin, NSPoint::new(f64::NAN, 20.0)));
        assert!(!tab_drag_threshold(
            origin,
            NSPoint::new(12.0, f64::INFINITY)
        ));
    }

    #[test]
    fn tab_drag_drop_uses_row_midpoints_and_never_anchors_to_source() {
        let order = [7, 8, 9];
        let frame = |id| {
            let index = order.iter().position(|candidate| *candidate == id)?;
            Some(tab_row_frames(200.0, false, 4.0 + index as f64 * TAB_ROW_HEIGHT).selection)
        };
        let viewport = rect(0.0, 0.0, 200.0, 180.0);
        for (source, y, expected) in [
            (8, 4.0, (Some(7), 3.0)),
            (8, 49.0, (Some(9), 79.0)),
            (8, 110.0, (None, 117.0)),
            (7, 49.0, (Some(8), 41.0)),
            (9, 4.0, (Some(7), 3.0)),
        ] {
            let drop = tab_drag_drop(&order, source, NSPoint::new(10.0, y), viewport, frame);
            assert_eq!(drop, Some(expected));
            assert_ne!(drop.unwrap().0, Some(source));
        }
        assert!(tab_drag_drop(&order, 99, NSPoint::new(10.0, 4.0), viewport, frame).is_none());
        assert!(tab_drag_drop(&[7], 7, NSPoint::new(10.0, 4.0), viewport, frame).is_none());
        assert!(tab_drag_drop(&order, 8, NSPoint::new(10.0, 4.0), viewport, |_| None).is_none());
    }

    #[test]
    fn tab_drag_drop_rejects_outside_or_invalid_viewport_coordinates() {
        let order = [7, 8, 9];
        let frame = |id| {
            let index = order.iter().position(|candidate| *candidate == id)?;
            Some(tab_row_frames(60.0, true, 4.0 + index as f64 * TAB_ROW_HEIGHT).selection)
        };
        let viewport = rect(0.0, 38.0, 60.0, 76.0);
        assert_eq!(
            tab_drag_drop(&order, 7, NSPoint::new(12.0, 50.0), viewport, frame),
            Some((Some(8), 41.0))
        );
        for point in [
            NSPoint::new(-1.0, 50.0),
            NSPoint::new(60.0, 50.0),
            NSPoint::new(12.0, 37.0),
            NSPoint::new(12.0, 114.0),
            NSPoint::new(f64::NAN, 50.0),
        ] {
            assert!(tab_drag_drop(&order, 7, point, viewport, frame).is_none());
        }
        assert!(
            tab_drag_drop(
                &order,
                7,
                NSPoint::new(12.0, 50.0),
                rect(0.0, 0.0, 0.0, 100.0),
                frame
            )
            .is_none()
        );
    }

    #[test]
    fn tab_drag_target_keeps_background_selection_and_rejects_changed_order() {
        let mut tabs = vec![
            numbered_tab_fixture(7, "user"),
            numbered_tab_fixture(8, "user"),
            numbered_tab_fixture(9, "other"),
        ];
        let target = crate::browser::TabDragTarget {
            tab: 8,
            space: "user".into(),
            selected: Some(7),
            order: vec![7, 8],
        };
        assert!(tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            false,
            false
        ));
        tabs[0].loading = true;
        tabs[1].title = "Updated while dragging".into();
        assert!(tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            false,
            false
        ));
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(8),
            false,
            false
        ));
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "other",
            Some(7),
            false,
            false
        ));
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            true,
            false
        ));
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            false,
            true
        ));
        tabs.swap(0, 1);
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            false,
            false
        ));
        tabs.swap(0, 1);
        tabs[1].space = "other".into();
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            false,
            false
        ));
        tabs.pop();
        tabs.pop();
        assert!(!tab_drag_target_matches(
            &target,
            &tabs,
            "user",
            Some(7),
            false,
            false
        ));
    }

    #[test]
    fn tab_row_actions_reject_removed_moved_and_overlay_hidden_tabs() {
        let mut tabs = vec![ShellTab {
            id: 7,
            title: "같은 제목".into(),
            url: "https://example.invalid/one".into(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_muted: None,
            crashed: false,
            failure: None,
            start_page: false,
            task_status: String::new(),
            dialog_pending: false,
            dialog_user: false,
            stop_scope: None,
            space: "user".into(),
            navigation_epoch: 1,
        }];
        assert!(tab_row_is_current(7, "user", "user", false, &tabs));
        assert!(!tab_row_is_current(8, "user", "user", false, &tabs));
        assert!(!tab_row_is_current(7, "user", "task-1", false, &tabs));
        assert!(!tab_row_is_current(7, "user", "user", true, &tabs));
        tabs[0].space = "task-1".into();
        assert!(!tab_row_is_current(7, "user", "user", false, &tabs));
        assert!(!tab_row_is_current(7, "user", "task-1", false, &tabs));
        assert!(tab_row_is_current(7, "task-1", "task-1", false, &tabs));
        tabs.clear();
        assert!(!tab_row_is_current(7, "task-1", "task-1", false, &tabs));
    }

    #[test]
    fn tab_row_layout_separates_close_and_selection_in_both_sidebar_modes() {
        // Collapsed widths include both overlay and legacy vertical scrollers.
        for (width, collapsed) in [(45.0, true), (60.0, true), (185.0, false), (200.0, false)] {
            let frames = tab_row_frames(width, collapsed, 40.0);
            assert!(frames.selection.size.width >= 19.0);
            assert!(frames.close.size.width >= 19.0);
            assert!(
                frames.selection.origin.x + frames.selection.size.width < frames.close.origin.x
            );
            assert!(frames.close.origin.x + frames.close.size.width <= width - 2.0);
            assert!(frames.icon.origin.x >= frames.selection.origin.x);
            assert!(
                frames.icon.origin.x + frames.icon.size.width
                    <= frames.selection.origin.x + frames.selection.size.width
            );
            if !collapsed {
                assert!(
                    frames.selection.origin.x + frames.selection.size.width < frames.mute.origin.x
                );
                assert!(frames.mute.origin.x + frames.mute.size.width < frames.close.origin.x);
                assert!(frames.mute.size.width >= 19.0);
                assert!(frames.title.size.width > 0.0);
                assert!(frames.title.origin.x + frames.title.size.width < frames.mute.origin.x);
            } else {
                assert_eq!(frames.mute.size.width, 0.0);
            }
        }
        let narrow = tab_row_frames(1.0, true, 0.0);
        assert_eq!(narrow.selection.size.width, 0.0);
        assert_eq!(narrow.close.size.width, 0.0);
    }

    #[test]
    fn search_query_keys_preserve_focused_buttons_and_other_native_keys() {
        for (key, expected) in [
            (36, SearchQueryKey::Submit),
            (76, SearchQueryKey::Submit),
            (125, SearchQueryKey::Move(true)),
            (126, SearchQueryKey::Move(false)),
        ] {
            assert_eq!(search_query_key(key, true, false), Some(expected));
            // Close, mode, history controls and result buttons keep AppKit keys.
            assert_eq!(search_query_key(key, false, false), None);
            assert_eq!(search_query_key(key, false, true), None);
        }
        // Escape keeps its separate overlay cancellation path; text and Tab
        // must never become selection commands, even while confirming deletion.
        for key in [0, 48, 49, 53, 123, 124] {
            for confirm in [false, true] {
                assert_eq!(search_query_key(key, true, confirm), None);
            }
        }
    }

    #[test]
    fn history_confirmation_consumes_query_submission_and_arrows_without_an_action() {
        for key in [36, 76, 125, 126] {
            assert_eq!(
                search_query_key(key, true, true),
                Some(SearchQueryKey::Consume)
            );
        }
        assert_eq!(
            search_query_key(36, true, false),
            Some(SearchQueryKey::Submit)
        );
    }

    #[test]
    fn search_submission_selector_cannot_bypass_confirmation_or_a_closed_overlay() {
        let selected = SearchResultId::History(7);
        let mut state = SearchState {
            open: true,
            mode: SearchMode::History,
            selected: Some(selected),
            ..SearchState::default()
        };
        assert_eq!(search_submission(&state), Some(selected));
        state.confirm_clear = true;
        assert_eq!(search_submission(&state), None);
        assert_eq!(state.selected, Some(selected));
        state.confirm_clear = false;
        assert_eq!(search_submission(&state), Some(selected));
        state.open = false;
        assert_eq!(search_submission(&state), None);
        state.open = true;
        state.selected = None;
        assert_eq!(search_submission(&state), None);
    }

    #[test]
    fn query_submission_cannot_use_an_old_revision_or_a_truncated_input() {
        let mut state = SearchState::default();
        state.open(SearchMode::All);
        state.set_query("query to open");
        let original = SearchResultId::Query(state.revision);
        state.selected = Some(original);
        assert_eq!(search_submission(&state), Some(original));
        state.set_query("different query");
        state.selected = Some(original); // delayed selector from an old row
        assert_eq!(search_submission(&state), None);
        state.set_query("query to open");
        state.selected = Some(original);
        assert_eq!(search_submission(&state), None);
        state.set_query(&"a".repeat(tab_search::MAX_QUERY_CHARS + 1));
        state.selected = Some(SearchResultId::Query(state.revision));
        assert_eq!(search_submission(&state), None);
        state.set_query("https://example.test/");
        state.selected = Some(SearchResultId::Query(state.revision));
        state.set_mode(SearchMode::History);
        state.selected = Some(SearchResultId::Query(state.revision));
        assert_eq!(search_submission(&state), None);
    }

    #[test]
    fn query_open_guard_preserves_selection_and_rejects_changed_or_reopened_overlays() {
        let mut state = SearchState::default();
        state.open(SearchMode::All);
        state.set_query("explicit query");
        // Clicking the last row is allowed while an internal result is selected.
        state.selected = Some(SearchResultId::Tab(7));
        let target = search_query_target(&state, state.revision).unwrap();
        assert!(search_query_target_matches(&state, &target));
        state.selected = Some(SearchResultId::Query(state.revision));
        assert!(!search_query_target_matches(&state, &target));
        state.selected = target.selected;
        state.confirm_clear = true;
        assert!(!search_query_target_matches(&state, &target));
        assert!(search_query_target(&state, state.revision).is_none());
        state.confirm_clear = false;
        state.set_mode(SearchMode::History);
        state.set_mode(SearchMode::All);
        state.selected = target.selected;
        assert!(!search_query_target_matches(&state, &target));
        let before_close = search_query_target(&state, state.revision).unwrap();
        state.close();
        state.open(SearchMode::All);
        state.set_query(&before_close.query);
        state.selected = before_close.selected;
        assert!(!search_query_target_matches(&state, &before_close));
        assert!(search_query_target(&state, before_close.revision).is_none());
    }

    #[test]
    fn explicit_query_selection_survives_staging_replacing_an_exact_history_match() {
        use SearchResultId::{History, Query, Tab};
        let url = "https://example.test/exact";
        let mut state = SearchState::default();
        state.open(SearchMode::All);
        state.set_query(url);
        let mut original = tab_search::combined_results([], [], [(7, url)]);
        tab_search::append_query_result(&mut original, state.revision);
        state.reconcile(&original);
        assert_eq!(state.selected, Some(History(7)));

        let captured = search_query_target(&state, state.revision).unwrap();
        let target = select_query_submission(&mut state, captured).unwrap();
        assert_eq!(state.selected, Some(Query(state.revision)));
        // on_after_created -> attach -> sync exposes the staging tab. URL
        // deduplication removes the previously selected History result.
        let mut staging = tab_search::combined_results([(11, url)], [], [(7, url)]);
        tab_search::append_query_result(&mut staging, state.revision);
        assert_eq!(staging, [Tab(11), Query(state.revision)]);
        state.reconcile(&staging);
        assert!(search_query_target_matches(&state, &target));

        // A failed staging cleanup restores History; the clicked Query remains
        // selected, allowing the same overlay to display its failure notice.
        state.reconcile(&original);
        assert!(search_query_target_matches(&state, &target));
        state.move_selection(&original, false);
        assert_eq!(state.selected, Some(History(7)));
        assert!(!search_query_target_matches(&state, &target));
        assert!(select_query_submission(&mut state, target).is_none());
    }

    #[test]
    fn command_shortcuts_share_translated_keys_without_exempting_paste() {
        let command = NSEventModifierFlags::Command;
        let shift = NSEventModifierFlags::Shift;
        for (translated, original, modifiers, expected_menu, expected_reopen) in [
            ("A", "ㅁ", command | shift, false, false),
            ("B", "ㅠ", command | shift, true, false),
            ("S", "ㄴ", command | shift, true, false),
            ("T", "ㅅ", command | shift, false, true),
            ("l", "ㅣ", command, false, false),
            ("d", "ㅇ", command, false, false),
            ("v", "ㅍ", command, false, false),
        ] {
            let key = resolve_shortcut_key(
                modifiers,
                |applied| {
                    assert_eq!(applied, modifiers);
                    Some(translated.into())
                },
                || panic!("Available Command translation must take precedence over {original}"),
            )
            .unwrap();
            assert_eq!(key, translated.to_lowercase());
            assert_eq!(is_menu_only_shortcut(&key, modifiers), expected_menu);
            assert_eq!(is_reopen_shortcut(&key, modifiers), expected_reopen);
        }
    }

    #[test]
    fn ordinary_input_ctrl_tab_and_modified_commands_keep_original_keys() {
        let command = NSEventModifierFlags::Command;
        let control = NSEventModifierFlags::Control;
        let option = NSEventModifierFlags::Option;
        let shift = NSEventModifierFlags::Shift;
        for (modifiers, original) in [
            (NSEventModifierFlags::empty(), "ㅁ"),
            (shift, "ㅆ"),
            (control, "\t"),
            (control | shift, "\u{19}"),
            (command | control, "ㅍ"),
            (command | option, "ㅍ"),
            (command | control | option | shift, "ㅍ"),
        ] {
            assert_eq!(
                resolve_shortcut_key(
                    modifiers,
                    |_| panic!("This input must not request Command translation"),
                    || Some(original.into()),
                )
                .as_deref(),
                Some(original),
            );
        }
    }

    #[test]
    fn missing_command_translation_uses_the_original_key_fallback() {
        let command = NSEventModifierFlags::Command;
        assert_eq!(
            resolve_shortcut_key(command, |_| None, || Some("L".into())).as_deref(),
            Some("l"),
        );
        assert_eq!(
            resolve_shortcut_key(command, |_| None, || Some("ㅁ".into())).as_deref(),
            Some("ㅁ"),
        );
        assert_eq!(resolve_shortcut_key(command, |_| None, || None), None);
    }

    #[test]
    fn reorder_menu_uses_visible_order_without_wrap_or_hidden_space_targets() {
        let mut tabs = vec![
            numbered_tab_fixture(7, "user"),
            numbered_tab_fixture(20, "task-a"),
            numbered_tab_fixture(3, "user"),
            numbered_tab_fixture(9, "user"),
        ];
        let target = |tabs: &[ShellTab], active, forward| {
            tab_order_target(tabs, "user", active, false, false, forward)
        };
        assert_eq!(target(&tabs, Some(3), false).unwrap().neighbor, 7);
        assert_eq!(target(&tabs, Some(3), true).unwrap().neighbor, 9);
        assert!(target(&tabs, Some(7), false).is_none());
        assert!(target(&tabs, Some(9), true).is_none());
        assert!(target(&tabs, None, true).is_none());
        assert!(target(&tabs, Some(20), true).is_none());
        for (overview, overlay) in [(true, false), (false, true)] {
            assert!(tab_order_target(&tabs, "user", Some(3), overview, overlay, true).is_none());
        }
        let old = target(&tabs, Some(3), true).unwrap();
        tabs.swap(2, 3);
        assert_ne!(target(&tabs, Some(3), true).as_ref(), Some(&old));
        assert_eq!(
            numbered_tab_target(&tabs, "user", 2, Some(3), false, false),
            Some(9)
        );
        assert_eq!(cycle_tab_target(&tabs, "user", Some(3), false), Some(9));
        // Closing the only peer cannot turn a one-tab space into a wrap move.
        tabs.retain(|tab| tab.id == 3 || tab.space != "user");
        assert!(target(&tabs, Some(3), false).is_none());
        assert!(target(&tabs, Some(3), true).is_none());
    }

    #[test]
    fn cycle_tabs_wrap_in_sidebar_order_without_crossing_spaces_or_limiting_to_nine() {
        let ids: Vec<_> = (0..12).map(|index| 100 - index * 3).collect();
        let mut tabs: Vec<_> = ids
            .iter()
            .map(|&id| numbered_tab_fixture(id, "user"))
            .collect();
        tabs.insert(3, numbered_tab_fixture(501, "task-a"));
        tabs.insert(8, numbered_tab_fixture(702, "task-b"));
        tabs.push(numbered_tab_fixture(506, "task-a"));
        for (index, &active) in ids.iter().enumerate() {
            assert_eq!(
                cycle_tab_target(&tabs, "user", Some(active), true),
                Some(ids[(index + 1) % ids.len()])
            );
            assert_eq!(
                cycle_tab_target(&tabs, "user", Some(active), false),
                Some(ids[(index + ids.len() - 1) % ids.len()])
            );
        }
        for forward in [false, true] {
            assert_eq!(
                cycle_tab_target(&tabs, "task-a", Some(501), forward),
                Some(506)
            );
            assert_eq!(
                cycle_tab_target(&tabs, "task-a", Some(506), forward),
                Some(501)
            );
            assert_eq!(cycle_tab_target(&tabs, "task-b", Some(702), forward), None);
            assert_eq!(cycle_tab_target(&tabs, "user", Some(501), forward), None);
        }
    }

    #[test]
    fn cycle_tabs_do_not_reselect_a_single_tab_or_guess_a_missing_selection() {
        let tabs = [numbered_tab_fixture(4, "user")];
        for forward in [false, true] {
            assert_eq!(cycle_tab_target(&tabs, "user", Some(4), forward), None);
            assert_eq!(cycle_tab_target(&tabs, "user", Some(9), forward), None);
            assert_eq!(cycle_tab_target(&tabs, "user", None, forward), None);
            assert_eq!(cycle_tab_target(&tabs, "empty", Some(4), forward), None);
            assert_eq!(cycle_tab_target(&[], "user", None, forward), None);
        }
        // Number selection can deliberately reveal its only tab; cycling cannot.
        assert_eq!(
            numbered_tab_target(&tabs, "user", 1, Some(4), false, true),
            Some(4)
        );
        assert_eq!(
            numbered_tab_target(&tabs, "user", 1, Some(4), true, false),
            Some(4)
        );
    }

    #[test]
    fn cycle_tab_chords_keep_existing_aliases_and_reject_extra_modifiers() {
        let command = NSEventModifierFlags::Command;
        let control = NSEventModifierFlags::Control;
        let shift = NSEventModifierFlags::Shift;
        let option = NSEventModifierFlags::Option;
        let state = NSEventModifierFlags::CapsLock | NSEventModifierFlags::NumericPad;
        for key in ["\t", "\u{19}"] {
            assert_eq!(cycle_tab_shortcut(key, control | state), Some(true));
            assert_eq!(
                cycle_tab_shortcut(key, control | shift | state),
                Some(false)
            );
            for extra in [command, option, command | option] {
                assert_eq!(cycle_tab_shortcut(key, control | extra), None);
                assert_eq!(cycle_tab_shortcut(key, control | shift | extra), None);
            }
        }
        for (key, forward) in [("[", false), ("]", true), ("{", false), ("}", true)] {
            assert_eq!(
                cycle_tab_shortcut(key, command | shift | state),
                Some(forward)
            );
            for extra in [control, option, control | option] {
                assert_eq!(cycle_tab_shortcut(key, command | shift | extra), None);
            }
        }
        // Preserve the existing translated-brace aliases; plain brackets remain history navigation.
        assert_eq!(cycle_tab_shortcut("{", command), Some(false));
        assert_eq!(cycle_tab_shortcut("}", command), Some(true));
        for (key, modifiers) in [
            ("[", command),
            ("]", command),
            ("\t", command),
            ("\t", shift),
            ("\t", option),
            ("[", control),
            ("1", command),
            ("", command | shift),
        ] {
            assert_eq!(cycle_tab_shortcut(key, modifiers), None);
        }
    }

    #[test]
    fn numbered_tabs_follow_current_space_order_and_nine_always_means_last() {
        let tabs = [
            (501, "task-a"),
            (40, "user"),
            (702, "task-b"),
            (13, "user"),
            (506, "task-a"),
            (12, "user"),
            (90, "user"),
            (20, "user"),
            (33, "user"),
            (88, "user"),
            (16, "user"),
            (18, "user"),
            (7, "user"),
        ]
        .map(|(id, space)| numbered_tab_fixture(id, space));
        for (space, number, expected) in [
            ("user", 1, Some(40)),
            ("user", 2, Some(13)),
            ("user", 8, Some(16)),
            ("user", 9, Some(7)),
            ("task-a", 1, Some(501)),
            ("task-a", 2, Some(506)),
            ("task-a", 9, Some(506)),
            ("task-b", 1, Some(702)),
            ("task-b", 2, None),
            ("empty", 1, None),
            ("empty", 9, None),
            ("user", 0, None),
            ("user", 10, None),
        ] {
            assert_eq!(
                numbered_tab_target(&tabs, space, number, None, false, false),
                expected,
                "{space} shortcut {number}",
            );
        }
        assert_eq!(numbered_tab_target(&[], "user", 9, None, true, true), None);
    }

    #[test]
    fn numbered_tab_chords_reject_extra_command_modifiers_and_keep_translation() {
        let command = NSEventModifierFlags::Command;
        let extra_state = NSEventModifierFlags::CapsLock | NSEventModifierFlags::NumericPad;
        let key = resolve_shortcut_key(
            command | extra_state,
            |applied| {
                assert_eq!(applied, command);
                Some("9".into())
            },
            || panic!("The translated Command key is available"),
        )
        .unwrap();
        assert_eq!(numbered_tab_shortcut(&key, command | extra_state), Some(9));
        assert_eq!(numbered_tab_shortcut("1", command), Some(1));
        assert_eq!(numbered_tab_shortcut("8", command), Some(8));
        for extra in [
            NSEventModifierFlags::Shift,
            NSEventModifierFlags::Control,
            NSEventModifierFlags::Option,
        ] {
            assert_eq!(numbered_tab_shortcut("1", command | extra), None);
        }
        for modifiers in [NSEventModifierFlags::empty(), extra_state] {
            assert_eq!(numbered_tab_shortcut("1", modifiers), None);
        }
        for key in ["0", "10", "", "１", "!", "t"] {
            assert_eq!(numbered_tab_shortcut(key, command), None);
        }
    }

    #[test]
    fn numbered_tab_noop_preserves_same_page_unless_a_surface_needs_dismissal() {
        let tabs = [
            numbered_tab_fixture(4, "user"),
            numbered_tab_fixture(9, "user"),
        ];
        assert_eq!(
            numbered_tab_target(&tabs, "user", 1, Some(4), false, false),
            None
        );
        for (overview, overlay) in [(true, false), (false, true), (true, true)] {
            assert_eq!(
                numbered_tab_target(&tabs, "user", 1, Some(4), overview, overlay),
                Some(4)
            );
            assert_eq!(
                numbered_tab_target(&tabs, "user", 3, Some(4), overview, overlay),
                None
            );
        }
        assert_eq!(
            numbered_tab_target(&tabs, "user", 2, Some(4), false, false),
            Some(9)
        );
        assert_eq!(
            numbered_tab_target(&tabs, "user", 9, Some(9), false, false),
            None
        );
    }

    #[test]
    fn reopen_shortcut_does_not_alias_new_tab_or_modified_page_commands() {
        let command = NSEventModifierFlags::Command;
        let shift = NSEventModifierFlags::Shift;
        assert!(is_reopen_shortcut("t", command | shift));
        assert!(!is_reopen_shortcut("t", command));
        assert!(!is_reopen_shortcut("t", shift));
        assert!(!is_reopen_shortcut(
            "t",
            command | shift | NSEventModifierFlags::Option
        ));
        assert!(!is_reopen_shortcut(
            "t",
            command | shift | NSEventModifierFlags::Control
        ));
        assert!(!is_reopen_shortcut("w", command | shift));
    }
    #[test]
    fn page_find_results_cannot_reopen_or_replace_a_newer_query() {
        let mut state = PageFindState {
            open: true,
            target: Some((4, 7)),
            query: "first".into(),
            ..PageFindState::default()
        };
        let first = state.request().unwrap();
        state.query = "second".into();
        let second = state.request().unwrap();
        assert!(!state.receive(4, 7, first, 12, 1, true));
        assert!(!state.receive(4, 8, second, 12, 1, true));
        assert!(!state.receive(5, 7, second, 12, 1, true));
        assert!(state.receive(4, 7, second, 3, 2, true));
        assert_eq!(state.close(true), Some((4, second)));
        assert!(!state.receive(4, 7, second, 4, 3, true));
        assert_eq!(state.query, "second");
        state.open = true;
        let reopened = state.request().unwrap();
        assert!(reopened > second);
        assert!(!state.receive(4, 7, second, 4, 3, true));
        assert!(state.receive(4, 7, reopened, 3, 1, true));
        state.close(false);
        assert!(state.query.is_empty());
        assert_eq!(state.target, None);
        assert_eq!(state.token, None);
    }

    #[test]
    fn page_find_partial_unknown_counts_do_not_mean_no_matches() {
        let mut state = PageFindState {
            open: true,
            target: Some((2, 1)),
            query: "한글".into(),
            ..PageFindState::default()
        };
        let token = state.request().unwrap();
        assert!(state.receive(2, 1, token, -1, -1, false));
        assert_eq!(state.count, None);
        assert_eq!(state.ordinal, None);
        assert!(!state.final_update);
        assert!(state.receive(2, 1, token, 5, 2, false));
        assert!(state.receive(2, 1, token, -1, -1, true));
        assert_eq!(state.count, Some(5));
        assert_eq!(state.ordinal, Some(2));
        assert!(state.final_update);
    }

    #[test]
    fn bookmark_completion_cannot_close_a_reopened_editor() {
        let mut state = BookmarkState {
            open: true,
            ..BookmarkState::default()
        };
        let first = state.begin(BookmarkOperation::Save).unwrap();
        state.dismiss_editor();
        assert!(state.take_completion(first).is_none());
        let second = state.begin(BookmarkOperation::Prepare).unwrap();
        assert!(second > first);
        assert!(state.take_completion(first).is_none());
        assert!(matches!(
            state.take_completion(second),
            Some(BookmarkOperation::Prepare)
        ));
        assert!(state.take_completion(second).is_none());
        let third = state.begin(BookmarkOperation::Remove).unwrap();
        state.open = false;
        assert!(state.take_completion(third).is_none());
    }

    #[test]
    fn bookmark_failed_save_keeps_fixed_draft() {
        let mut state = BookmarkState {
            open: true,
            ..BookmarkState::default()
        };
        state.draft = Some(crate::bookmark_runtime::Draft {
            id: Some(7),
            url: "https://example.com/original".into(),
            title: "사용자가 정한 이름".into(),
        });
        let request = state.begin(BookmarkOperation::Save).unwrap();
        assert!(matches!(
            state.take_completion(request),
            Some(BookmarkOperation::Save)
        ));
        let draft = state.draft.as_ref().unwrap();
        assert_eq!(draft.id, Some(7));
        assert_eq!(draft.url, "https://example.com/original");
        assert_eq!(draft.title, "사용자가 정한 이름");
    }

    #[test]
    fn failed_address_yields_to_later_navigation_only_after_editing() {
        let draft = AddressDraft {
            tab: 2,
            url: "https://example.com/first".into(),
            navigation_epoch: 7,
        };
        assert!(!draft.superseded(2, &draft.url, 7, false));
        assert!(!draft.superseded(2, "https://example.com/next", 8, true));
        assert!(draft.superseded(2, "https://example.com/next", 8, false));
        assert!(draft.superseded(2, &draft.url, 8, false));
        assert!(draft.superseded(2, "https://example.com/first#section", 7, false));
        assert!(!draft.superseded(3, "https://example.com/next", 8, false));
    }

    #[test]
    fn page_edit_menu_actions_match_keyboard_editing_scope() {
        for action in [
            sel!(undo:),
            sel!(redo:),
            sel!(cut:),
            sel!(copy:),
            sel!(paste:),
            sel!(selectAll:),
        ] {
            assert!(is_page_edit_action(action));
        }
        for action in [
            sel!(showSpaces:),
            sel!(toggleSidebar:),
            sel!(focusAddress:),
            sel!(openCurrentPageInUserTab:),
            sel!(nextTab:),
            sel!(previousTab:),
            sel!(showHistory:),
            sel!(toggleSearchMode:),
            sel!(prepareClearHistory:),
            sel!(confirmClearHistory:),
            sel!(cancelClearHistory:),
            sel!(resetCurrentSiteDownloads:),
            sel!(revealDownloadSource:),
            sel!(revealPermissionSource:),
            sel!(toggleTabAudio:),
            sel!(toggleTabRowAudio:),
            sel!(terminate:),
        ] {
            assert!(!is_page_edit_action(action));
        }
    }

    #[test]
    fn context_menu_requires_the_same_visible_page_without_native_surfaces() {
        let mut tabs = vec![
            numbered_tab_fixture(7, "user"),
            numbered_tab_fixture(9, "task-a"),
        ];
        assert!(context_menu_page(
            &tabs,
            "task-a",
            Some(9),
            false,
            false,
            false,
            9
        ));
        for (space, active, overview, overlay, start, tab) in [
            ("user", Some(7), false, false, false, 9),
            ("user", Some(9), false, false, false, 9),
            ("task-a", None, false, false, false, 9),
            ("task-a", Some(9), true, false, false, 9),
            ("task-a", Some(9), false, true, false, 9),
            ("task-a", Some(9), false, false, true, 9),
            ("task-a", Some(9), false, false, false, 99),
        ] {
            assert!(!context_menu_page(
                &tabs, space, active, overview, overlay, start, tab
            ));
        }
        tabs[1].crashed = true;
        assert!(!context_menu_page(
            &tabs,
            "task-a",
            Some(9),
            false,
            false,
            false,
            9
        ));
    }

    #[test]
    fn page_input_requires_intent_and_nonzero_scroll() {
        for kind in [
            NSEventType::LeftMouseDown,
            NSEventType::RightMouseDown,
            NSEventType::OtherMouseDown,
            NSEventType::KeyDown,
        ] {
            assert!(is_page_input_type(kind, 0.0, 0.0));
        }
        assert!(is_page_input_type(NSEventType::ScrollWheel, 1.0, 0.0));
        assert!(is_page_input_type(NSEventType::ScrollWheel, 0.0, -1.0));
        assert!(!is_page_input_type(NSEventType::ScrollWheel, 0.0, 0.0));
        for kind in [
            NSEventType::MouseMoved,
            NSEventType::MouseEntered,
            NSEventType::MouseExited,
            NSEventType::FlagsChanged,
            NSEventType::KeyUp,
            NSEventType::LeftMouseUp,
        ] {
            assert!(!is_page_input_type(kind, 1.0, 1.0));
        }
    }

    #[test]
    fn menu_only_shortcuts_do_not_hide_page_editing_shortcuts() {
        let command = NSEventModifierFlags::Command;
        let shift = command | NSEventModifierFlags::Shift;
        assert!(is_menu_only_shortcut("b", shift));
        assert!(is_menu_only_shortcut("s", shift));
        assert!(is_menu_only_shortcut("q", command));
        assert!(!is_menu_only_shortcut("b", command));
        assert!(!is_menu_only_shortcut("s", command));
        assert!(!is_menu_only_shortcut("q", shift));
        assert!(!is_menu_only_shortcut(
            "b",
            shift | NSEventModifierFlags::Option
        ));
        for key in ["z", "x", "c", "v", "a"] {
            assert!(!is_menu_only_shortcut(key, command));
            assert!(!is_menu_only_shortcut(key, shift));
        }
    }

    #[test]
    fn empty_active_task_space_keeps_a_revision_bound_stop_action() {
        let mut space = crate::spaces::SpaceView {
            id: "task-empty".into(),
            name: "빈 작업".into(),
            kind: "task".into(),
            status: "active".into(),
            revision: 7,
            tabs: Vec::new(),
            selected_tab: None,
            active: true,
        };
        assert_eq!(
            space_stop_scope(&space),
            Some(TaskScope {
                task: "task-empty".into(),
                revision: 7
            })
        );
        space.status = "paused".into();
        assert!(space_stop_scope(&space).is_none());
        space.status = "active".into();
        space.kind = "user".into();
        assert!(space_stop_scope(&space).is_none());
    }
    #[test]
    fn previews_reject_large_or_invalid_images_before_decode() {
        let mut png = vec![0; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&640u32.to_be_bytes());
        png[20..24].copy_from_slice(&480u32.to_be_bytes());
        assert!(bounded_preview_png(&png));
        png[16..20].copy_from_slice(&641u32.to_be_bytes());
        assert!(!bounded_preview_png(&png));
        png[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert!(!bounded_preview_png(&png));
        png[16..20].copy_from_slice(&320u32.to_be_bytes());
        png.resize(PREVIEW_BYTES_LIMIT + 1, 0);
        assert!(!bounded_preview_png(&png));
        assert!(!bounded_preview_png(b"not an image"));
    }
    #[test]
    fn display_text_is_bounded_without_splitting_unicode() {
        assert_eq!(compact("한글\n제목", 3), "한글 …");
        assert_eq!(compact("짧은 제목", 20), "짧은 제목");
    }
}
