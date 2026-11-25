// UI — 인디고 팔레트 + 사이드바/상단바 위젯들 (순수 그리기, 상태 없음)
use eframe::egui::{self, vec2, Color32, FontFamily, FontId, Pos2, RichText, Rounding, Sense, Stroke, Ui};

#[derive(Clone, Copy)]
pub struct Pal { pub bg:Color32, pub side:Color32, pub txt:Color32, pub muted:Color32, pub faint:Color32, pub border:Color32,
    pub hover:Color32, pub selr:Color32, pub addbg:Color32, pub web:Color32, pub accent:Color32, pub deleg:Color32, pub topbar:Color32 }
pub fn c(r:u8,g:u8,b:u8)->Color32{ Color32::from_rgb(r,g,b) }
impl Pal {
    pub fn light()->Self{ Pal{ bg:c(0xF9,0xF9,0xFC), side:c(0xFC,0xFC,0xFE), txt:c(0x2B,0x2B,0x36),
        muted:c(0x8C,0x8C,0x9C), faint:c(0xBE,0xBE,0xCC), border:c(0xE9,0xE9,0xF1),
        hover:c(0xF0,0xF0,0xF7), selr:c(0xEC,0xEC,0xFB), addbg:c(0xF2,0xF2,0xF8), web:c(0xFC,0xFC,0xFE),
        accent:c(0x5B,0x5B,0xD6), deleg:c(0xC8,0x5C,0x6E), topbar:c(0xF4,0xF4,0xFC) } }
    pub fn dark()->Self{ Pal{ bg:c(0x17,0x18,0x20), side:c(0x1B,0x1C,0x25), txt:c(0xD8,0xD8,0xE4),
        muted:c(0x87,0x88,0x98), faint:c(0x55,0x56,0x68), border:c(0x2B,0x2C,0x3A),
        hover:c(0x25,0x26,0x33), selr:c(0x2A,0x2A,0x4E), addbg:c(0x22,0x23,0x30), web:c(0x1C,0x1D,0x27),
        accent:c(0x86,0x86,0xF2), deleg:c(0xE0,0x82,0x95), topbar:c(0x1C,0x1D,0x2A) } }
}
pub fn area_color(pl:&Pal, ci:u8)->Color32{ match ci { 0=>pl.accent, 1=>pl.deleg, _=>c(0x6B,0xA8,0x8A) } }

pub fn mono(z:f32)->FontId{ FontId::new(z,FontFamily::Monospace) }
pub fn ico(g:&str, sz:f32)->RichText{ RichText::new(g).font(FontId::new(sz,FontFamily::Name("ico".into()))) }
// 탭/항목 행 — (클릭됨, 닫기×클릭됨) 반환
pub fn item(ui:&mut Ui, pl:&Pal, icol:Color32, icon:&str, label:&str, sel:bool, indent:f32, running:bool, closable:bool)->(bool,bool){
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w,31.0),Sense::click());
    let bg= if sel {pl.selr} else if resp.hovered(){pl.hover} else {Color32::TRANSPARENT};
    if bg!=Color32::TRANSPARENT { ui.painter().rect_filled(rect,Rounding::same(7.0),bg); }
    let p=ui.painter(); let ix=rect.left()+12.0+indent;
    if !icon.is_empty(){ p.text(Pos2::new(ix,rect.center().y),egui::Align2::LEFT_CENTER,icon,mono(11.0),icol); }
    p.text(Pos2::new(ix+19.0,rect.center().y),egui::Align2::LEFT_CENTER,label,mono(13.0),if sel{pl.accent}else{pl.txt});
    let mut close=false;
    if running {
        let bx=rect.right()-44.0;
        p.rect_filled(egui::Rect::from_min_size(Pos2::new(bx,rect.center().y-8.0),vec2(38.0,16.0)),Rounding::same(8.0), if sel{pl.accent}else{icol});
        p.text(Pos2::new(bx+19.0,rect.center().y),egui::Align2::CENTER_CENTER,"run",mono(9.5),Color32::WHITE);
    } else if closable && resp.contains_pointer() {
        // hover 시 닫기 × — 겹치는 위젯 없이, 행 클릭 위치가 × 영역 안인지로 판정(hover 뺏김/깜빡임 회피)
        let cc=Pos2::new(rect.right()-16.0, rect.center().y);
        let cr=egui::Rect::from_center_size(cc, vec2(20.0,20.0));
        let hot=ui.rect_contains_pointer(cr);
        if hot { ui.painter().rect_filled(cr,Rounding::same(6.0),pl.border); }
        ui.painter().text(cc,egui::Align2::CENTER_CENTER,"✕",mono(11.0),if hot{pl.txt}else{pl.muted});
        if resp.clicked() { if let Some(pp)=resp.interact_pointer_pos(){ if cr.contains(pp){ close=true; } } }
    }
    (resp.clicked() && !close, close)
}
// 폴더(영역/프로젝트) 헤더 — 접기/펴기
pub fn folder(ui:&mut Ui, pl:&Pal, open:bool, icon:&str, icol:Color32, name:&str, indent:f32, header:bool)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w,if header{30.0}else{29.0}),Sense::click());
    if resp.hovered(){ ui.painter().rect_filled(rect,Rounding::same(7.0),pl.hover); }
    let p=ui.painter(); let x=rect.left()+10.0+indent;
    p.text(Pos2::new(x,rect.center().y),egui::Align2::LEFT_CENTER,if open{"▾"}else{"▸"},mono(10.0),pl.muted);
    p.text(Pos2::new(x+15.0,rect.center().y),egui::Align2::LEFT_CENTER,icon,mono(11.5),icol);
    let (fsz,fcol)= if header {(12.5, icol)} else {(13.0, pl.txt)};
    p.text(Pos2::new(x+34.0,rect.center().y),egui::Align2::LEFT_CENTER,name,mono(fsz),fcol);
    resp.clicked()
}
pub fn newbtn(ui:&mut Ui, pl:&Pal, label:&str, indent:f32)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w-4.0-indent,25.0),Sense::click());
    let rr=egui::Rect::from_min_size(Pos2::new(rect.left()+indent,rect.top()),vec2(rect.width(),rect.height())).shrink(1.0);
    dashed(ui,rr,if resp.hovered(){pl.muted}else{pl.faint});
    ui.painter().text(rr.center(),egui::Align2::CENTER_CENTER,format!("+ {label}"),mono(11.0),pl.muted); resp.clicked()
}
fn dashed(ui:&Ui,r:egui::Rect,col:Color32){ let (d,g)=(4.0,3.0); let p=ui.painter(); let st=Stroke::new(1.0,col);
    let mut x=r.left(); while x<r.right(){let e=(x+d).min(r.right()); p.line_segment([Pos2::new(x,r.top()),Pos2::new(e,r.top())],st); p.line_segment([Pos2::new(x,r.bottom()),Pos2::new(e,r.bottom())],st); x+=d+g; }
    let mut y=r.top(); while y<r.bottom(){let e=(y+d).min(r.bottom()); p.line_segment([Pos2::new(r.left(),y),Pos2::new(r.left(),e)],st); p.line_segment([Pos2::new(r.right(),y),Pos2::new(r.right(),e)],st); y+=d+g; }
}
pub fn subhead(ui:&mut Ui, _pl:&Pal, t:&str, col:Color32){ let w=ui.available_width(); let (r,_)=ui.allocate_exact_size(vec2(w,20.0),Sense::hover());
    ui.painter().text(Pos2::new(r.left()+10.0,r.center().y),egui::Align2::LEFT_CENTER,t,mono(10.5),col); }
pub fn divider(ui:&mut Ui, pl:&Pal){ let w=ui.available_width(); let (r,_)=ui.allocate_exact_size(vec2(w,1.0),Sense::hover()); ui.painter().line_segment([r.left_center(),r.right_center()],Stroke::new(1.0,pl.border)); }
pub fn tbtn(ui:&mut Ui, pl:&Pal, g:&str)->egui::Response{ ui.add(egui::Button::new(ico(g,13.0).color(pl.muted)).min_size(vec2(30.0,28.0)).rounding(Rounding::same(8.0)).fill(Color32::TRANSPARENT)) }
pub fn winbtn(ui:&mut Ui, pl:&Pal, g:&str, close:bool)->egui::Response{
    let (rect,resp)=ui.allocate_exact_size(vec2(36.0,30.0),Sense::click());
    if resp.hovered(){ let bg=if close {c(0xE0,0x50,0x50)} else {pl.hover}; ui.painter().rect_filled(rect,Rounding::same(6.0),bg); }
    let col=if close&&resp.hovered(){Color32::WHITE}else{pl.muted};
    ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,g,FontId::new(9.5,FontFamily::Name("ico".into())),col);
    resp
}
pub fn avatar(ui:&mut Ui, pl:&Pal){
    let (r,_)=ui.allocate_exact_size(vec2(46.0,28.0),Sense::click());
    ui.painter().text(Pos2::new(r.left()+7.0,r.center().y),egui::Align2::LEFT_CENTER,"1",mono(11.0),pl.muted);
    let cc=Pos2::new(r.right()-13.0,r.center().y);
    ui.painter().circle_filled(cc,11.0,pl.accent);
    ui.painter().text(cc,egui::Align2::CENTER_CENTER,"n",mono(12.5),Color32::WHITE);
}
