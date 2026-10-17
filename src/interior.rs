//! 建築內部剖面視窗：整棟樓層目錄、單層／剖切／平面圖、樓層展開、簡化量體、空間標註。
//! 每棟地標有完整的樓層計畫書（地下到屋頂逐層），標準層共用同一段幾何；
//! 只有選取（或少樓層建築的全部）樓層畫細部，其餘樓層以樓板＋外殼＋核心筒的簡化量體表示。
use crate::data::{Data, InteriorMeta, MeshVertex};
use crate::render::{Globals, Renderer, Target};
use crate::style::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use egui_wgpu::wgpu;
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

/// 有完整樓層計畫書的地標
const PROGRAMS: &[&str] = &[
    "cecilia", "ifc", "exchange", "beizhan", "zhongyang", "cityhall", "yishitang", "univ1", "univ2", "univ3", "univ4", "univ5", "oldhall", "library2", "moma", "apt", "dorm2",
    "jinchao", "xunguo", "beilou", "airport", "rnd", "csd", "embassy", "market", "jinjing", "culture", "cargo", "tangyu", "caoyu", "gang",
];

pub fn kind_for(id: &str) -> Option<String> {
    if PROGRAMS.contains(&id) {
        return Some(format!("lm_{id}"));
    }
    None
}

/// 少於這個樓層數時，「全棟」直接畫出每層細部
const DETAIL_ALL: usize = 16;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    All,
    One,
    Cut,
    Plan,
}

pub struct InteriorView {
    kind: String,
    title: String,
    color: Option<String>,
    yaw: f32,
    pitch: f32,
    dist: f32,
    explode: f32,
    sel: i32,
    mode: Mode,
    tags: bool,
    scroll_to_sel: bool,
    built: Option<(f32, i32, Mode, (u32, u32))>,
    gpu: Option<(wgpu::Buffer, wgpu::BindGroup, wgpu::Buffer, u32, Target, egui::TextureId)>,
}

fn push_box(v: &mut Vec<MeshVertex>, c: Vec3, half: Vec3, col: [u8; 4]) {
    let (x, y, z) = (Vec3::X * half.x, Vec3::Y * half.y, Vec3::Z * half.z);
    for (n, u, w) in [(Vec3::X, y, z), (-Vec3::X, z, y), (Vec3::Y, z, x), (-Vec3::Y, x, z), (Vec3::Z, x, y), (-Vec3::Z, y, x)] {
        let nn = n * half;
        let q = [c + nn - u - w, c + nn + u - w, c + nn + u + w, c + nn - u + w];
        for k in [0, 1, 2, 0, 2, 3] {
            v.push(MeshVertex { pos: q[k].to_array(), nrm: n.to_array(), col, group: 0, year: 0 });
        }
    }
}

fn split_name(name: &str) -> (String, String) {
    let mut it = name.splitn(2, ' ');
    let a = it.next().unwrap_or("").to_string();
    let b = it.next().unwrap_or("").to_string();
    let is_code = a.starts_with('B') && a[1..].chars().all(|c| c.is_ascii_digit()) && a.len() > 1 || a.ends_with('F') && a[..a.len() - 1].chars().all(|c| c.is_ascii_digit()) && a.len() > 1;
    if is_code { (a, b) } else { (String::new(), name.to_string()) }
}

impl InteriorView {
    pub fn new(kind: &str, title: &str, color: Option<String>) -> InteriorView {
        let sel = std::env::var("LIJING_FLOOR").ok().and_then(|s| s.parse().ok()).unwrap_or(-1);
        let mode = match std::env::var("LIJING_FMODE").as_deref() {
            Ok("one") => Mode::One,
            Ok("cut") => Mode::Cut,
            Ok("plan") => Mode::Plan,
            _ => Mode::All,
        };
        InteriorView { kind: kind.into(), title: title.into(), color, yaw: -0.7, pitch: if mode == Mode::Plan { 1.5 } else { 0.55 }, dist: 0.0, explode: 0.0, sel, mode, tags: true, scroll_to_sel: true, built: None, gpu: None }
    }

    fn order(meta: &InteriorMeta) -> Vec<usize> {
        let mut o: Vec<usize> = (0..meta.levels.len()).collect();
        o.sort_by(|a, b| {
            let (la, lb) = (&meta.levels[*a], &meta.levels[*b]);
            la.y.partial_cmp(&lb.y).unwrap().then(la.zoff.partial_cmp(&lb.zoff).unwrap())
        });
        o
    }

    fn focus(&mut self, meta: &InteriorMeta, tall: f32) {
        let fs = if self.sel >= 0 { let l = &meta.levels[self.sel as usize]; l.w.max(l.d).max(20.0) } else { meta.size };
        self.dist = match self.mode {
            Mode::One => fs * 1.25,
            Mode::Plan => fs * 1.35,
            Mode::Cut => (fs * 1.6).max(tall * 0.5),
            Mode::All => if tall > meta.size { tall * 1.75 } else { meta.size * 1.1 },
        };
    }

    fn set_mode(&mut self, m: Mode, meta: &InteriorMeta, order: &[usize], tall: f32) {
        self.mode = m;
        if self.sel < 0 && m != Mode::All {
            self.sel = order.iter().copied().find(|&i| meta.levels[i].y >= 0.0).unwrap_or(order[0]) as i32;
        }
        if m == Mode::Plan { self.pitch = 1.5 } else if self.pitch > 1.45 { self.pitch = 0.55 }
        self.focus(meta, tall);
        self.scroll_to_sel = true;
    }

    fn select(&mut self, i: usize, meta: &InteriorMeta, tall: f32) {
        self.sel = i as i32;
        if self.mode == Mode::All && meta.levels.len() <= DETAIL_ALL { self.mode = Mode::One; }
        self.focus(meta, tall);
        self.scroll_to_sel = true;
    }

    fn step(&mut self, d: i32, meta: &InteriorMeta, order: &[usize], tall: f32) {
        let mut k = order.iter().position(|&i| i as i32 == self.sel).map(|k| k as i32).unwrap_or_else(|| order.iter().position(|&i| meta.levels[i].y >= 0.0).unwrap_or(0) as i32 - d.signum().min(0));
        k = (k + d).clamp(0, order.len() as i32 - 1);
        self.select(order[k as usize], meta, tall);
    }

    /// 每層的顯示狀態：(顯示, 細部)
    fn vis(&self, meta: &InteriorMeta) -> Vec<(bool, bool)> {
        let many = meta.levels.len() > DETAIL_ALL;
        let sy = if self.sel >= 0 { meta.levels[self.sel as usize].y } else { 0.0 };
        meta.levels.iter().enumerate().map(|(i, l)| {
            let me = i as i32 == self.sel;
            match self.mode {
                Mode::One | Mode::Plan => (me, true),
                Mode::Cut => (l.y <= sy + 0.01, me || (!many && l.y >= sy - 0.01)),
                Mode::All => (true, !many || me),
            }
        }).collect()
    }

    pub fn window(&mut self, ctx: &egui::Context, rs: &egui_wgpu::RenderState, r: &mut Renderer, d: &Data, open: &mut bool) {
        let Some(meta) = d.scene_meta.interiors.iter().find(|m| m.kind == self.kind).cloned() else { *open = false; return };
        let order = Self::order(&meta);
        let ymin = meta.levels.iter().map(|l| l.y).fold(f32::MAX, f32::min);
        let ymax = meta.levels.iter().map(|l| l.y + l.h).fold(f32::MIN, f32::max);
        let tall = ymax - ymin;
        if self.dist == 0.0 { if self.sel >= meta.levels.len() as i32 { self.sel = -1; } self.focus(&meta, tall); }
        // 鍵盤切換樓層
        let (up, dn) = ctx.input(|i| (i.key_pressed(egui::Key::PageUp) || i.key_pressed(egui::Key::CloseBracket), i.key_pressed(egui::Key::PageDown) || i.key_pressed(egui::Key::OpenBracket)));
        if up { self.step(1, &meta, &order, tall); }
        if dn { self.step(-1, &meta, &order, tall); }
        let screen = ctx.content_rect();
        egui::Window::new(egui::RichText::new(&self.title).size(16.0).strong())
            .open(open)
            .default_size(Vec2::new(screen.width() * 0.84, screen.height() * 0.84))
            .default_pos(screen.min + Vec2::new(screen.width() * 0.08, screen.height() * 0.06))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (m, n) in [(Mode::All, "全棟"), (Mode::One, "單層"), (Mode::Cut, "剖切到此層"), (Mode::Plan, "平面圖")] {
                        if seg(ui, n, self.mode == m) { self.set_mode(m, &meta, &order, tall); }
                    }
                    ui.separator();
                    if ui.button("▼ 下一層").clicked() { self.step(-1, &meta, &order, tall); }
                    if ui.button("▲ 上一層").clicked() { self.step(1, &meta, &order, tall); }
                    ui.separator();
                    if seg(ui, "標註", self.tags) { self.tags = !self.tags; }
                    ui.separator();
                    ui.label("展開樓層");
                    ui.add(egui::Slider::new(&mut self.explode, 0.0..=4.0).show_value(false));
                });
                let avail = ui.available_size() - Vec2::new(0.0, 46.0);
                let h = avail.y.max(120.0);
                ui.horizontal(|ui| {
                    // ---- 樓層目錄 ----
                    let list_w = 236.0f32.min(avail.x * 0.3);
                    ui.allocate_ui_with_layout(Vec2::new(list_w, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_min_size(Vec2::new(list_w, h));
                        egui::ScrollArea::vertical().id_salt("floors").max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_width(list_w - 8.0);
                            ui.spacing_mut().item_spacing.y = 1.0;
                            let mut zone = "";
                            let mut clicked = None;
                            for &i in order.iter().rev() {
                                let l = &meta.levels[i];
                                if !l.zone.is_empty() && l.zone != zone {
                                    zone = &l.zone;
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new(&l.zone).size(10.5).color(MUTED));
                                }
                                let (rr, resp) = ui.allocate_exact_size(Vec2::new(list_w - 14.0, 21.0), Sense::click());
                                let on = i as i32 == self.sel;
                                let p = ui.painter();
                                if on { p.rect_filled(rr, 5.0, INK); } else if resp.hovered() { p.rect_filled(rr, 5.0, Color32::from_rgb(0xe8, 0xee, 0xf2)); }
                                let fg = if on { Color32::WHITE } else { INK };
                                let (code, rest) = split_name(&l.name);
                                let mut x = rr.min.x + 6.0;
                                if !code.is_empty() {
                                    p.text(Pos2::new(x, rr.center().y), Align2::LEFT_CENTER, &code, FontId::proportional(12.5), fg);
                                    x += 40.0;
                                }
                                let maxw = rr.max.x - 40.0 - x;
                                let g = p.layout(rest.clone(), FontId::proportional(12.0), fg, f32::INFINITY);
                                let txt = if g.size().x > maxw { let n = ((maxw / 12.0) as usize).max(2); format!("{}…", rest.chars().take(n).collect::<String>()) } else { rest };
                                p.text(Pos2::new(x, rr.center().y), Align2::LEFT_CENTER, txt, FontId::proportional(12.0), fg);
                                p.text(Pos2::new(rr.max.x - 4.0, rr.center().y), Align2::RIGHT_CENTER, format!("{:+.0}", l.y), FontId::proportional(10.5), if on { Color32::from_gray(200) } else { MUTED });
                                if on && self.scroll_to_sel { resp.scroll_to_me(Some(egui::Align::Center)); }
                                if resp.clicked() { clicked = Some(i); }
                            }
                            self.scroll_to_sel = false;
                            if let Some(i) = clicked { self.select(i, &meta, tall); self.scroll_to_sel = false; }
                        });
                    });
                    // ---- 視圖 ----
                    let vw = ui.available_width().max(100.0);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(vw, h), Sense::click_and_drag());
                    if resp.dragged() {
                        let dl = resp.drag_delta();
                        self.yaw -= dl.x * 0.006;
                        if self.mode != Mode::Plan { self.pitch = (self.pitch + dl.y * 0.005).clamp(0.05, 1.5); }
                    }
                    if resp.hovered() {
                        let sc = ctx.input(|i| i.smooth_scroll_delta.y);
                        if sc != 0.0 { self.dist = (self.dist * (-sc * 0.0025).exp()).clamp(10.0, 3000.0); }
                    }
                    // 樓層位置（展開時往上拉開）
                    let mut offs = vec![0f32; meta.levels.len()];
                    let mut extra = 0.0;
                    for &i in &order { offs[i] = extra; extra += self.explode * (meta.levels[i].h + 6.0); }
                    let vis = self.vis(&meta);
                    let ppp = ctx.pixels_per_point();
                    let px = (((rect.width() * ppp) as u32).max(1), ((rect.height() * ppp) as u32).max(1));
                    let key = (self.explode, self.sel, self.mode, px);
                    if self.gpu.is_none() || self.built != Some(key) {
                        let mut verts: Vec<MeshVertex> = vec![];
                        let tint = self.color.as_ref().map(|c| crate::mesher::hex(c));
                        for (i, l) in meta.levels.iter().enumerate() {
                            let (show, det) = vis[i];
                            if !show { continue; }
                            let y0 = l.y + offs[i];
                            if det {
                                let [off, n] = meta.geoms[l.g];
                                for v in &d.interior_verts[off..off + n] {
                                    let mut v = *v;
                                    v.pos[1] += y0;
                                    v.group = 0;
                                    v.year = 0;
                                    if let (Some(h), true) = (tint, v.col[3] == 254) { v.col = [h[0], h[1], h[2], 0]; }
                                    verts.push(v);
                                }
                            } else {
                                let (w, dd, z) = (l.w.max(4.0), l.d.max(4.0), l.zoff);
                                push_box(&mut verts, Vec3::new(0.0, y0 - 0.18, z), Vec3::new(w / 2.0, 0.18, dd / 2.0), [200, 205, 210, 0]);
                                push_box(&mut verts, Vec3::new(0.0, y0 - 0.18, z + dd / 2.0 + 0.1), Vec3::new(w / 2.0, 0.18, 0.1), [185, 99, 74, 0]);
                                push_box(&mut verts, Vec3::new(0.0, y0 + (l.h - 0.4) / 2.0, z), Vec3::new(w / 2.0 - 0.3, (l.h - 0.4) / 2.0, dd / 2.0 - 0.3), [214, 226, 236, 0]);
                                if let Some(c) = l.core { push_box(&mut verts, Vec3::new(0.0, y0 + l.h / 2.0, z), Vec3::new(c[0] / 2.0 + 0.05, l.h / 2.0, c[1] / 2.0 + 0.05), [176, 182, 190, 0]); }
                            }
                        }
                        if verts.is_empty() { verts.push(bytemuck::Zeroable::zeroed()); }
                        let vb = rs.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("interior"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
                        let (gb, bg, tid, target) = match self.gpu.take() {
                            Some((gb, bg, _, _, t, tid)) if t.size == px => (gb, bg, tid, t),
                            Some((gb, bg, _, _, _, tid)) => { let t = Target::new(&rs.device, px.0, px.1); rs.renderer.write().update_egui_texture_from_wgpu_texture(&rs.device, &t.color_view, wgpu::FilterMode::Linear, tid); (gb, bg, tid, t) }
                            None => { let (gb, bg) = r.aux_bind(&rs.device); let t = Target::new(&rs.device, px.0, px.1); let tid = rs.renderer.write().register_native_texture(&rs.device, &t.color_view, wgpu::FilterMode::Linear); (gb, bg, tid, t) }
                        };
                        self.gpu = Some((gb, bg, vb, verts.len() as u32, target, tid));
                        self.built = Some(key);
                    }
                    // 鏡頭中心
                    let cy = if self.mode == Mode::All && self.sel < 0 {
                        (ymin + ymax + extra) / 2.0
                    } else {
                        let ys: Vec<f32> = meta.levels.iter().enumerate().filter(|(i, _)| { let (s, dt) = vis[*i]; s && (dt || *i as i32 == self.sel || self.mode != Mode::All) }).flat_map(|(i, l)| [l.y + offs[i], l.y + offs[i] + l.h]).collect();
                        let ys = if self.mode == Mode::Cut && self.sel >= 0 { let i = self.sel as usize; let l = &meta.levels[i]; vec![l.y + offs[i] - tall * 0.15, l.y + offs[i] + l.h] } else { ys };
                        if ys.is_empty() { 0.0 } else { (ys.iter().cloned().fold(f32::MAX, f32::min) + ys.iter().cloned().fold(f32::MIN, f32::max)) / 2.0 }
                    };
                    let cz = if self.sel >= 0 && self.mode != Mode::All { meta.levels[self.sel as usize].zoff } else { 0.0 };
                    let eye = Vec3::new(self.yaw.sin() * self.pitch.cos() * self.dist, cy + self.pitch.sin() * self.dist, cz + self.yaw.cos() * self.pitch.cos() * self.dist);
                    let vp = Mat4::perspective_rh(40f32.to_radians(), rect.width() / rect.height().max(1.0), 0.5, 8000.0) * Mat4::look_at_rh(eye, Vec3::new(0.0, cy, cz), Vec3::Y);
                    let (gb, bg, vb, n, target, tid) = self.gpu.as_ref().unwrap();
                    let mut g: Globals = r.globals;
                    g.view_proj = vp.to_cols_array_2d();
                    g.cam_pos = [eye.x, eye.y, eye.z, 0.0];
                    let sd = Vec3::new(0.45, 0.8, 0.4).normalize();
                    g.sun_dir = [sd.x, sd.y, sd.z, 0.0];
                    g.sun_col = lin4("#fffaf0", 0.00001);
                    g.sky_col = lin4("#c3d8ea", 99999.0);
                    g.ground_col = lin4("#a8a594", 0.0);
                    g.fog_col = lin4("#eef2f4", 0.0);
                    let mut gv = [[0.0f32; 4]; 16];
                    gv[0][0] = 1.0;
                    g.groups = gv;
                    rs.queue.write_buffer(gb, 0, bytemuck::bytes_of(&g));
                    let mut enc = rs.device.create_command_encoder(&Default::default());
                    let bgc = lin4("#eef2f4", 0.0);
                    r.draw_mesh_only(&mut enc, target, bg, vb, *n, [bgc[0] as f64, bgc[1] as f64, bgc[2] as f64]);
                    rs.queue.submit([enc.finish()]);
                    let p = ui.painter_at(rect);
                    p.image(*tid, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    // 標註（只標細部樓層，避讓）
                    let mut placed: Vec<Rect> = vec![];
                    if self.tags {
                        for (i, l) in meta.levels.iter().enumerate() {
                            if !(vis[i].0 && vis[i].1) { continue; }
                            for t in &l.tags {
                                let c = vp * Vec4::new(t.p[0], t.p[1] + l.y + offs[i], t.p[2], 1.0);
                                if c.w <= 0.0 { continue; }
                                let q = Pos2::new(rect.min.x + (c.x / c.w + 1.0) / 2.0 * rect.width(), rect.min.y + (1.0 - c.y / c.w) / 2.0 * rect.height());
                                let gal = p.layout_no_wrap(t.text.clone(), FontId::proportional(11.5), Color32::WHITE);
                                let rr = Rect::from_center_size(q - Vec2::new(0.0, gal.size().y / 2.0 + 2.0), gal.size() + Vec2::new(12.0, 4.0));
                                if !rect.contains_rect(rr) || placed.iter().any(|o| o.intersects(rr)) { continue; }
                                placed.push(rr);
                                p.rect_filled(rr, 6.0, Color32::from_rgba_unmultiplied(22, 32, 42, 215));
                                p.text(rr.center(), Align2::CENTER_CENTER, &t.text, FontId::proportional(11.5), Color32::WHITE);
                            }
                        }
                    }
                    // 樓層資訊卡
                    let info: Vec<(String, bool)> = if self.sel >= 0 {
                        let l = &meta.levels[self.sel as usize];
                        let mut v = vec![(l.name.clone(), true), (format!("樓板標高 {:+.1} m・層高 {} m", l.y, l.h), false)];
                        if l.w > 0.0 { v.push((format!("樓板 {:.0} × {:.0} m（約 {:.0} 坪）", l.w, l.d, l.w * l.d / 3.3058), false)); }
                        v
                    } else if meta.floors > 0 {
                        vec![(format!("全棟 {} 層", meta.floors), true), (format!("高度約 {:.0} m・地下 {:.0} m", ymax, -ymin.min(0.0)), false), ("點選左側樓層看細部".into(), false)]
                    } else { vec![] };
                    if !info.is_empty() {
                        let mut y = rect.min.y + 12.0;
                        let w = info.iter().map(|(s, b)| p.layout_no_wrap(s.clone(), FontId::proportional(if *b { 13.5 } else { 12.0 }), INK).size().x).fold(0.0, f32::max) + 20.0;
                        let box_r = Rect::from_min_size(Pos2::new(rect.min.x + 12.0, y), Vec2::new(w, info.len() as f32 * 19.0 + 12.0));
                        p.rect(box_r, 8.0, Color32::from_rgba_unmultiplied(255, 255, 255, 225), Stroke::new(1.0, Color32::from_gray(215)), egui::StrokeKind::Inside);
                        y += 8.0;
                        for (s, b) in &info {
                            p.text(Pos2::new(box_r.min.x + 10.0, y), Align2::LEFT_TOP, s, FontId::proportional(if *b { 13.5 } else { 12.0 }), if *b { INK } else { MUTED });
                            y += 19.0;
                        }
                    }
                });
                ui.label(egui::RichText::new(&meta.note).size(11.5).color(MUTED));
                ui.label(egui::RichText::new("拖曳旋轉、滾輪縮放；左側樓層目錄或 PageUp／PageDown 切換樓層；紅色為剖切面，未選取的樓層以簡化量體顯示。").size(11.0).color(MUTED));
            });
    }
}
