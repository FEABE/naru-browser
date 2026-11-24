// Naru 크롬 UI 목업 v6 — 인디고/바이올렛(쿨) 팔레트 + 라이트/다크 토글.
// 형태=init.Tasks, 내용=브라우저(spaces/tabs/bookmarks/library). 하단 에이전트 바 토글.

use eframe::egui;
use egui::{vec2, Align, Color32, FontFamily, FontId, Frame, Layout, Margin, Pos2, Rect,
           RichText, Rounding, Sense, Stroke, TextStyle, Ui};

#[derive(Clone,Copy)]
struct Pal { bg:Color32, side:Color32, txt:Color32, muted:Color32, faint:Color32, border:Color32,
    hover:Color32, selr:Color32, addbg:Color32, web:Color32, accent:Color32, deleg:Color32, topbar:Color32 }
fn c(r:u8,g:u8,b:u8)->Color32{ Color32::from_rgb(r,g,b) }
impl Pal {
    fn light()->Self{ Pal{ bg:c(0xF9,0xF9,0xFC), side:c(0xFC,0xFC,0xFE), txt:c(0x2B,0x2B,0x36),
        muted:c(0x8C,0x8C,0x9C), faint:c(0xBE,0xBE,0xCC), border:c(0xE9,0xE9,0xF1),
        hover:c(0xF0,0xF0,0xF7), selr:c(0xEC,0xEC,0xFB), addbg:c(0xF2,0xF2,0xF8), web:c(0xFC,0xFC,0xFE),
        accent:c(0x5B,0x5B,0xD6), deleg:c(0xA8,0x6C,0xDB), topbar:c(0xF4,0xF4,0xFC) } }
    fn dark()->Self{ Pal{ bg:c(0x17,0x18,0x20), side:c(0x1B,0x1C,0x25), txt:c(0xD8,0xD8,0xE4),
        muted:c(0x87,0x88,0x98), faint:c(0x55,0x56,0x68), border:c(0x2B,0x2C,0x3A),
        hover:c(0x25,0x26,0x33), selr:c(0x2A,0x2A,0x4E), addbg:c(0x22,0x23,0x30), web:c(0x1C,0x1D,0x27),
        accent:c(0x86,0x86,0xF2), deleg:c(0xB1,0x85,0xE6), topbar:c(0x1C,0x1D,0x2A) } }
}

struct Tab { title:String, url:String, owner:u8 }
struct App { address:String, sidebar:bool, agent_open:bool, dark:bool, sel:usize, tabs:Vec<Tab> }

impl App {
    fn new(cc:&eframe::CreationContext<'_>)->Self{
        let mut f=egui::FontDefinitions::default();
        for p in ["C:/Windows/Fonts/CascadiaMono.ttf","C:/Windows/Fonts/consola.ttf"]{
            if let Ok(b)=std::fs::read(p){ f.font_data.insert("mono".into(),egui::FontData::from_owned(b));
                f.families.entry(FontFamily::Monospace).or_default().insert(0,"mono".into());
                f.families.entry(FontFamily::Proportional).or_default().insert(0,"mono".into()); break; } }
        if let Ok(b)=std::fs::read("C:/Windows/Fonts/malgun.ttf"){ f.font_data.insert("kr".into(),egui::FontData::from_owned(b));
            f.families.entry(FontFamily::Monospace).or_default().push("kr".into());
            f.families.entry(FontFamily::Proportional).or_default().push("kr".into()); }
        cc.egui_ctx.set_fonts(f);
        let mut s=(*cc.egui_ctx.style()).clone();
        for (t,z) in [(TextStyle::Heading,15.0),(TextStyle::Body,13.0),(TextStyle::Button,13.0),
                      (TextStyle::Small,11.0),(TextStyle::Monospace,13.0)]{ s.text_styles.insert(t,FontId::new(z,FontFamily::Monospace)); }
        s.spacing.item_spacing=vec2(6.0,3.0); s.spacing.window_margin=Margin::ZERO;
        cc.egui_ctx.set_style(s);
        Self{ address:"aside.com".into(), sidebar:true, agent_open:true, dark:false, sel:0,
            tabs:vec![
                Tab{title:"new tab".into(),url:"naru://start".into(),owner:0},
                Tab{title:"inbox · gmail".into(),url:"mail.google.com".into(),owner:0},
                Tab{title:"docs · figma".into(),url:"figma.com".into(),owner:0},
                Tab{title:"research · sf parks".into(),url:"google.com".into(),owner:1},
                Tab{title:"update sheet".into(),url:"docs.google.com".into(),owner:2},
            ] }
    }
}

fn mono(z:f32)->FontId{ FontId::new(z,FontFamily::Monospace) }
fn row(ui:&mut Ui, pl:&Pal, icol:Color32, icon:&str, label:&str, count:Option<i32>, sel:bool)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w,30.0),Sense::click());
    let bg= if sel {pl.selr} else if resp.hovered(){pl.hover} else {Color32::TRANSPARENT};
    if bg!=Color32::TRANSPARENT { ui.painter().rect_filled(rect,Rounding::same(7.0),bg); }
    let p=ui.painter();
    if !icon.is_empty(){ p.text(Pos2::new(rect.left()+12.0,rect.center().y),egui::Align2::LEFT_CENTER,icon,mono(11.0),icol); }
    p.text(Pos2::new(rect.left()+32.0,rect.center().y),egui::Align2::LEFT_CENTER,label,mono(13.0),if sel{pl.accent}else{pl.txt});
    if let Some(n)=count{ p.text(Pos2::new(rect.right()-10.0,rect.center().y),egui::Align2::RIGHT_CENTER,n.to_string(),mono(12.0),pl.faint); }
    resp.clicked()
}
fn sec(ui:&mut Ui, pl:&Pal, name:&str){
    let w=ui.available_width(); let (rect,_)=ui.allocate_exact_size(vec2(w,30.0),Sense::hover()); let p=ui.painter();
    p.text(Pos2::new(rect.left()+12.0,rect.center().y),egui::Align2::LEFT_CENTER,"▸",mono(10.0),pl.faint);
    p.text(Pos2::new(rect.left()+32.0,rect.center().y),egui::Align2::LEFT_CENTER,name,mono(13.0),pl.txt);
    p.text(Pos2::new(rect.right()-10.0,rect.center().y),egui::Align2::RIGHT_CENTER,"+",mono(14.0),pl.muted);
}
fn newbtn(ui:&mut Ui, pl:&Pal, label:&str)->bool{
    let w=ui.available_width(); let (rect,resp)=ui.allocate_exact_size(vec2(w-4.0,28.0),Sense::click());
    let rr=rect.shrink(1.0); dashed(ui,rr,if resp.hovered(){pl.muted}else{pl.faint});
    ui.painter().text(rr.center(),egui::Align2::CENTER_CENTER,format!("+ {label}"),mono(12.0),pl.muted); resp.clicked()
}
fn dashed(ui:&Ui,r:Rect,col:Color32){ let (d,g)=(4.0,3.0); let p=ui.painter(); let st=Stroke::new(1.0,col);
    let mut x=r.left(); while x<r.right(){let e=(x+d).min(r.right()); p.line_segment([Pos2::new(x,r.top()),Pos2::new(e,r.top())],st); p.line_segment([Pos2::new(x,r.bottom()),Pos2::new(e,r.bottom())],st); x+=d+g; }
    let mut y=r.top(); while y<r.bottom(){let e=(y+d).min(r.bottom()); p.line_segment([Pos2::new(r.left(),y),Pos2::new(r.left(),e)],st); p.line_segment([Pos2::new(r.right(),y),Pos2::new(r.right(),e)],st); y+=d+g; }
}
fn subhead(ui:&mut Ui, pl:&Pal, t:&str, col:Color32){ let w=ui.available_width(); let (r,_)=ui.allocate_exact_size(vec2(w,22.0),Sense::hover());
    ui.painter().text(Pos2::new(r.left()+12.0,r.center().y),egui::Align2::LEFT_CENTER,t,mono(10.5),col); }
fn divider(ui:&mut Ui, pl:&Pal){ let w=ui.available_width(); let (r,_)=ui.allocate_exact_size(vec2(w,1.0),Sense::hover()); ui.painter().line_segment([r.left_center(),r.right_center()],Stroke::new(1.0,pl.border)); }
fn tbtn(ui:&mut Ui, pl:&Pal, g:&str)->egui::Response{ ui.add(egui::Button::new(RichText::new(g).size(14.0).color(pl.muted)).min_size(vec2(28.0,28.0)).rounding(Rounding::same(8.0)).fill(Color32::TRANSPARENT)) }
fn avatar(ui:&mut Ui, pl:&Pal){
    let (r,_)=ui.allocate_exact_size(vec2(46.0,28.0),Sense::click());
    ui.painter().text(Pos2::new(r.left()+7.0,r.center().y),egui::Align2::LEFT_CENTER,"1",mono(11.0),pl.muted);
    let cc=Pos2::new(r.right()-13.0,r.center().y);
    ui.painter().circle_filled(cc,11.0,pl.accent);
    ui.painter().text(cc,egui::Align2::CENTER_CENTER,"n",mono(12.5),Color32::WHITE);
}

impl eframe::App for App{
    fn update(&mut self,ctx:&egui::Context,_f:&mut eframe::Frame){
        let pl = if self.dark { Pal::dark() } else { Pal::light() };
        // visuals 매 프레임 적용
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill=pl.bg; v.window_fill=pl.bg; v.override_text_color=Some(pl.txt);
        v.selection.bg_fill=pl.selr; v.selection.stroke=Stroke::NONE; v.text_cursor.stroke.color=pl.accent;
        ctx.set_visuals(v);
        let bord=Stroke::new(1.0,pl.border);

        egui::TopBottomPanel::top("top").frame(Frame::none().fill(pl.topbar).inner_margin(Margin::symmetric(10.0,8.0)).stroke(bord)).show(ctx,|ui|{
            // 앰비언트 인디고 글로우
            let tr=ui.max_rect().expand2(vec2(10.0,10.0));
            let a_top=Color32::from_rgba_unmultiplied(pl.accent.r(),pl.accent.g(),pl.accent.b(), if self.dark {26} else {20});
            let a_bot=Color32::from_rgba_unmultiplied(pl.accent.r(),pl.accent.g(),pl.accent.b(), 0);
            let mut m=egui::epaint::Mesh::default(); let uv=egui::epaint::WHITE_UV;
            for (pos,col) in [(tr.left_top(),a_top),(tr.right_top(),a_top),(tr.right_bottom(),a_bot),(tr.left_bottom(),a_bot)]{
                m.vertices.push(egui::epaint::Vertex{pos,uv,color:col}); }
            m.indices.extend_from_slice(&[0,1,2,0,2,3]);
            ui.painter().add(egui::Shape::mesh(m));
            ui.horizontal(|ui|{
                ui.spacing_mut().item_spacing.x=2.0;
                if tbtn(ui,&pl,"≡").clicked(){ self.sidebar=!self.sidebar; }
                tbtn(ui,&pl,"‹"); tbtn(ui,&pl,"›"); tbtn(ui,&pl,"↺");
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Center),|ui|{
                    avatar(ui,&pl);
                    ui.add_space(6.0);
                    if tbtn(ui,&pl,if self.dark{"☀"}else{"☾"}).clicked(){ self.dark=!self.dark; }
                    ui.add_space(10.0);
                    Frame::none().fill(pl.addbg).rounding(Rounding::same(16.0)).stroke(bord).inner_margin(Margin::symmetric(14.0,7.0)).show(ui,|ui|{
                        ui.horizontal(|ui|{
                            ui.label(RichText::new("⌕").size(13.0).color(pl.muted));
                            ui.add(egui::TextEdit::singleline(&mut self.address).frame(false).text_color(pl.txt)
                                .desired_width(ui.available_width()-26.0)
                                .hint_text(RichText::new("search or type a url · 검색 또는 주소").color(pl.faint)));
                            ui.label(RichText::new("☆").size(13.0).color(pl.muted));
                        });
                    });
                });
            });
        });

        if self.sidebar {
            egui::SidePanel::left("side").resizable(false).exact_width(250.0)
                .frame(Frame::none().fill(pl.side).inner_margin(Margin::symmetric(8.0,10.0)).stroke(bord)).show(ctx,|ui|{
                let (hr,_)=ui.allocate_exact_size(vec2(ui.available_width(),30.0),Sense::hover()); let p=ui.painter();
                p.text(Pos2::new(hr.left()+12.0,hr.center().y),egui::Align2::LEFT_CENTER,"⌂ personal",mono(13.5),pl.txt);
                p.text(Pos2::new(hr.right()-10.0,hr.center().y),egui::Align2::RIGHT_CENTER,"▢",mono(13.0),pl.muted);
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink([false,false]).show(ui,|ui|{
                    sec(ui,&pl,"spaces");
                    row(ui,&pl,pl.accent,"●","personal",None,true);
                    row(ui,&pl,pl.faint,"○","work",None,false);
                    row(ui,&pl,pl.faint,"○","study",None,false);
                    newbtn(ui,&pl,"new space"); ui.add_space(8.0);
                    sec(ui,&pl,"tabs");
                    for i in 0..self.tabs.len(){ if self.tabs[i].owner==0 { let s=i==self.sel;
                        if row(ui,&pl,if s{pl.accent}else{pl.faint},"○",&self.tabs[i].title.clone(),None,s){self.sel=i;} } }
                    let an=self.tabs.iter().filter(|t|t.owner!=0).count();
                    if an>0 { subhead(ui,&pl,&format!("agent tabs ({an})"),pl.accent);
                        for i in 0..self.tabs.len(){ if self.tabs[i].owner!=0 { let s=i==self.sel;
                            let icol=if self.tabs[i].owner==1{pl.accent}else{pl.deleg};
                            if row(ui,&pl,icol,"●",&self.tabs[i].title.clone(),None,s){self.sel=i;} } } }
                    newbtn(ui,&pl,"new tab"); ui.add_space(8.0);
                    sec(ui,&pl,"bookmarks");
                    for n in ["notion","figma","github"]{ row(ui,&pl,pl.faint,"◆",n,None,false); }
                    ui.add_space(8.0);
                    subhead(ui,&pl,"library",pl.faint);
                    row(ui,&pl,pl.muted,"↺","history",Some(128),false);
                    row(ui,&pl,pl.accent,"↓","downloads",Some(3),false);
                    row(ui,&pl,pl.deleg,"▤","reading list",Some(5),false);
                    ui.add_space(12.0); divider(ui,&pl); ui.add_space(6.0);
                    row(ui,&pl,pl.muted,"◉","account",None,false);
                    row(ui,&pl,pl.muted,"◴","settings",None,false); ui.add_space(4.0);
                });
            });
        }

        if self.agent_open {
            egui::TopBottomPanel::bottom("agent").frame(Frame::none().fill(pl.bg).inner_margin(Margin::symmetric(16.0,10.0)).stroke(bord)).show(ctx,|ui|{
                ui.horizontal(|ui|{
                    Frame::none().fill(pl.addbg).rounding(Rounding::same(8.0)).stroke(bord).inner_margin(Margin::symmetric(12.0,8.0)).show(ui,|ui|{
                        ui.set_width(ui.available_width()-48.0);
                        ui.horizontal(|ui|{
                            ui.label(RichText::new("✦").color(pl.accent));
                            let mut d=String::new();
                            ui.add(egui::TextEdit::singleline(&mut d).frame(false).desired_width(ui.available_width())
                                .hint_text(RichText::new("ask the agent to do something · 에이전트에게 작업 요청").color(pl.faint)));
                        });
                    });
                    let _=ui.add(egui::Button::new(RichText::new("→").size(16.0).color(Color32::WHITE)).fill(pl.accent).min_size(vec2(36.0,36.0)).rounding(Rounding::same(8.0)));
                });
            });
        }

        egui::CentralPanel::default().frame(Frame::none().fill(pl.bg)).show(ctx,|ui|{
            Frame::none().inner_margin(Margin::symmetric(16.0,12.0)).show(ui,|ui|{
                ui.horizontal(|ui|{
                    ui.label(RichText::new(self.tabs[self.sel].title.clone()).size(14.0).strong().color(pl.txt));
                    ui.with_layout(Layout::right_to_left(Align::Center),|ui|{
                        let lbl=if self.agent_open{"✦ agent ▾"}else{"✦ agent ▸"};
                        if ui.add(egui::Button::new(RichText::new(lbl).size(12.0).color(if self.agent_open{pl.accent}else{pl.muted}))).clicked(){ self.agent_open=!self.agent_open; }
                    });
                });
            });
            Frame::none().inner_margin(Margin::symmetric(16.0,0.0)).show(ui,|ui|{
                Frame::none().fill(pl.web).rounding(Rounding::same(10.0)).stroke(bord).show(ui,|ui|{
                    let h=ui.available_height()-16.0;
                    ui.set_min_height(h.max(120.0)); ui.set_min_width(ui.available_width());
                    ui.vertical_centered(|ui|{ ui.add_space((h*0.44).max(40.0));
                        ui.label(RichText::new(self.tabs[self.sel].url.clone()).color(pl.muted).size(14.0));
                        ui.add_space(2.0);
                        ui.label(RichText::new("cef · osr · wgpu compositing").color(pl.faint).size(11.0)); });
                });
            });
            ui.add_space(12.0);
        });

        // ===== 떠있는 에이전트 상태 pill (ego lite 시그니처) =====
        if self.tabs.iter().any(|t| t.owner!=0) {
            let tm = ctx.input(|i| i.time);
            let pulse = (0.5 + 0.5*((tm*3.2).sin())) as f32;
            let pill_bg = if self.dark { c(0x24,0x25,0x3A) } else { c(0x2B,0x2B,0x4E) };
            egui::Area::new(egui::Id::new("agent_pill"))
                .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, if self.agent_open {-80.0} else {-26.0}))
                .show(ctx, |ui| {
                    Frame::none().fill(pill_bg).rounding(Rounding::same(22.0))
                        .inner_margin(Margin::symmetric(16.0,9.0))
                        .shadow(egui::epaint::Shadow{offset:vec2(0.0,5.0),blur:20.0,spread:0.0,color:Color32::from_black_alpha(70)})
                        .show(ui,|ui|{
                            ui.horizontal(|ui|{
                                let (rr,_)=ui.allocate_exact_size(vec2(12.0,12.0),Sense::hover());
                                ui.painter().circle_filled(rr.center(), 3.5+pulse*2.0, pl.accent);
                                ui.label(RichText::new("research · sf parks").size(12.5).color(c(0xEE,0xEE,0xF6)).strong());
                                ui.label(RichText::new("· agent in control").size(12.0).color(pl.accent));
                                ui.add_space(10.0);
                                let _=ui.add(egui::Button::new(RichText::new("take over").size(11.5).color(c(0xCC,0xCC,0xDA)))
                                    .fill(c(0x3D,0x3E,0x66)).rounding(Rounding::same(13.0)).min_size(vec2(0.0,24.0)));
                                let _=ui.add(egui::Button::new(RichText::new("stop").size(11.5).color(Color32::WHITE))
                                    .fill(c(0xCF,0x57,0x57)).rounding(Rounding::same(13.0)).min_size(vec2(0.0,24.0)));
                            });
                        });
                });
            ctx.request_repaint();
        }
    }
}

fn main()->eframe::Result<()>{
    let o=eframe::NativeOptions{ viewport: egui::ViewportBuilder::default().with_inner_size([1360.0,860.0]).with_min_inner_size([920.0,600.0]).with_title("Naru"), ..Default::default() };
    eframe::run_native("Naru",o,Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
