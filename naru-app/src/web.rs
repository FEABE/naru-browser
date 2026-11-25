// CEF OSR 스캐폴딩 — 앱/브라우저프로세스/렌더/디스플레이/클라이언트 핸들러 + 브라우저 생성
use cef::{rc::Rc as _, *};
use eframe::egui::Color32;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub struct FrameBuf { pub w: usize, pub h: usize, pub pixels: Vec<Color32>, pub dirty: bool }
pub type SharedFrame = Rc<RefCell<FrameBuf>>;
pub type SharedSize = Rc<RefCell<(f32, f32)>>;
pub type SharedStr = Rc<RefCell<String>>;

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
#[derive(Clone)] pub struct NaruRender { frame: SharedFrame, size: SharedSize, scale: f32 }
impl NaruRender { pub fn new(frame: SharedFrame, size: SharedSize, scale: f32) -> Self { Self { frame, size, scale } } }
wrap_render_handler! {
    pub struct RenderHandlerBuilder { handler: NaruRender, }
    impl RenderHandler {
        fn view_rect(&self, _b: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect { let (w,h)=*self.handler.size.borrow();
                if w>0.0 && h>0.0 { rect.x=0; rect.y=0; rect.width=w as _; rect.height=h as _; } }
        }
        fn screen_info(&self, _b: Option<&mut Browser>, si: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            if let Some(si)=si { si.device_scale_factor=self.handler.scale; return true as _; } false as _
        }
        fn on_paint(&self, _b: Option<&mut Browser>, _t: PaintElementType, _d: Option<&[Rect]>,
                    buffer: *const u8, width: ::std::os::raw::c_int, height: ::std::os::raw::c_int) {
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

// DisplayHandler: 페이지 이동/제목 변경을 공유셀에 기록 → UI 동기화 (에이전트·링크·리다이렉트 전부)
#[derive(Clone)] pub struct NaruDisplay { url: SharedStr, title: SharedStr }
wrap_display_handler! {
    pub struct DisplayHandlerBuilder { handler: NaruDisplay, }
    impl DisplayHandler {
        fn on_address_change(&self, _b: Option<&mut Browser>, _f: Option<&mut cef::Frame>, url: Option<&CefString>) {
            if let Some(u)=url { *self.handler.url.borrow_mut()=u.to_string(); }
        }
        fn on_title_change(&self, _b: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(t)=title { *self.handler.title.borrow_mut()=t.to_string(); }
        }
    }
}
impl DisplayHandlerBuilder { pub fn build(h: NaruDisplay) -> DisplayHandler { Self::new(h) } }

wrap_client! {
    pub(crate) struct ClientBuilder { render_handler: RenderHandler, display_handler: DisplayHandler, }
    impl Client {
        fn render_handler(&self) -> Option<cef::RenderHandler> { Some(self.render_handler.clone()) }
        fn display_handler(&self) -> Option<cef::DisplayHandler> { Some(self.display_handler.clone()) }
    }
}
impl ClientBuilder { pub(crate) fn build(r: RenderHandler, d: DisplayHandler) -> cef::Client { Self::new(r, d) } }

pub fn create_browser(url:&str, frame:SharedFrame, size:SharedSize, surl:SharedStr, stitle:SharedStr) -> Browser {
    let render = NaruRender::new(frame, size, 1.0);
    let disp = NaruDisplay { url:surl, title:stitle };
    let mut client = ClientBuilder::build(RenderHandlerBuilder::build(render), DisplayHandlerBuilder::build(disp));
    let wi = WindowInfo { windowless_rendering_enabled: true as _, ..Default::default() };
    let bs = BrowserSettings { windowless_frame_rate: 60, ..Default::default() };
    browser_host_create_browser_sync(Some(&wi), Some(&mut client), Some(&url.into()), Some(&bs), None, None)
        .expect("tab browser 생성 실패")
}
