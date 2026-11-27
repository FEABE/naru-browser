// CEF OSR 스캐폴딩 — 앱/브라우저프로세스/렌더/디스플레이/수명/클라이언트 핸들러 + 브라우저 생성
use cef::{rc::Rc as _, *};
use eframe::egui::{Color32, CursorIcon};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub struct FrameBuf { pub w: usize, pub h: usize, pub pixels: Vec<Color32>, pub dirty: bool }
pub type SharedFrame = Rc<RefCell<FrameBuf>>;
pub type SharedSize = Rc<RefCell<(f32, f32)>>;
pub type SharedStr = Rc<RefCell<String>>;
pub type SharedScale = Rc<RefCell<f32>>;          // 디바이스 배율(HiDPI) — 매 프레임 ctx.pixels_per_point로 갱신
pub type SharedCursor = Rc<RefCell<CursorIcon>>;  // 웹페이지가 요청한 커서 모양(OSR은 OS가 모르니 직접 받음)
pub type SharedPopups = Rc<RefCell<Vec<String>>>; // on_before_popup으로 취소한 팝업 URL 큐 → Naru 탭으로 열림
pub type SharedF32 = Rc<RefCell<f32>>;            // 로딩 진행도(0~1)
pub type SharedFind = Rc<RefCell<(i32, i32)>>;    // 페이지 내 찾기 결과 (현재 매치 순번, 총 매치 수)

// 탭별 공유 I/O 묶음 — create_browser에 넘길 핸들러↔UI 공유셀들(위치 인자 폭발·오삽입 방지)
#[derive(Clone)]
pub struct BrowserIo {
    pub url: SharedStr, pub title: SharedStr, pub cursor: SharedCursor,
    pub popups: SharedPopups, pub progress: SharedF32, pub status: SharedStr,
    pub find_result: SharedFind,
}

// CEF 커서 타입 → egui 커서 아이콘 (링크=손가락, 입력=I-beam, 리사이즈 방향 등)
fn cef_cursor_to_egui(t: CursorType) -> CursorIcon {
    use CursorType as C;
    if      t==C::HAND { CursorIcon::PointingHand }
    else if t==C::IBEAM { CursorIcon::Text }
    else if t==C::CROSS { CursorIcon::Crosshair }
    else if t==C::WAIT { CursorIcon::Wait }
    else if t==C::PROGRESS { CursorIcon::Progress }
    else if t==C::HELP { CursorIcon::Help }
    else if t==C::MOVE { CursorIcon::Move }
    else if t==C::GRAB { CursorIcon::Grab }
    else if t==C::GRABBING { CursorIcon::Grabbing }
    else if t==C::NOTALLOWED { CursorIcon::NotAllowed }
    else if t==C::NODROP { CursorIcon::NoDrop }
    else if t==C::COPY { CursorIcon::Copy }
    else if t==C::CONTEXTMENU { CursorIcon::ContextMenu }
    else if t==C::CELL { CursorIcon::Cell }
    else if t==C::VERTICALTEXT { CursorIcon::VerticalText }
    else if t==C::ALIAS { CursorIcon::Alias }
    else if t==C::ZOOMIN { CursorIcon::ZoomIn }
    else if t==C::ZOOMOUT { CursorIcon::ZoomOut }
    else if t==C::EASTWESTRESIZE || t==C::EASTRESIZE || t==C::WESTRESIZE { CursorIcon::ResizeHorizontal }
    else if t==C::NORTHSOUTHRESIZE || t==C::NORTHRESIZE || t==C::SOUTHRESIZE { CursorIcon::ResizeVertical }
    else if t==C::NORTHEASTSOUTHWESTRESIZE || t==C::NORTHEASTRESIZE || t==C::SOUTHWESTRESIZE { CursorIcon::ResizeNeSw }
    else if t==C::NORTHWESTSOUTHEASTRESIZE || t==C::NORTHWESTRESIZE || t==C::SOUTHEASTRESIZE { CursorIcon::ResizeNwSe }
    else if t==C::COLUMNRESIZE { CursorIcon::ResizeColumn }
    else if t==C::ROWRESIZE { CursorIcon::ResizeRow }
    else { CursorIcon::Default }
}

#[derive(Clone)] pub struct NaruApp {}
impl NaruApp { pub fn new() -> Self { Self {} } }
wrap_app! {
    pub(crate) struct AppBuilder { app: NaruApp, }
    impl App {
        fn on_before_command_line_processing(&self, _p: Option<&cef::CefStringUtf16>, command_line: Option<&mut cef::CommandLine>) {
            let Some(cl) = command_line else { return };
            cl.append_switch(Some(&"no-startup-window".into()));
            cl.append_switch(Some(&"noerrdialogs".into()));
            cl.append_switch(Some(&"hide-crash-restore-bubble".into()));
            cl.append_switch(Some(&"use-mock-keychain".into()));
            // 에이전트 제어용 CDP는 opt-in: NARU_AGENT 설정 시에만 포트 노출(기본 비노출 — 임의 로컬/웹 접근 차단).
            // remote-allow-origins(=*)는 설정하지 않음 → Origin 없는 로컬 에이전트만 접속, 웹페이지발 접속(DNS rebinding) 차단.
            if std::env::var("NARU_AGENT").is_ok() {
                let port = std::env::var("NARU_CDP_PORT").unwrap_or_else(|_| "9222".into());
                cl.append_switch_with_value(Some(&"remote-debugging-port".into()), Some(&port.as_str().into()));
            }
        }
        fn browser_process_handler(&self) -> Option<cef::BrowserProcessHandler> {
            Some(BrowserProcessHandlerBuilder::build(NaruBph::new()))
        }
    }
}
impl AppBuilder { pub(crate) fn build(app: NaruApp) -> cef::App { Self::new(app) } }
#[derive(Clone)] pub struct NaruBph { ready: RefCell<bool> }
impl NaruBph { pub fn new() -> Self { Self { ready: RefCell::new(false) } } }
wrap_browser_process_handler! {
    pub(crate) struct BrowserProcessHandlerBuilder { handler: NaruBph, }
    impl BrowserProcessHandler { fn on_context_initialized(&self) { *self.handler.ready.borrow_mut() = true; } }
}
impl BrowserProcessHandlerBuilder { pub(crate) fn build(handler: NaruBph) -> BrowserProcessHandler { Self::new(handler) } }
#[derive(Clone)] pub struct NaruRender { frame: SharedFrame, size: SharedSize, scale: SharedScale }
impl NaruRender { pub fn new(frame: SharedFrame, size: SharedSize, scale: SharedScale) -> Self { Self { frame, size, scale } } }
wrap_render_handler! {
    pub struct RenderHandlerBuilder { handler: NaruRender, }
    impl RenderHandler {
        fn view_rect(&self, _b: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect { let (w,h)=*self.handler.size.borrow();
                if w>0.0 && h>0.0 { rect.x=0; rect.y=0; rect.width=w as _; rect.height=h as _; } }
        }
        fn screen_info(&self, _b: Option<&mut Browser>, si: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            if let Some(si)=si { si.device_scale_factor=*self.handler.scale.borrow(); return true as _; } false as _
        }
        fn on_paint(&self, _b: Option<&mut Browser>, t: PaintElementType, _d: Option<&[Rect]>,
                    buffer: *const u8, width: ::std::os::raw::c_int, height: ::std::os::raw::c_int) {
            // VIEW 페인트만 처리 — POPUP(<select> 드롭다운 등)은 작은 버퍼라 전체 프레임을 덮어쓰면 깨짐.
            // (팝업 합성은 추후 과제 — 지금은 드롭다운 오버레이만 미표시)
            if t != PaintElementType::VIEW { return; }
            if buffer.is_null() || width<=0 || height<=0 { return; }
            let w=width as usize; let h=height as usize; let px=w*h; let n=px*4;
            let src=unsafe{ std::slice::from_raw_parts(buffer,n) };
            let mut f=self.handler.frame.borrow_mut();
            if f.pixels.len()!=px { f.pixels.resize(px,Color32::TRANSPARENT); }
            // BGRA(unmultiplied) → Color32 1패스 변환 (기존: 바이트 스왑 + from_rgba 재변환 2패스 제거)
            let dst=&mut f.pixels;
            for p in 0..px { let i=p*4; dst[p]=Color32::from_rgba_unmultiplied(src[i+2],src[i+1],src[i],src[i+3]); }
            f.w=w; f.h=h; f.dirty=true;
        }
    }
}
impl RenderHandlerBuilder { pub fn build(handler: NaruRender) -> RenderHandler { Self::new(handler) } }

// DisplayHandler: 페이지 이동/제목/커서/로딩진행/상태메시지를 공유셀에 기록 → UI 동기화
#[derive(Clone)] pub struct NaruDisplay { url: SharedStr, title: SharedStr, cursor: SharedCursor, progress: SharedF32, status: SharedStr }
wrap_display_handler! {
    pub struct DisplayHandlerBuilder { handler: NaruDisplay, }
    impl DisplayHandler {
        fn on_address_change(&self, _b: Option<&mut Browser>, _f: Option<&mut cef::Frame>, url: Option<&CefString>) {
            if let Some(u)=url { *self.handler.url.borrow_mut()=u.to_string(); }
        }
        fn on_title_change(&self, _b: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(t)=title { *self.handler.title.borrow_mut()=t.to_string(); }
        }
        fn on_loading_progress_change(&self, _b: Option<&mut Browser>, progress: f64) {
            *self.handler.progress.borrow_mut() = progress as f32;
        }
        fn on_status_message(&self, _b: Option<&mut Browser>, value: Option<&CefString>) {
            *self.handler.status.borrow_mut() = value.map(|v|v.to_string()).unwrap_or_default();
        }
        // OSR은 OS 창이 없어 웹 커서를 직접 받아 egui 쪽에 반영해야 함. true 반환 = 우리가 처리함.
        fn on_cursor_change(&self, _b: Option<&mut Browser>, _cursor: sys::HCURSOR, type_: CursorType,
                            _ci: Option<&CursorInfo>) -> ::std::os::raw::c_int {
            *self.handler.cursor.borrow_mut() = cef_cursor_to_egui(type_);
            true as _
        }
    }
}
impl DisplayHandlerBuilder { pub fn build(h: NaruDisplay) -> DisplayHandler { Self::new(h) } }

// LifeSpanHandler: 팝업/새 창을 CEF가 따로 띄우지 않게 취소하고(1 반환) 대상 URL만 큐에 담음 → Naru 새 탭으로 연다.
#[derive(Clone)] pub struct NaruLifeSpan { popups: SharedPopups }
wrap_life_span_handler! {
    pub struct LifeSpanHandlerBuilder { handler: NaruLifeSpan, }
    impl LifeSpanHandler {
        #[allow(clippy::too_many_arguments)]
        fn on_before_popup(&self, _b: Option<&mut Browser>, _f: Option<&mut Frame>, _id: ::std::os::raw::c_int,
                           target_url: Option<&CefString>, _tfn: Option<&CefString>, _disp: WindowOpenDisposition,
                           user_gesture: ::std::os::raw::c_int, _pf: Option<&PopupFeatures>,
                           _wi: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>,
                           _settings: Option<&mut BrowserSettings>, _extra: Option<&mut Option<DictionaryValue>>,
                           _nojs: Option<&mut ::std::os::raw::c_int>) -> ::std::os::raw::c_int {
            // 사용자 제스처(클릭 등)로 연 것만 탭으로 승격 — 스크립트가 자동으로 여는 광고 팝업은 그냥 차단.
            if user_gesture != 0 {
                if let Some(u)=target_url { let s=u.to_string();
                    if !s.is_empty() && s!="about:blank" { self.handler.popups.borrow_mut().push(s); } }
            }
            true as _ // CEF 팝업 취소
        }
    }
}
impl LifeSpanHandlerBuilder { pub fn build(h: NaruLifeSpan) -> LifeSpanHandler { Self::new(h) } }

// FindHandler: 페이지 내 찾기 결과(현재 순번/총 개수)를 공유셀에 기록 → 찾기바에 "n/m" 표시
#[derive(Clone)] pub struct NaruFind { result: SharedFind }
wrap_find_handler! {
    pub struct FindHandlerBuilder { handler: NaruFind, }
    impl FindHandler {
        fn on_find_result(&self, _b: Option<&mut Browser>, _id: ::std::os::raw::c_int, count: ::std::os::raw::c_int,
                          _rect: Option<&Rect>, active: ::std::os::raw::c_int, _final: ::std::os::raw::c_int) {
            *self.handler.result.borrow_mut() = (active, count);
        }
    }
}
impl FindHandlerBuilder { pub fn build(h: NaruFind) -> FindHandler { Self::new(h) } }

wrap_client! {
    pub(crate) struct ClientBuilder { render_handler: RenderHandler, display_handler: DisplayHandler, life_span_handler: LifeSpanHandler, find_handler: FindHandler, }
    impl Client {
        fn render_handler(&self) -> Option<cef::RenderHandler> { Some(self.render_handler.clone()) }
        fn display_handler(&self) -> Option<cef::DisplayHandler> { Some(self.display_handler.clone()) }
        fn life_span_handler(&self) -> Option<cef::LifeSpanHandler> { Some(self.life_span_handler.clone()) }
        fn find_handler(&self) -> Option<cef::FindHandler> { Some(self.find_handler.clone()) }
    }
}
impl ClientBuilder { pub(crate) fn build(r: RenderHandler, d: DisplayHandler, l: LifeSpanHandler, fh: FindHandler) -> cef::Client { Self::new(r, d, l, fh) } }

pub fn create_browser(url:&str, frame:SharedFrame, size:SharedSize, scale:SharedScale, io:BrowserIo) -> Browser {
    let render = NaruRender::new(frame, size, scale);
    let disp = NaruDisplay { url:io.url, title:io.title, cursor:io.cursor, progress:io.progress, status:io.status };
    let ls = NaruLifeSpan { popups:io.popups };
    let fh = NaruFind { result:io.find_result };
    let mut client = ClientBuilder::build(RenderHandlerBuilder::build(render), DisplayHandlerBuilder::build(disp), LifeSpanHandlerBuilder::build(ls), FindHandlerBuilder::build(fh));
    let wi = WindowInfo { windowless_rendering_enabled: true as _, ..Default::default() };
    let bs = BrowserSettings { windowless_frame_rate: 60, ..Default::default() };
    browser_host_create_browser_sync(Some(&wi), Some(&mut client), Some(&url.into()), Some(&bs), None, None)
        .expect("tab browser 생성 실패")
}
