//! One native AppKit browser window. Controls never create a web renderer.

use crate::protocol::TaskScope;
use crate::tab_search::{self, SearchResultId, SearchState, SearchTab};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApp, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBackingStoreType, NSButton, NSCellImagePosition, NSColor, NSEvent, NSEventModifierFlags,
    NSEventType, NSFont, NSFontWeightMedium, NSImage, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSImageView, NSMenu, NSMenuItem, NSScrollView, NSTextField, NSView,
    NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSData, NSNotification, NSObject, NSPoint, NSRect, NSSize, NSString,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ffi::c_void,
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
    pub crashed: bool,
    pub task_status: String,
    pub stop_scope: Option<TaskScope>,
    pub space: String,
    pub navigation_epoch: u64,
}

pub enum ShellAction {
    Back(i32),
    Forward(i32),
    Reload(i32),
    Navigate(i32, String),
    NewTab,
    CloseTab(i32),
    Stop(i32, TaskScope),
    SelectTab(i32),
    SelectSearchTab(i32),
    SelectSpace(String),
    ShowOverview(bool),
    StopSpace(TaskScope),
}

const PREVIEW_LIMIT: usize = 8;
const PREVIEW_BYTES_LIMIT: usize = 1024 * 1024;
const SIDEBAR_WIDTH: f64 = 216.0;
const RAIL_WIDTH: f64 = 56.0;
const TOOLBAR_HEIGHT: f64 = 44.0;
const FIND_BAR_HEIGHT: f64 = 44.0;
const TAB_ROW_HEIGHT: f64 = 36.0;

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
    button: Retained<NSButton>,
    icon: Retained<NSImageView>,
    title: Retained<NSTextField>,
}

struct SearchRow {
    id: SearchResultId,
    button: Retained<NSButton>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
}

struct AddressDraft {
    tab: i32,
    url: String,
    navigation_epoch: u64,
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
    surface: Retained<SurfaceView>,
    name: Retained<NSTextField>,
    source: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    choose: Retained<NSButton>,
    cancel: Retained<NSButton>,
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
    rows: Vec<DownloadRow>,
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
    bookmarks_button: Retained<NSButton>,
    bookmarks: BookmarkPanel,
    downloads_button: Retained<NSButton>,
    downloads: DownloadPanel,
    download_activity: (usize, usize),
    find: FindBar,
    search_surface: Retained<SurfaceView>,
    search_title: Retained<NSTextField>,
    search_hint: Retained<NSTextField>,
    search_field: Retained<NSTextField>,
    search_close: Retained<NSButton>,
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
    status: Retained<NSTextField>,
    status_surface: Retained<SurfaceView>,
    stop: Retained<NSButton>,
    tab_buttons: Vec<TabRow>,
    selected_tab_background: Retained<SurfaceView>,
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
    card_signature: String,
    displayed_address: Option<i32>,
    address_draft: Option<AddressDraft>,
    notice: Option<String>,
    // AppKit target/delegate properties are weak. Drop this last.
    target: Retained<ShellTarget>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    // This fixed symbol vocabulary is shared by controls. It never grows with tabs/tasks.
    static SYMBOLS: RefCell<BTreeMap<&'static str, Retained<NSImage>>> = const { RefCell::new(BTreeMap::new()) };
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
}

struct SurfaceStyle {
    kind: Cell<SurfaceKind>,
    radius: f64,
    border: bool,
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

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            // AppKit owns this layer. Apply colors only in its supported update path.
            let style = self.ivars();
            let kind = style.kind.get();
            let dark = is_dark(self);
            let color = surface_color(kind, dark);
            let border_color = if kind == SurfaceKind::SelectedCard {
                NSColor::systemTealColor()
            } else { rgb(if dark { 0x3A444F } else { 0xDCE3E8 }) };
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
        });
        let view: Retained<Self> =
            unsafe { msg_send![super(view), initWithFrame: rect(0.0, 0.0, 1.0, 1.0)] };
        view.setWantsLayer(true);
        view.setNeedsDisplay(true);
        view
    }
    fn set_kind(&self, kind: SurfaceKind) {
        if self.ivars().kind.replace(kind) != kind {
            self.setNeedsDisplay(true);
        }
    }
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
    unsafe impl NSWindowDelegate for ShellTarget {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _window: &NSWindow) -> bool {
            // CEF must finish every beforeunload/close callback before teardown.
            crate::browser::close_all();
            false
        }
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            layout();
        }
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
            if item.action() == Some(sel!(reopenClosedTab:)) {
                can_reopen_closed_tab_ui()
            } else {
                true
            }
        }

        #[unsafe(method(reopenClosedTab:))]
        fn reopen_closed_tab(&self, _sender: Option<&AnyObject>) {
            reopen_closed_tab();
        }

        #[unsafe(method(selectTab:))]
        fn select_tab(&self, sender: &NSButton) {
            let id: isize = unsafe { msg_send![sender, tag] };
            if let Ok(id) = i32::try_from(id) { select(id); }
        }
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
        #[unsafe(method(showSpaces:))]
        fn show_spaces(&self, _sender: Option<&AnyObject>) {
            let visible = SHELL.with(|slot| slot.borrow().as_ref().is_some_and(|shell| shell.overview));
            run_action(Some(ShellAction::ShowOverview(!visible)));
        }
        #[unsafe(method(showTabSearch:))]
        fn show_tab_search(&self, _sender: Option<&AnyObject>) { open_tab_search(); }
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
        #[unsafe(method(chooseDownloadPath:))]
        fn choose_download_path(&self, sender: &NSButton) { download_row_action(sender, true); }
        #[unsafe(method(cancelDownload:))]
        fn cancel_download(&self, sender: &NSButton) { download_row_action(sender, false); }
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
        #[unsafe(method(controlTextDidChange:))]
        fn search_text_changed(&self, _notification: &NSNotification) {
            if page_find_is_editing() { page_find_text_changed(); }
            else if bookmarks_are_open() { bookmark_text_changed(); } else { search_text_changed(); }
        }
        #[unsafe(method(toggleSidebar:))]
        fn toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow_mut().as_mut() {
                    shell.sidebar_collapsed = !shell.sidebar_collapsed;
                    shell.tab_signature.clear();
                    shell.render_controls();
                }
            });
            layout();
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
        fn reload_tab(&self, _sender: Option<&AnyObject>) {
            run_action(active_tab().map(ShellAction::Reload));
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
            rows: Vec::new(),
        }
    }

    fn clear_rows(&mut self) {
        for row in self.rows.drain(..) {
            row.surface.removeFromSuperview();
        }
    }

    // Keep stable controls during progress updates so Tab focus survives.
    // Return true only when the focused row itself was removed.
    fn render(
        &mut self,
        snapshot: crate::downloads::Snapshot,
        target: &ShellTarget,
        window: &NSWindow,
    ) -> bool {
        let summary = if snapshot.rows.is_empty() {
            "아직 다운로드가 없습니다".to_owned()
        } else {
            format!(
                "진행 중 {}개 · 최대 24개 표시 · 기록을 지워도 파일은 유지됩니다",
                snapshot.active_count
            )
        };
        let text = self
            .notice
            .as_deref()
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
                for control in [&choose, &cancel] {
                    unsafe {
                        let _: () = msg_send![&**control, setTag: item.id as isize];
                    }
                }
                DownloadRow {
                    id: item.id,
                    surface,
                    name,
                    source,
                    detail,
                    choose,
                    cancel,
                }
            };
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
            for (control, action) in [
                (&row.choose, "저장 위치 선택"),
                (&row.cancel, "다운로드 취소"),
            ] {
                let hint = NSString::from_str(&format!("{} · {action}", item.name));
                control.setToolTip(Some(&hint));
                unsafe {
                    let _: () = msg_send![&**control, setAccessibilityLabel: &*hint];
                }
            }
            self.rows.push(row);
        }
        let responder = window.firstResponder();
        let focus_removed = responder
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
        self.document.setFrame(rect(
            0.0,
            0.0,
            width,
            (self.rows.len() as f64 * 96.0).max(height),
        ));
        for (index, row) in self.rows.iter().enumerate() {
            row.surface
                .setFrame(rect(0.0, index as f64 * 96.0, width, 88.0));
            let text_width = (width - 232.0).max(1.0);
            row.name.setFrame(rect(14.0, 10.0, text_width, 21.0));
            row.source.setFrame(rect(14.0, 35.0, text_width, 18.0));
            row.detail.setFrame(rect(14.0, 58.0, text_width, 18.0));
            row.choose.setFrame(rect(width - 205.0, 29.0, 136.0, 30.0));
            row.cancel.setFrame(rect(width - 65.0, 29.0, 55.0, 30.0));
        }
    }
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
        self.search.open || self.bookmarks.state.open || self.downloads.open
    }

    fn rebuild_search_results(&mut self, bookmarks: crate::bookmark_runtime::Snapshot) {
        let tabs = tab_search::search(
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
        );
        let results = tab_search::combined_results(tabs, bookmarks.rows.iter().map(|item| item.id));
        self.search.reconcile(&results);
        let rows: Vec<_> = results
            .iter()
            .filter_map(|id| {
                let (name, detail, accessible) = match id {
                    SearchResultId::Tab(id) => {
                        let tab = self.tabs.iter().find(|tab| tab.id == *id)?;
                        let name = if tab.title.is_empty() || tab.url == "about:blank" {
                            "새 탭"
                        } else {
                            &tab.title
                        };
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
                        let item = bookmarks.rows.iter().find(|item| item.id == *id)?;
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
                };
                Some((*id, name, detail, accessible))
            })
            .collect();
        let tab_count = results
            .iter()
            .filter(|id| matches!(id, SearchResultId::Tab(_)))
            .count();
        let summary = if results.is_empty() {
            "일치하는 탭이나 북마크가 없습니다".to_owned()
        } else {
            format!(
                "탭 {}개 · 북마크 {}개 · 탭 우선, 최대 40개 · Enter로 열기, Esc로 닫기",
                tab_count,
                results.len() - tab_count
            )
        };
        let status = if !bookmarks.ready {
            Some("북마크를 불러오는 중 · 열린 탭은 검색할 수 있습니다".to_owned())
        } else if let Some(notice) = &bookmarks.notice {
            Some(if !bookmarks.writable && !bookmarks.busy {
                format!("북마크 읽기 전용 · 열기 가능: {notice}")
            } else {
                format!("북마크: {notice}")
            })
        } else if bookmarks.busy {
            Some("북마크 저장 중 · 이전 목록을 표시합니다".to_owned())
        } else if !bookmarks.writable {
            Some("북마크 읽기 전용 · 열기는 가능합니다".to_owned())
        } else {
            None
        };
        let hint = status
            .map(|status| format!("{status} · {summary}"))
            .unwrap_or(summary);
        self.search_hint
            .setStringValue(&NSString::from_str(&compact(&hint, 108)));
        self.search_hint
            .setToolTip(Some(&NSString::from_str(&hint)));
        unsafe {
            let _: () =
                msg_send![&*self.search_hint, setAccessibilityHelp: &*NSString::from_str(&hint)];
        }
        let signature = format!(
            "{:?}:{:?}:{rows:?}",
            self.search.query, self.search.selected
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
            size.height - 145.0,
            size.width - margin * 2.0,
            22.0,
        ));
        let height = (size.height - 177.0).max(1.0);
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
        let root = SurfaceView::new(marker, SurfaceKind::Window, 0.0, false);
        root.setFrame(rect(0.0, 0.0, 1180.0, 800.0));
        window.setContentView(Some(&root));
        let sidebar = surface_child(&root, SurfaceKind::Sidebar, 0.0, true, marker);
        let toolbar = surface_child(&root, SurfaceKind::Toolbar, 0.0, true, marker);
        let content = child_view(&root, marker);
        content.setWantsLayer(true);
        let tab_document = TopDownView::new(marker);
        let selected_tab_background =
            surface_child(&tab_document, SurfaceKind::Selection, 9.0, false, marker);
        selected_tab_background.setHidden(true);
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
        let spaces_background = surface_child(&sidebar, SurfaceKind::Selection, 9.0, false, marker);
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
        let search_button = button(
            &sidebar,
            "탭·북마크 검색",
            &target,
            sel!(showTabSearch:),
            "탭·북마크 검색 (⌘⇧A)",
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
        let space_label = label(&sidebar, "내 공간", 10.5, true, marker);
        space_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let brand = label(&sidebar, "Naru", 17.0, true, marker);
        let brand_icon = symbol_view(&sidebar, "safari", "Naru", marker);
        brand_icon.setContentTintColor(Some(&NSColor::systemTealColor()));
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
        let address_surface = surface_child(&toolbar, SurfaceKind::Address, 9.0, true, marker);
        let address_icon = symbol_view(&address_surface, "globe", "페이지 주소", marker);
        let address =
            NSTextField::initWithFrame(NSTextField::alloc(marker), rect(0.0, 0.0, 100.0, 28.0));
        address.setEditable(true);
        address.setSelectable(true);
        address.setBezeled(false);
        address.setBordered(false);
        address.setDrawsBackground(false);
        address.setFont(Some(&NSFont::systemFontOfSize(12.0)));
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
        let new_tab = button(
            &sidebar,
            "새 탭",
            &target,
            sel!(newTab:),
            "현재 공간에 새 탭 (⌘T)",
            marker,
        );
        let status_surface = surface_child(&toolbar, SurfaceKind::Badge, 10.0, false, marker);
        let status = label(&status_surface, "직접 사용 중", 10.5, false, marker);
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
        stop.setContentTintColor(Some(&NSColor::systemTealColor()));
        align_left(&spaces_button);
        align_left(&new_tab);
        let search_surface = surface_child(&root, SurfaceKind::Window, 0.0, true, marker);
        search_surface.setHidden(true);
        let search_title = label(&search_surface, "탭·북마크 검색", 24.0, true, marker);
        let search_hint = label(
            &search_surface,
            "모든 공간의 열린 탭과 공유 북마크 · 방문 기록은 검색하지 않습니다",
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
            "탭·북마크 검색 · 방향키로 선택, Enter로 이동, Esc로 닫기",
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
        let search_document = TopDownView::new(marker);
        let search_selection =
            surface_child(&search_document, SurfaceKind::Selection, 9.0, false, marker);
        search_selection.setHidden(true);
        let search_scroll = scroll_view(&search_surface, &search_document, marker);
        let bookmarks = BookmarkPanel::new(&root, &target, marker);
        let downloads = DownloadPanel::new(&root, &target, marker);
        let find = FindBar::new(&root, &target, marker);
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
            bookmarks_button,
            bookmarks,
            downloads_button,
            downloads,
            download_activity: (0, 0),
            find,
            search_surface,
            search_title,
            search_hint,
            search_field,
            search_close,
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
            status,
            status_surface,
            stop,
            tab_buttons: Vec::new(),
            selected_tab_background,
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
            card_signature: String::new(),
            displayed_address: None,
            address_draft: None,
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
        let browsing = !self.overview && tab.is_some();
        self.back
            .setEnabled(browsing && tab.is_some_and(|tab| tab.can_go_back));
        self.forward
            .setEnabled(browsing && tab.is_some_and(|tab| tab.can_go_forward));
        self.reload.setEnabled(browsing);
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
        if tab.is_some_and(|tab| {
            self.address_draft.as_ref().is_some_and(|draft| {
                draft.superseded(tab.id, &tab.url, tab.navigation_epoch, !editor.is_null())
            })
        }) {
            // Once editing ends, a later page navigation owns the address again.
            self.address_draft = None;
            self.notice = None;
        }
        if self.overview
            || self.displayed_address != self.active
            || (editor.is_null()
                && !self
                    .address_draft
                    .as_ref()
                    .is_some_and(|draft| Some(draft.tab) == self.active))
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
        self.stop.setEnabled(can_stop);
        self.stop.setHidden(!can_stop);
        let text = if let Some(notice) = &self.notice {
            compact(notice, 100)
        } else if self.overview {
            "공간 보기".into()
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
            NSColor::systemTealColor()
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
            NSColor::systemTealColor()
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
                "탭·북마크 검색"
            },
            "검색",
            "탭·북마크 검색 (⌘⇧A)",
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
        let download_title = (self.download_activity != (0, 0)).then(|| {
            if self.sidebar_collapsed {
                active_downloads.to_string()
            } else {
                format!("다운로드 {active_downloads}")
            }
        });
        let download_hint = (self.download_activity != (0, 0)).then(|| {
            format!(
                "다운로드 목록, 저장 위치 선택 대기 {pending_downloads}개, 전체 진행 {active_downloads}개"
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
        let downloads_color = if pending_downloads > 0 && !self.downloads.open {
            NSColor::systemTealColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.downloads_button
            .setContentTintColor(Some(&downloads_color));
        if self.sidebar_collapsed {
            align_center(&self.spaces_button);
            align_center(&self.new_tab);
            align_center(&self.search_button);
            align_center(&self.bookmarks_button);
            align_center(&self.downloads_button);
        } else {
            align_left(&self.spaces_button);
            align_left(&self.new_tab);
            align_left(&self.search_button);
            align_left(&self.bookmarks_button);
            align_left(&self.downloads_button);
        }
        let title = if self.overview {
            "공간 - Naru".into()
        } else {
            tab.map(|tab| compact(&tab.title, 80))
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
        let signature = format!(
            "{}:{:?}:{}:{:?}",
            self.active_space,
            self.active,
            self.sidebar_collapsed,
            visible
                .iter()
                .map(|tab| (
                    tab.id,
                    &tab.title,
                    &tab.url,
                    tab.loading,
                    tab.stop_scope.is_some()
                ))
                .collect::<Vec<_>>()
        );
        if self.tab_signature == signature {
            return;
        }
        self.tab_signature = signature;
        for row in self.tab_buttons.drain(..) {
            row.button.removeFromSuperview();
            row.icon.removeFromSuperview();
            row.title.removeFromSuperview();
        }
        let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
        for (index, tab) in visible.into_iter().enumerate() {
            let name = if tab.title.is_empty() || tab.url == "about:blank" {
                "새 탭"
            } else {
                &tab.title
            };
            let selected = Some(tab.id) == self.active;
            let symbol = if tab.stop_scope.is_some() {
                "sparkles"
            } else if tab.loading {
                "arrow.triangle.2.circlepath"
            } else {
                "globe"
            };
            let icon = symbol_view(&self.tab_document, symbol, name, marker);
            let icon_color = if selected || tab.stop_scope.is_some() {
                NSColor::systemTealColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            icon.setContentTintColor(Some(&icon_color));
            let title = label(
                &self.tab_document,
                &compact(name, 60),
                12.5,
                selected,
                marker,
            );
            title.setHidden(self.sidebar_collapsed);
            let button = button(
                &self.tab_document,
                if self.sidebar_collapsed && icon.image().is_none() {
                    "□"
                } else {
                    ""
                },
                &self.target,
                sel!(selectTab:),
                &format!("{name}\n{}", tab.url),
                marker,
            );
            unsafe {
                let _: () = msg_send![&*button, setTag: tab.id as isize];
                let _: () = msg_send![&*button, setAccessibilityLabel: &*NSString::from_str(&format!("{}: {name}{}", index + 1, if selected { ", 선택됨" } else { "" }))];
            }
            self.tab_buttons.push(TabRow {
                button,
                icon,
                title,
            });
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
            let detail_text = representative_title
                .map(|title| format!("탭 {}개, {}", space.tabs.len(), compact(title, 48)))
                .unwrap_or_else(|| format!("탭 {}개", space.tabs.len()));
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
                NSColor::systemTealColor()
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
            open.setContentTintColor(Some(&NSColor::systemTealColor()));
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
            stop.setContentTintColor(Some(&NSColor::systemTealColor()));
            stop.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            stop.setHidden(space_stop_scope(space).is_none());
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
    SHELL.with(|slot| *slot.borrow_mut() = Some(shell));
    layout();
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

fn set_download_notice(notice: Option<String>) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.downloads.notice = notice;
        }
    });
}

fn open_downloads() {
    // A toolbar click may already have ended the address field editor. Keep its
    // current text through native overlay updates without changing page state.
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            if !shell.overview && shell.address_draft.is_none() {
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
        }
    });
    close_page_find(false, false);
    close_tab_search(false);
    close_bookmarks(false);
    let controls = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.downloads.open {
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
    close_page_find(false, false);
    let tab = active_tab();
    close_tab_search(false);
    close_downloads(false);
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
    close_page_find(false, false);
    close_bookmarks(false);
    close_downloads(false);
    let controls = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.search.open {
            shell.search = SearchState {
                open: true,
                ..SearchState::default()
            };
            shell.search_signature.clear();
        }
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

fn close_tab_search(restore_focus: bool) {
    let window = SHELL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let shell = slot.as_mut()?;
        if !shell.search.open {
            return None;
        }
        shell.search = SearchState::default();
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
    let field = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|shell| shell.search.open)
            .map(|shell| shell.search_field.clone())
    });
    let Some(field) = field else { return };
    let text = field.stringValue().to_string();
    let bounded = tab_search::bounded_query(&text);
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut().filter(|shell| shell.search.open) {
            shell.search.set_query(&bounded);
        }
    });
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
            .filter(|shell| shell.search.open)
            .and_then(|shell| shell.search.selected)
    });
    if let Some(result) = result {
        activate_search_result(result);
    }
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
    };
    match opened {
        Ok(()) => close_tab_search(true),
        Err(error) => {
            sync();
            SHELL.with(|slot| {
                if let Some(shell) = slot.borrow().as_ref() {
                    let message = format!("이 결과를 열지 못했습니다: {error} · 다른 탭이나 북마크를 선택하세요");
                    shell.search_hint.setStringValue(&NSString::from_str(&compact(&message, 108)));
                    shell.search_hint.setToolTip(Some(&NSString::from_str(&message)));
                    unsafe { let _: () = msg_send![&*shell.search_hint, setAccessibilityHelp: &*NSString::from_str(&message)]; }
                }
            });
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
                if shell.overview || shell.overlay_is_open() {
                    None
                } else {
                    shell.active
                },
                shell.overlay_is_open(),
                shell
                    .views
                    .iter()
                    .map(|(id, view)| (*id, view.clone()))
                    .collect::<Vec<_>>(),
            )
        })
    });
    let Some((window, active, search_open, views)) = state else {
        return;
    };
    if focus && active.is_none() && !search_open && !find_editing {
        window.makeFirstResponder(None);
    }
    for (id, view) in views {
        view.setHidden(active != Some(id));
    }
    // Focus WebContents through CEF, after releasing all borrowed/retained views.
    if focus && !search_open && !find_editing {
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
                shell.find.state.open,
                shell.find.surface.clone(),
                shell.views.values().cloned().collect::<Vec<_>>(),
            )
        })
    });
    let Some((
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
        find_open,
        find_surface,
        views,
    )) = ui
    else {
        return;
    };
    let size = root.bounds().size;
    let side = if collapsed { RAIL_WIDTH } else { SIDEBAR_WIDTH };
    let main_width = (size.width - side).max(1.0);
    let toolbar_bottom = (size.height - TOOLBAR_HEIGHT).max(1.0);
    let overlay_open = search_open || bookmarks_open || downloads_open;
    let find_visible = find_open && !overview && !overlay_open;
    let page_height = (toolbar_bottom - if find_visible { FIND_BAR_HEIGHT } else { 0.0 }).max(1.0);
    sidebar.setFrame(rect(0.0, 0.0, side, size.height));
    toolbar.setFrame(rect(side, toolbar_bottom, main_width, TOOLBAR_HEIGHT));
    find_surface.setFrame(rect(side, page_height, main_width, FIND_BAR_HEIGHT));
    find_surface.setHidden(!find_visible);
    sidebar.setHidden(overlay_open);
    toolbar.setHidden(overlay_open);
    content.setFrame(rect(side, 0.0, main_width, page_height));
    overview_scroll.setFrame(rect(side, 0.0, main_width, page_height));
    content.setHidden(overview || overlay_open);
    overview_scroll.setHidden(!overview || overlay_open);
    search_surface.setHidden(!search_open);
    bookmarks_surface.setHidden(!bookmarks_open);
    downloads_surface.setHidden(!downloads_open);
    // Only the CEF hierarchy can re-enter browser callbacks during layout.
    // Its frames are set before borrowing the shell for native chrome controls.
    let bounds = content.bounds();
    for view in views {
        view.setFrame(bounds);
    }
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            shell.layout_chrome(size, side, main_width);
            if find_visible {
                shell.find.layout(main_width);
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
        }
    });
}

impl Shell {
    fn layout_chrome(&self, size: NSSize, side: f64, main_width: f64) {
        self.brand_icon
            .setFrame(rect(16.0, size.height - 34.0, 21.0, 21.0));
        self.brand
            .setFrame(rect(45.0, size.height - 36.0, (side - 90.0).max(1.0), 25.0));
        self.sidebar_toggle.setFrame(rect(
            if self.sidebar_collapsed {
                14.0
            } else {
                side - 38.0
            },
            size.height - 35.0,
            26.0,
            26.0,
        ));
        self.spaces_background
            .setFrame(rect(10.0, size.height - 83.0, side - 20.0, 34.0));
        self.spaces_button.setFrame(rect(
            if self.sidebar_collapsed { 10.0 } else { 20.0 },
            size.height - 83.0,
            if self.sidebar_collapsed {
                side - 20.0
            } else {
                side - 40.0
            },
            34.0,
        ));
        self.space_label
            .setFrame(rect(18.0, size.height - 114.0, side - 36.0, 17.0));
        self.tab_scroll.setFrame(rect(
            8.0,
            168.0,
            side - 16.0,
            (size.height - if self.sidebar_collapsed { 264.0 } else { 292.0 }).max(1.0),
        ));
        self.downloads_button.setFrame(rect(
            if self.sidebar_collapsed { 10.0 } else { 20.0 },
            125.0,
            if self.sidebar_collapsed {
                side - 20.0
            } else {
                side - 40.0
            },
            30.0,
        ));
        self.bookmarks_button.setFrame(rect(
            if self.sidebar_collapsed { 10.0 } else { 20.0 },
            89.0,
            if self.sidebar_collapsed {
                side - 20.0
            } else {
                side - 40.0
            },
            30.0,
        ));
        self.search_button.setFrame(rect(
            if self.sidebar_collapsed { 10.0 } else { 20.0 },
            53.0,
            if self.sidebar_collapsed {
                side - 20.0
            } else {
                side - 40.0
            },
            30.0,
        ));
        self.new_tab.setFrame(rect(
            if self.sidebar_collapsed { 10.0 } else { 20.0 },
            15.0,
            if self.sidebar_collapsed {
                side - 20.0
            } else {
                side - 40.0
            },
            30.0,
        ));
        self.back.setFrame(rect(8.0, 8.0, 28.0, 28.0));
        self.forward.setFrame(rect(37.0, 8.0, 28.0, 28.0));
        self.reload.setFrame(rect(66.0, 8.0, 28.0, 28.0));
        let stop_width = if self.stop.isHidden() { 0.0 } else { 68.0 };
        let badge_width = if main_width < 650.0 { 96.0 } else { 110.0 };
        let address_width = (main_width - 154.0 - badge_width - stop_width).max(100.0);
        self.address_surface
            .setFrame(rect(104.0, 7.0, address_width, 30.0));
        self.address_icon.setFrame(rect(10.0, 8.0, 14.0, 14.0));
        self.address
            .setFrame(rect(32.0, 6.0, (address_width - 42.0).max(1.0), 18.0));
        let badge_x = 114.0 + address_width;
        self.status_surface
            .setFrame(rect(badge_x, 11.0, badge_width, 22.0));
        self.status
            .setFrame(rect(9.0, 3.0, badge_width - 18.0, 16.0));
        self.stop
            .setFrame(rect(badge_x + badge_width + 6.0, 8.0, 60.0, 28.0));
        self.close_tab
            .setFrame(rect(main_width - 34.0, 8.0, 26.0, 28.0));
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
            row.button.setFrame(rect(2.0, y, width - 4.0, 32.0));
            row.icon.setFrame(rect(
                if self.sidebar_collapsed {
                    (width - 16.0) / 2.0
                } else {
                    14.0
                },
                y + 8.0,
                16.0,
                16.0,
            ));
            row.title
                .setFrame(rect(40.0, y + 7.0, (width - 52.0).max(1.0), 19.0));
            let id: isize = unsafe { msg_send![&*row.button, tag] };
            if self.active == i32::try_from(id).ok() {
                self.selected_tab_background.setFrame(row.button.frame());
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

pub fn refresh() {
    let marker = MainThreadMarker::new().expect("Browser shell requires the main thread");
    let visible = SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.window.isVisible() && !shell.window.isMiniaturized())
    });
    if visible && !NSApp(marker).isHidden() {
        sync();
    }
}

pub fn sync() {
    let snapshot = crate::browser::shell_snapshot();
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
    let bookmark_query = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        if shell.search.open {
            Some(shell.search.query.clone())
        } else if shell.bookmarks.state.open {
            Some(shell.bookmarks.state.query.clone())
        } else {
            None
        }
    });
    let bookmarks = bookmark_query.map(|query| crate::bookmark_runtime::snapshot(&query));
    let download_activity = crate::download_runtime::activity();
    let downloads = downloads_are_open().then(crate::download_runtime::snapshot);
    let mut download_focus = None;
    let changed = SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            let changed =
                shell.active != snapshot.active_tab || shell.overview != snapshot.overview;
            shell.download_activity = download_activity;
            shell.render(snapshot);
            if let Some(bookmarks) = bookmarks {
                if shell.search.open {
                    shell.rebuild_search_results(bookmarks);
                } else if shell.bookmarks.state.open {
                    shell.bookmarks.render(bookmarks, &shell.target);
                }
            }
            if let Some(downloads) = downloads {
                if shell.downloads.open
                    && shell
                        .downloads
                        .render(downloads, &shell.target, &shell.window)
                {
                    download_focus = Some((shell.window.clone(), shell.downloads.close.clone()));
                }
            }
            changed
        } else {
            false
        }
    });
    layout();
    apply_selection(changed);
    if let Some((window, close)) = download_focus.filter(|_| downloads_are_open()) {
        window.makeFirstResponder(Some(&close));
    }
    crate::browser::request_space_previews();
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
        let slot = slot.borrow();
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
    if !SHELL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|shell| shell.window.isKeyWindow())
    }) {
        return false;
    }
    let Some(key) = shortcut_key(event) else {
        return false;
    };
    let modifiers = event.modifierFlags();
    // Consume this chord before page-input handoff, even when reopening is disabled.
    if is_reopen_shortcut(&key, modifiers) {
        if !event.isARepeat() {
            reopen_closed_tab();
        }
        return true;
    }
    let command = modifiers.contains(NSEventModifierFlags::Command)
        && !modifiers.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option);
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
    let cycle = modifiers.contains(NSEventModifierFlags::Control)
        && !modifiers.intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Option)
        && matches!(key.as_str(), "\t" | "\u{19}");
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
        if cycle
            || (command
                && (matches!(key.as_str(), "l" | "t" | "w" | "r" | "{" | "}" | "[" | "]")
                    || is_menu_only_shortcut(&key, modifiers)))
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
        if cycle
            || (command
                && (matches!(key.as_str(), "l" | "t" | "w" | "r" | "{" | "}" | "[" | "]")
                    || is_menu_only_shortcut(&key, modifiers)))
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
            match event.keyCode() {
                53 => {
                    close_tab_search(true);
                    return true;
                }
                125 => {
                    move_search_selection(true);
                    return true;
                }
                126 => {
                    move_search_selection(false);
                    return true;
                }
                36 | 76 => {
                    submit_tab_search();
                    return true;
                }
                _ => {}
            }
        }
        if cycle
            || (command
                && (matches!(key.as_str(), "l" | "t" | "w" | "r" | "{" | "}" | "[" | "]")
                    || is_menu_only_shortcut(&key, modifiers)))
        {
            close_tab_search(false);
        }
    }
    if cycle {
        if !event.isARepeat() {
            cycle_tab(!modifiers.contains(NSEventModifierFlags::Shift));
        }
        return true;
    }
    if !command || !matches!(key.as_str(), "l" | "t" | "w" | "r" | "{" | "}" | "[" | "]") {
        return false;
    }
    if event.isARepeat() {
        return true;
    }
    match key.as_str() {
        "l" => focus_address(),
        "t" => run_action(Some(ShellAction::NewTab)),
        "w" => run_action(active_tab().map(ShellAction::CloseTab)),
        "r" => run_action(active_tab().map(ShellAction::Reload)),
        "{" => cycle_tab(false),
        "}" => cycle_tab(true),
        "[" if modifiers.contains(NSEventModifierFlags::Shift) => cycle_tab(false),
        "]" if modifiers.contains(NSEventModifierFlags::Shift) => cycle_tab(true),
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
        "탭·북마크 검색",
        Some(sel!(showTabSearch:)),
        "a",
        Some(target),
        true,
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
    let id = SHELL.with(|slot| {
        let slot = slot.borrow();
        let shell = slot.as_ref()?;
        let tabs: Vec<_> = shell
            .tabs
            .iter()
            .filter(|tab| tab.space == shell.active_space)
            .collect();
        let index = tabs.iter().position(|tab| Some(tab.id) == shell.active)?;
        let next = if forward {
            (index + 1) % tabs.len()
        } else {
            (index + tabs.len() - 1) % tabs.len()
        };
        Some(tabs[next].id)
    });
    if let Some(id) = id {
        select(id);
    }
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
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
    unsafe {
        let _: () = msg_send![&*label, setLineBreakMode: 4isize];
        let _: () = msg_send![&*label, setMaximumNumberOfLines: 1isize];
    }
    parent.addSubview(&label);
    label
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
        SurfaceKind::Window => (0xF7F9FA, 0x171C22),
        SurfaceKind::Sidebar => (0xEEF2F4, 0x1D242C),
        SurfaceKind::Toolbar => (0xF7F9FA, 0x1C232B),
        SurfaceKind::Card | SurfaceKind::SelectedCard => (0xFFFFFF, 0x252D37),
        SurfaceKind::Selection => (0xDCEFEB, 0x1E403D),
        SurfaceKind::Preview => (0xEBF0F3, 0x1A222B),
        SurfaceKind::Address => (0xFFFFFF, 0x2B3440),
        SurfaceKind::Badge => (0xEAF0F3, 0x303C48),
        SurfaceKind::AccentBadge => (0xDFF2EF, 0x1C4641),
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
            sel!(terminate:),
        ] {
            assert!(!is_page_edit_action(action));
        }
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
