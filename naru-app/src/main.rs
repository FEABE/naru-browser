// Naru — egui 셸 + 실제 CEF OSR 멀티탭 (완성본 v4)
// init.Tasks식 3단계 트리: 영역(personal/company/sidejob) → 프로젝트 → 탭.
// 탭은 활성화 시 lazy 생성, 비활성은 was_hidden. 에이전트 컨트롤 pill.
use cef::{rc::Rc as _, args::Args, *};
use eframe::egui;
use egui::{vec2, Align, Color32, FontFamily, FontId, Frame, Layout, Margin, Pos2,
           RichText, Rounding, Sense, Stroke, TextStyle, Ui};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

fn mark(s: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
        .open("C:\\Users\\Lenovo\\AppData\\Local\\Temp\\naru_trace.txt") { let _ = writeln!(f, "{}", s); }
}

#[derive(Default)]
struct FrameBuf { w: usize, h: usize, rgba: Vec<u8>, dirty: bool }
type SharedFrame = Rc<RefCell<FrameBuf>>;
type SharedSize = Rc<RefCell<(f32, f32)>>;

#[derive(Clone, Copy)]
struct Pal { bg:Color32, side:Color32, txt:Color32, muted:Color32, faint:Color32, border:Color32,
    hover:Color32, selr:Color32, addbg:Color32, web:Color32, accent:Color32, deleg:Color32, topbar:Color32 }
fn c(r:u8,g:u8,b:u8)->Color32{ Color32::from_rgb(r,g,b) }
impl Pal {
    fn light()->Self{ Pal{ bg:c(0xF9,0xF9,0xFC), side:c(0xFC,0xFC,0xFE), txt:c(0x2B,0x2B,0x36),
        muted:c(0x8C,0x8C,0x9C), faint:c(0xBE,0xBE,0xCC), border:c(0xE9,0xE9,0xF1),
        hover:c(0xF0,0xF0,0xF7), selr:c(0xEC,0xEC,0xFB), addbg:c(0xF2,0xF2,0xF8), web:c(0xFC,0xFC,0xFE),
        accent:c(0x5B,0x5B,0xD6), deleg:c(0xC8,0x5C,0x6E), topbar:c(0xF4,0xF4,0xFC) } }
    fn dark()->Self{ Pal{ bg:c(0x17,0x18,0x20), side:c(0x1B,0x1C,0x25), txt:c(0xD8,0xD8,0xE4),
        muted:c(0x87,0x88,0x98), faint:c(0x55,0x56,0x68), border:c(0x2B,0x2C,0x3A),
        hover:c(0x25,0x26,0x33), selr:c(0x2A,0x2A,0x4E), addbg:c(0x22,0x23,0x30), web:c(0x1C,0x1D,0x27),
        accent:c(0x86,0x86,0xF2), deleg:c(0xE0,0x82,0x95), topbar:c(0x1C,0x1D,0x2A) } }
}
fn area_color(pl:&Pal, ci:u8)->Color32{ match ci { 0=>pl.accent, 1=>pl.deleg, _=>c(0x6B,0xA8,0x8A) } }

struct Area { name: String, color: u8, open: bool }
struct Proj { name: String, area: usize, open: bool }
struct Tab {
    title: String, url: String, owner: u8, area: usize, proj: Option<usize>,
    browser: Option<Browser>, frame: SharedFrame, size: SharedSize, tex: Option<egui::TextureHandle>,
}
impl Tab {
    fn new(title:&str, url:&str, owner:u8, area:usize, proj:Option<usize>)->Self{
        Tab{ title:title.into(), url:url.into(), owner, area, proj,
            browser:None, frame:Rc::new(RefCell::new(FrameBuf::default())),
            size:Rc::new(RefCell::new((1100.0,720.0))), tex:None }
    }
}

fn mono(z:f32)->FontId{ FontId::new(z,FontFamily::Monospace) }
fn ico(g:&str, sz:f32)->RichText{ RichText::new(g).font(FontId::new(sz,FontFamily::Name("ico".into()))) }
// 탭/항목 행
fn item(ui:&mut Ui, pl:&Pal, icol:Color32, icon:&str, label:&str, sel:bool, indent:f32, running:bool)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w,31.0),Sense::click());
    let bg= if sel {pl.selr} else if resp.hovered(){pl.hover} else {Color32::TRANSPARENT};
    if bg!=Color32::TRANSPARENT { ui.painter().rect_filled(rect,Rounding::same(7.0),bg); }
    let p=ui.painter(); let ix=rect.left()+12.0+indent;
    if !icon.is_empty(){ p.text(Pos2::new(ix,rect.center().y),egui::Align2::LEFT_CENTER,icon,mono(11.0),icol); }
    p.text(Pos2::new(ix+19.0,rect.center().y),egui::Align2::LEFT_CENTER,label,mono(13.0),if sel{pl.accent}else{pl.txt});
    if running {
        let bx=rect.right()-44.0;
        p.rect_filled(egui::Rect::from_min_size(Pos2::new(bx,rect.center().y-8.0),vec2(38.0,16.0)),Rounding::same(8.0), if sel{pl.accent}else{icol});
        p.text(Pos2::new(bx+19.0,rect.center().y),egui::Align2::CENTER_CENTER,"run",mono(9.5),Color32::WHITE);
    }
    resp.clicked()
}
// 폴더(영역/프로젝트) 헤더 — 접기/펴기
fn folder(ui:&mut Ui, pl:&Pal, open:bool, icon:&str, icol:Color32, name:&str, indent:f32, header:bool)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w,if header{30.0}else{29.0}),Sense::click());
    if resp.hovered(){ ui.painter().rect_filled(rect,Rounding::same(7.0),pl.hover); }
    let p=ui.painter(); let x=rect.left()+10.0+indent;
    p.text(Pos2::new(x,rect.center().y),egui::Align2::LEFT_CENTER,if open{"▾"}else{"▸"},mono(10.0),pl.muted);
    p.text(Pos2::new(x+15.0,rect.center().y),egui::Align2::LEFT_CENTER,icon,mono(11.5),icol);
    let (fsz,fcol)= if header {(12.5, icol)} else {(13.0, pl.txt)};
    p.text(Pos2::new(x+34.0,rect.center().y),egui::Align2::LEFT_CENTER,name,mono(fsz),fcol);
    resp.clicked()
}
fn newbtn(ui:&mut Ui, pl:&Pal, label:&str, indent:f32)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w-4.0-indent,25.0),Sense::click());
    let rr=egui::Rect::from_min_size(Pos2::new(rect.left()+indent,rect.top()),vec2(rect.width(),rect.height())).shrink(1.0);
    dashed(ui,rr,if resp.hovered(){pl.muted}else{pl.faint});
    ui.painter().text(rr.center(),egui::Align2::CENTER_CENTER,format!("+ {label}"),mono(11.0),pl.muted); resp.clicked()
}
fn dashed(ui:&Ui,r:egui::Rect,col:Color32){ let (d,g)=(4.0,3.0); let p=ui.painter(); let st=Stroke::new(1.0,col);
    let mut x=r.left(); while x<r.right(){let e=(x+d).min(r.right()); p.line_segment([Pos2::new(x,r.top()),Pos2::new(e,r.top())],st); p.line_segment([Pos2::new(x,r.bottom()),Pos2::new(e,r.bottom())],st); x+=d+g; }
    let mut y=r.top(); while y<r.bottom(){let e=(y+d).min(r.bottom()); p.line_segment([Pos2::new(r.left(),y),Pos2::new(r.left(),e)],st); p.line_segment([Pos2::new(r.right(),y),Pos2::new(r.right(),e)],st); y+=d+g; }
}
fn subhead(ui:&mut Ui, _pl:&Pal, t:&str, col:Color32){ let w=ui.available_width(); let (r,_)=ui.allocate_exact_size(vec2(w,20.0),Sense::hover());
    ui.painter().text(Pos2::new(r.left()+10.0,r.center().y),egui::Align2::LEFT_CENTER,t,mono(10.5),col); }
fn divider(ui:&mut Ui, pl:&Pal){ let w=ui.available_width(); let (r,_)=ui.allocate_exact_size(vec2(w,1.0),Sense::hover()); ui.painter().line_segment([r.left_center(),r.right_center()],Stroke::new(1.0,pl.border)); }
fn tbtn(ui:&mut Ui, pl:&Pal, g:&str)->egui::Response{ ui.add(egui::Button::new(ico(g,13.0).color(pl.muted)).min_size(vec2(30.0,28.0)).rounding(Rounding::same(8.0)).fill(Color32::TRANSPARENT)) }
fn winbtn(ui:&mut Ui, pl:&Pal, g:&str, close:bool)->egui::Response{
    let (rect,resp)=ui.allocate_exact_size(vec2(36.0,30.0),Sense::click());
    if resp.hovered(){ let bg=if close {c(0xE0,0x50,0x50)} else {pl.hover}; ui.painter().rect_filled(rect,Rounding::same(6.0),bg); }
    let col=if close&&resp.hovered(){Color32::WHITE}else{pl.muted};
    ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,g,FontId::new(9.5,FontFamily::Name("ico".into())),col);
    resp
}
fn avatar(ui:&mut Ui, pl:&Pal){
    let (r,_)=ui.allocate_exact_size(vec2(46.0,28.0),Sense::click());
    ui.painter().text(Pos2::new(r.left()+7.0,r.center().y),egui::Align2::LEFT_CENTER,"1",mono(11.0),pl.muted);
    let cc=Pos2::new(r.right()-13.0,r.center().y);
    ui.painter().circle_filled(cc,11.0,pl.accent);
    ui.painter().text(cc,egui::Align2::CENTER_CENTER,"n",mono(12.5),Color32::WHITE);
}

// ───── CEF 스캐폴딩 ─────
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
            // 에이전트 제어용 CDP(Chrome DevTools Protocol) 포트 — 외부 CLI/AI가 탭 조종
            cl.append_switch_with_value(Some(&"remote-debugging-port".into()), Some(&"9222".into()));
            cl.append_switch_with_value(Some(&"remote-allow-origins".into()), Some(&"*".into()));
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
            let w=width as usize; let h=height as usize; let n=w*h*4;
            let src=unsafe{ std::slice::from_raw_parts(buffer,n) };
            let mut f=self.handler.frame.borrow_mut();
            if f.rgba.len()!=n { f.rgba.resize(n,0); }
            let dst=&mut f.rgba; let mut i=0;
            while i<n { dst[i]=src[i+2]; dst[i+1]=src[i+1]; dst[i+2]=src[i]; dst[i+3]=src[i+3]; i+=4; }
            f.w=w; f.h=h; f.dirty=true;
        }
    }
}
impl RenderHandlerBuilder { pub fn build(handler: NaruRender) -> RenderHandler { Self::new(handler) } }
wrap_client! {
    pub(crate) struct ClientBuilder { render_handler: RenderHandler, }
    impl Client { fn render_handler(&self) -> Option<cef::RenderHandler> { Some(self.render_handler.clone()) } }
}
impl ClientBuilder { pub(crate) fn build(render_handler: RenderHandler) -> cef::Client { Self::new(render_handler) } }

fn create_browser(url:&str, frame:SharedFrame, size:SharedSize) -> Browser {
    let render = NaruRender::new(frame, size, 1.0);
    let mut client = ClientBuilder::build(RenderHandlerBuilder::build(render));
    let wi = WindowInfo { windowless_rendering_enabled: true as _, ..Default::default() };
    let bs = BrowserSettings { windowless_frame_rate: 60, ..Default::default() };
    browser_host_create_browser_sync(Some(&wi), Some(&mut client), Some(&url.into()), Some(&bs), None, None)
        .expect("tab browser 생성 실패")
}

// ───── eframe 앱 ─────
struct Naru {
    areas: Vec<Area>, projs: Vec<Proj>, tabs: Vec<Tab>,
    active: usize, shown: usize, address: String, sidebar: bool, dark: bool,
}
impl Naru {
    fn ensure_browser(&mut self, ti:usize){
        if self.tabs[ti].browser.is_none() {
            let fr=self.tabs[ti].frame.clone(); let sz=self.tabs[ti].size.clone();
            let url=self.tabs[ti].url.clone();
            self.tabs[ti].browser = Some(create_browser(&url, fr, sz));
        }
    }
    fn load_active(&mut self, url:&str){
        let mut u=url.trim().to_string(); if u.is_empty(){return;}
        if !u.contains("://") && !u.starts_with("naru:") { u=format!("https://{u}"); }
        if let Some(b)=&self.tabs[self.active].browser { if let Some(fr)=b.main_frame(){ fr.load_url(Some(&u.as_str().into())); } }
        self.tabs[self.active].url=u;
    }
}

impl eframe::App for Naru {
    fn update(&mut self, ctx:&egui::Context, _ef:&mut eframe::Frame){
        do_message_loop_work();
        let act=self.active;
        self.ensure_browser(act);

        // 탭 전환: 비활성 숨김 + 활성 표시
        if self.shown != act {
            for (i,t) in self.tabs.iter().enumerate() {
                if i!=act { if let Some(b)=&t.browser { if let Some(h)=b.host(){ h.was_hidden(1); } } }
            }
            if let Some(b)=&self.tabs[act].browser { if let Some(h)=b.host(){ h.was_hidden(0); h.was_resized(); } }
            self.shown=act; self.address=self.tabs[act].url.clone();
        }

        // 활성 탭 프레임 업로드
        let mut got_frame=false;
        { let frame=self.tabs[act].frame.clone(); let mut f=frame.borrow_mut();
          if f.dirty && f.w>0 && f.h>0 {
              let img=egui::ColorImage::from_rgba_unmultiplied([f.w,f.h],&f.rgba);
              match &mut self.tabs[act].tex { Some(t)=>t.set(img,egui::TextureOptions::LINEAR),
                  None=>self.tabs[act].tex=Some(ctx.load_texture("web",img,egui::TextureOptions::LINEAR)) }
              f.dirty=false; got_frame=true;
          } }

        let pl = if self.dark { Pal::dark() } else { Pal::light() };
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill=pl.bg; v.window_fill=pl.bg; v.override_text_color=Some(pl.txt);
        v.selection.bg_fill=pl.selr; v.selection.stroke=Stroke::NONE; v.text_cursor.stroke.color=pl.accent;
        ctx.set_visuals(v);
        let bord=Stroke::new(1.0,pl.border);

        // ===== 상단바 =====
        egui::TopBottomPanel::top("top").frame(Frame::none().fill(pl.topbar).inner_margin(Margin::symmetric(10.0,8.0)).stroke(bord)).show(ctx,|ui|{
            let tr=ui.max_rect().expand2(vec2(10.0,10.0));
            let a_top=Color32::from_rgba_unmultiplied(pl.accent.r(),pl.accent.g(),pl.accent.b(), if self.dark {26} else {20});
            let a_bot=Color32::from_rgba_unmultiplied(pl.accent.r(),pl.accent.g(),pl.accent.b(), 0);
            let mut m=egui::epaint::Mesh::default(); let uv=egui::epaint::WHITE_UV;
            for (pos,col) in [(tr.left_top(),a_top),(tr.right_top(),a_top),(tr.right_bottom(),a_bot),(tr.left_bottom(),a_bot)]{
                m.vertices.push(egui::epaint::Vertex{pos,uv,color:col}); }
            m.indices.extend_from_slice(&[0,1,2,0,2,3]); ui.painter().add(egui::Shape::mesh(m));
            let drag=ui.interact(ui.max_rect(), egui::Id::new("tbar"), Sense::click_and_drag());
            if drag.double_clicked(){ let mx=ctx.input(|i| i.viewport().maximized.unwrap_or(false)); ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!mx)); }
            if drag.drag_started_by(egui::PointerButton::Primary){ ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); }
            ui.horizontal(|ui|{
                ui.spacing_mut().item_spacing.x=2.0;
                if tbtn(ui,&pl,"\u{E700}").clicked(){ self.sidebar=!self.sidebar; }
                if tbtn(ui,&pl,"\u{E72B}").clicked(){ if let Some(b)=&self.tabs[act].browser { b.go_back(); } }
                if tbtn(ui,&pl,"\u{E72A}").clicked(){ if let Some(b)=&self.tabs[act].browser { b.go_forward(); } }
                if tbtn(ui,&pl,"\u{E72C}").clicked(){ if let Some(b)=&self.tabs[act].browser { b.reload(); } }
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Center),|ui|{
                    ui.spacing_mut().item_spacing.x=0.0;
                    if winbtn(ui,&pl,"\u{E8BB}",true).clicked(){ ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                    if winbtn(ui,&pl,"\u{E922}",false).clicked(){ let mx=ctx.input(|i| i.viewport().maximized.unwrap_or(false)); ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!mx)); }
                    if winbtn(ui,&pl,"\u{E921}",false).clicked(){ ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)); }
                    ui.add_space(10.0); avatar(ui,&pl); ui.add_space(6.0);
                    if tbtn(ui,&pl,if self.dark{"\u{E706}"}else{"\u{E708}"}).clicked(){ self.dark=!self.dark; }
                    ui.add_space(10.0);
                    Frame::none().fill(pl.addbg).rounding(Rounding::same(16.0)).stroke(bord).inner_margin(Margin::symmetric(14.0,7.0)).show(ui,|ui|{
                        ui.horizontal(|ui|{
                            ui.label(ico("\u{E721}",11.5).color(pl.muted));
                            let r=ui.add(egui::TextEdit::singleline(&mut self.address).frame(false).text_color(pl.txt)
                                .desired_width(ui.available_width()-24.0)
                                .hint_text(RichText::new("search or type a url · 검색 또는 주소").color(pl.faint)));
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { self.load_active(&self.address.clone()); }
                            ui.label(ico("\u{E734}",11.5).color(pl.muted));
                        });
                    });
                });
            });
        });

        // ===== 사이드바 (영역→프로젝트→탭) =====
        let mut switch_to:Option<usize>=None;
        let mut toggle_area:Option<usize>=None;
        let mut toggle_proj:Option<usize>=None;
        let mut new_tab_area:Option<usize>=None;
        let mut new_space=false;
        if self.sidebar {
            egui::SidePanel::left("side").resizable(false).exact_width(256.0)
                .frame(Frame::none().fill(pl.side).inner_margin(Margin::symmetric(8.0,10.0)).stroke(bord)).show(ctx,|ui|{
                let (hr,_)=ui.allocate_exact_size(vec2(ui.available_width(),28.0),Sense::hover()); let p=ui.painter();
                p.text(Pos2::new(hr.left()+12.0,hr.center().y),egui::Align2::LEFT_CENTER,"⌂ naru",mono(13.5),pl.txt);
                p.text(Pos2::new(hr.right()-10.0,hr.center().y),egui::Align2::RIGHT_CENTER,"▢",mono(13.0),pl.muted);
                ui.add_space(4.0);
                egui::ScrollArea::vertical().auto_shrink([false,false]).show(ui,|ui|{
                    // 제로셋업: 스페이스 1개면 계층 숨기고 flat 탭 목록. 2개+ 면 트리 공개.
                    let multi = self.areas.len() > 1;
                    if multi { subhead(ui,&pl,"spaces",pl.faint); }
                    let ind = if multi {16.0} else {0.0};
                    for ai in 0..self.areas.len() {
                        let acol=area_color(&pl,self.areas[ai].color);
                        if multi {
                            let aopen=self.areas[ai].open;
                            if folder(ui,&pl,aopen,"⌂",acol,&self.areas[ai].name.clone(),0.0,true){ toggle_area=Some(ai); }
                            if !aopen { continue; }
                        }
                        // 영역 직속 탭 (proj=None)
                        for ti in 0..self.tabs.len() {
                            if self.tabs[ti].area!=ai || self.tabs[ti].proj.is_some() { continue; }
                            let sel=ti==self.active; let agent=self.tabs[ti].owner!=0;
                            let icol=if agent{acol}else if sel{pl.accent}else{pl.faint};
                            if item(ui,&pl,icol,if agent{"●"}else{"○"},&self.tabs[ti].title.clone(),sel,ind,agent && sel){ switch_to=Some(ti); }
                        }
                        // 프로젝트들 (있을 때만)
                        for pi in 0..self.projs.len() {
                            if self.projs[pi].area!=ai { continue; }
                            let popen=self.projs[pi].open;
                            if folder(ui,&pl,popen,"\u{E8B7}",pl.muted,&self.projs[pi].name.clone(),ind,false){ toggle_proj=Some(pi); }
                            if !popen { continue; }
                            for ti in 0..self.tabs.len() {
                                if self.tabs[ti].proj!=Some(pi) { continue; }
                                let sel=ti==self.active; let agent=self.tabs[ti].owner!=0;
                                let icol=if agent{acol}else if sel{pl.accent}else{pl.faint};
                                if item(ui,&pl,icol,if agent{"●"}else{"○"},&self.tabs[ti].title.clone(),sel,ind+20.0,agent && sel){ switch_to=Some(ti); }
                            }
                        }
                        if newbtn(ui,&pl,"new tab",ind){ new_tab_area=Some(ai); }
                        ui.add_space(if multi {4.0} else {2.0});
                    }
                    ui.add_space(4.0);
                    if newbtn(ui,&pl,"new space",0.0){ new_space=true; }
                    ui.add_space(8.0);
                    subhead(ui,&pl,"library",pl.faint);
                    item(ui,&pl,pl.muted,"↺","history",false,0.0,false);
                    item(ui,&pl,pl.accent,"↓","downloads",false,0.0,false);
                    item(ui,&pl,pl.deleg,"▤","reading list",false,0.0,false);
                    ui.add_space(8.0); divider(ui,&pl); ui.add_space(4.0);
                    item(ui,&pl,pl.muted,"◉","account",false,0.0,false);
                    item(ui,&pl,pl.muted,"◴","settings",false,0.0,false); ui.add_space(4.0);
                });
            });
        }
        if let Some(ai)=toggle_area { self.areas[ai].open=!self.areas[ai].open; }
        if let Some(pi)=toggle_proj { self.projs[pi].open=!self.projs[pi].open; }
        if let Some(ti)=switch_to { self.active=ti; }
        if let Some(ai)=new_tab_area {
            self.tabs.push(Tab::new("new tab","https://duckduckgo.com",0,ai,None));
            self.active=self.tabs.len()-1;
        }
        if new_space {
            let n=self.areas.len();
            self.areas.push(Area{name:format!("space {}",n+1), color:(n%3) as u8, open:true});
        }

        // ===== 중앙 웹뷰 =====
        egui::CentralPanel::default().frame(Frame::none().fill(pl.bg).inner_margin(Margin::same(12.0))).show(ctx,|ui|{
            Frame::none().fill(pl.web).rounding(Rounding::same(10.0)).stroke(bord).show(ui,|ui|{
                let avail=ui.available_size();
                let mut changed=false;
                { let mut s=self.tabs[act].size.borrow_mut(); let nw=avail.x.max(1.0); let nh=avail.y.max(1.0);
                  if (s.0-nw).abs()>0.5||(s.1-nh).abs()>0.5 { *s=(nw,nh); changed=true; } }
                if changed { if let Some(b)=&self.tabs[act].browser { if let Some(h)=b.host(){ h.was_resized(); } } }
                if let Some(t)=&self.tabs[act].tex {
                    let resp=ui.add(egui::Image::new((t.id(),avail)).sense(egui::Sense::click_and_drag()));
                    let rect=resp.rect;
                    if let Some(b)=&self.tabs[act].browser { if let Some(host)=b.host() {
                        ctx.input(|i|{ if let Some(pos)=i.pointer.latest_pos(){ if rect.contains(pos){
                            let x=(pos.x-rect.left()).round() as i32; let y=(pos.y-rect.top()).round() as i32;
                            const LBTN:u32=16; let md=if i.pointer.primary_down(){LBTN}else{0};
                            let ev=MouseEvent{x,y,modifiers:md}; host.send_mouse_move_event(Some(&ev),0);
                            if i.pointer.primary_pressed(){ let e=MouseEvent{x,y,modifiers:LBTN}; host.send_mouse_click_event(Some(&e),MouseButtonType::LEFT,0,1); }
                            if i.pointer.primary_released(){ let e=MouseEvent{x,y,modifiers:0}; host.send_mouse_click_event(Some(&e),MouseButtonType::LEFT,1,1); }
                            let sc=i.raw_scroll_delta; if sc.x!=0.0||sc.y!=0.0 { host.send_mouse_wheel_event(Some(&ev),(sc.x*3.0) as i32,(sc.y*3.0) as i32); }
                        }} });
                    } }
                } else {
                    ui.vertical_centered(|ui|{ ui.add_space(ui.available_height()*0.44);
                        ui.label(RichText::new("naru:// 로딩 중…").color(pl.muted).size(14.0)); });
                }
            });
        });

        // ===== 에이전트 컨트롤 pill =====
        if self.tabs[act].owner != 0 {
            let title=self.tabs[act].title.clone();
            let (pb,pt,tb,tt,pbord)= if self.dark {
                (c(0x24,0x25,0x3A), c(0xEE,0xEE,0xF6), c(0x3D,0x3E,0x66), c(0xCC,0xCC,0xDA), Stroke::NONE)
            } else { (c(0xFF,0xFF,0xFF), c(0x2B,0x2B,0x36), c(0xF0,0xF0,0xF9), pl.accent, Stroke::new(1.0,pl.border)) };
            egui::Area::new(egui::Id::new("agent_pill")).anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0,-26.0)).show(ctx,|ui|{
                Frame::none().fill(pb).stroke(pbord).rounding(Rounding::same(22.0)).inner_margin(Margin::symmetric(16.0,9.0))
                    .shadow(egui::epaint::Shadow{offset:vec2(0.0,5.0),blur:20.0,spread:0.0,color:Color32::from_black_alpha(70)})
                    .show(ui,|ui|{ ui.horizontal(|ui|{
                        let (rr,_)=ui.allocate_exact_size(vec2(12.0,12.0),Sense::hover());
                        ui.painter().circle_filled(rr.center(),4.5,pl.accent);
                        ui.label(RichText::new(title).size(12.5).color(pt).strong());
                        ui.label(RichText::new("· agent in control").size(12.0).color(pl.accent));
                        ui.add_space(10.0);
                        let _=ui.add(egui::Button::new(RichText::new("take over").size(11.5).color(tt)).fill(tb).rounding(Rounding::same(13.0)).min_size(vec2(0.0,24.0)));
                        let _=ui.add(egui::Button::new(RichText::new("stop").size(11.5).color(Color32::WHITE)).fill(c(0xCF,0x57,0x57)).rounding(Rounding::same(13.0)).min_size(vec2(0.0,24.0)));
                    }); });
            });
        }

        let next_ms = if got_frame { 16 } else { 66 };
        ctx.request_repaint_after(Duration::from_millis(next_ms));
    }
}

fn setup_ui(cc: &eframe::CreationContext<'_>) {
    let mut f = egui::FontDefinitions::default();
    for p in ["C:/Windows/Fonts/CascadiaMono.ttf","C:/Windows/Fonts/consola.ttf"]{
        if let Ok(b)=std::fs::read(p){ f.font_data.insert("mono".into(),egui::FontData::from_owned(b));
            f.families.entry(FontFamily::Monospace).or_default().insert(0,"mono".into());
            f.families.entry(FontFamily::Proportional).or_default().insert(0,"mono".into()); break; } }
    if let Ok(b)=std::fs::read("C:/Windows/Fonts/malgun.ttf"){ f.font_data.insert("kr".into(),egui::FontData::from_owned(b));
        f.families.entry(FontFamily::Monospace).or_default().push("kr".into());
        f.families.entry(FontFamily::Proportional).or_default().push("kr".into()); }
    for p in ["C:/Windows/Fonts/SegoeIcons.ttf","C:/Windows/Fonts/SegMDL2.ttf"]{
        if let Ok(b)=std::fs::read(p){ f.font_data.insert("ico".into(),egui::FontData::from_owned(b));
            f.families.insert(FontFamily::Name("ico".into()), vec!["ico".into()]); break; } }
    cc.egui_ctx.set_fonts(f);
    let mut s=(*cc.egui_ctx.style()).clone();
    for (t,z) in [(TextStyle::Heading,15.0),(TextStyle::Body,13.0),(TextStyle::Button,13.0),
                  (TextStyle::Small,11.0),(TextStyle::Monospace,13.0)]{ s.text_styles.insert(t,FontId::new(z,FontFamily::Monospace)); }
    s.spacing.item_spacing=vec2(6.0,3.0); s.spacing.window_margin=Margin::ZERO;
    cc.egui_ctx.set_style(s);
}

fn main() -> std::process::ExitCode {
    std::panic::set_hook(Box::new(|info| { mark(&format!("PANIC: {}", info)); }));
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let cmd = args.as_cmd_line().unwrap();
    let switch = CefString::from("type");
    let is_browser_process = cmd.has_switch(Some(&switch)) != 1;
    let mut app = AppBuilder::build(NaruApp::new());
    let ret = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    if !is_browser_process { return 0.into(); }
    assert!(ret == -1, "cannot execute browser process");
    let settings = Settings {
        windowless_rendering_enabled: true as _, external_message_pump: true as _,
        root_cache_path: "C:\\Users\\Lenovo\\AppData\\Local\\Temp\\naru_cache".into(), ..Default::default() };
    assert_eq!(initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()), 1);
    for _ in 0..10 { do_message_loop_work(); }

    // 깡통 시작 상태: 영역 1개 + 탭 1개. 나머지는 유저가 new space/new tab로 생성.
    let areas = vec![ Area{name:"personal".into(), color:0, open:true} ];
    let projs: Vec<Proj> = Vec::new();
    let tabs = vec![ Tab::new("new tab","https://duckduckgo.com",0,0,None) ];

    let app_ui = Naru { areas, projs, tabs, active:0, shown:usize::MAX, address:"duckduckgo.com".into(), sidebar:true, dark:false };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0,860.0]).with_min_inner_size([920.0,600.0])
            .with_title("Naru").with_decorations(false), ..Default::default() };
    let _ = eframe::run_native("Naru", native, Box::new(move |cc| { setup_ui(cc); Ok(Box::new(app_ui)) }));
    shutdown();
    0.into()
}
