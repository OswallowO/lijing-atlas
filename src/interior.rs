//! 建築內部剖面視窗：獨立鏡頭、樓層展開、單層檢視、空間標註。
use crate::data::{Data, MeshVertex};
use crate::render::{Globals, Renderer, Target};
use crate::style::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Vec2};
use egui_wgpu::wgpu;
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

pub fn kind_for(id: &str) -> Option<&'static str> {
    Some(match id {
        "beizhan" | "zhongyang" | "airport" => "hub",
        "univ1" | "univ2" | "univ3" | "univ4" | "univ5" | "oldhall" => "school",
        "library2" | "moma" => "library",
        "jinchao" | "xunguo" => "cafe",
        "apt" | "dorm2" | "beilou" => "apartment",
        "cityhall" | "embassy" | "rnd" | "exchange" => "office",
        "yishitang" => "assembly",
        "cecilia" | "ifc" => "tower",
        _ => return None,
    })
}

pub struct InteriorView {
    kind: String,
    title: String,
    color: Option<String>,
    yaw: f32,
    pitch: f32,
    dist: f32,
    explode: f32,
    only: i32,
    built_explode: f32,
    gpu: Option<(wgpu::Buffer, wgpu::BindGroup, wgpu::Buffer, u32, Target, egui::TextureId)>,
}

impl InteriorView {
    pub fn new(kind: &str, title: &str, color: Option<String>) -> InteriorView {
        InteriorView { kind: kind.into(), title: title.into(), color, yaw: -0.7, pitch: 0.55, dist: 0.0, explode: 0.0, only: -1, built_explode: -1.0, gpu: None }
    }

    pub fn window(&mut self, ctx: &egui::Context, rs: &egui_wgpu::RenderState, r: &mut Renderer, d: &Data, open: &mut bool) {
        let Some(meta) = d.scene_meta.interiors.iter().find(|m| m.kind == self.kind).cloned() else { *open = false; return };
        if self.dist == 0.0 { self.dist = meta.size * 1.1; }
        let screen = ctx.content_rect();
        egui::Window::new(egui::RichText::new(&self.title).size(16.0).strong())
            .open(open)
            .default_size(Vec2::new(screen.width() * 0.8, screen.height() * 0.8))
            .default_pos(screen.min + Vec2::new(screen.width() * 0.1, screen.height() * 0.08))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if seg(ui, "全部", self.only < 0) { self.only = -1; self.dist = meta.size * 1.1; }
                    for (i, l) in meta.levels.iter().enumerate().rev() {
                        if seg(ui, &l.name, self.only == i as i32) { self.only = i as i32; self.dist = meta.size * 0.72; }
                    }
                    ui.separator();
                    ui.label("展開樓層");
                    ui.add(egui::Slider::new(&mut self.explode, 0.0..=4.0).show_value(false));
                });
                let avail = ui.available_size() - Vec2::new(0.0, 46.0);
                let (rect, resp) = ui.allocate_exact_size(avail.max(Vec2::splat(100.0)), Sense::click_and_drag());
                if resp.dragged() {
                    let dl = resp.drag_delta();
                    self.yaw -= dl.x * 0.006;
                    self.pitch = (self.pitch + dl.y * 0.005).clamp(0.05, 1.5);
                }
                if resp.hovered() {
                    let sc = ctx.input(|i| i.smooth_scroll_delta.y);
                    if sc != 0.0 { self.dist = (self.dist * (-sc * 0.0025).exp()).clamp(10.0, 900.0); }
                }
                // 樓層位置（展開時往上拉開）
                let mut order: Vec<usize> = (0..meta.levels.len()).collect();
                order.sort_by(|a, b| meta.levels[*a].y.partial_cmp(&meta.levels[*b].y).unwrap());
                let mut offs = vec![0f32; meta.levels.len()];
                let mut extra = 0.0;
                for &i in &order { offs[i] = extra; extra += self.explode * (meta.levels[i].h + 6.0); }
                let ppp = ctx.pixels_per_point();
                let px = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
                // GPU 資源
                if self.gpu.is_none() || self.built_explode != self.explode || self.gpu.as_ref().map(|g| g.4.size != px).unwrap_or(false) {
                    let mut verts: Vec<MeshVertex> = vec![];
                    for (i, l) in meta.levels.iter().enumerate() {
                        for v in &d.interior_verts[l.off..l.off + l.count] {
                            let mut v = *v;
                            v.pos[1] += offs[i];
                            v.group = (i + 1) as u16;
                            v.year = 0;
                            if let (Some(c), true) = (&self.color, v.col[3] == 254) { let h = crate::mesher::hex(c); v.col = [h[0], h[1], h[2], 0]; }
                            verts.push(v);
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
                    self.built_explode = self.explode;
                }
                // 鏡頭
                let ys: Vec<f32> = meta.levels.iter().enumerate().filter(|(i, _)| self.only < 0 || self.only == *i as i32).flat_map(|(i, l)| [l.y + offs[i], l.y + offs[i] + l.h]).collect();
                let cy = if ys.is_empty() { 0.0 } else { (ys.iter().cloned().fold(f32::MAX, f32::min) + ys.iter().cloned().fold(f32::MIN, f32::max)) / 2.0 };
                let eye = Vec3::new(self.yaw.sin() * self.pitch.cos() * self.dist, cy + self.pitch.sin() * self.dist, self.yaw.cos() * self.pitch.cos() * self.dist);
                let vp = Mat4::perspective_rh(40f32.to_radians(), rect.width() / rect.height().max(1.0), 0.5, 4000.0) * Mat4::look_at_rh(eye, Vec3::new(0.0, cy, 0.0), Vec3::Y);
                let (gb, bg, vb, n, target, tid) = self.gpu.as_ref().unwrap();
                let mut g: Globals = r.globals;
                g.view_proj = vp.to_cols_array_2d();
                g.cam_pos = [eye.x, eye.y, eye.z, 0.0];
                let sd = Vec3::new(0.45, 0.8, 0.4).normalize();
                g.sun_dir = [sd.x, sd.y, sd.z, 0.0];
                g.sun_col = lin4("#fffaf0", 0.00001);
                g.sky_col = lin4("#c3d8ea", 9999.0);
                g.ground_col = lin4("#a8a594", 0.0);
                g.fog_col = lin4("#eef2f4", 0.0);
                let mut vis = [[0.0f32; 4]; 16];
                vis[0][0] = 1.0;
                for i in 0..meta.levels.len().min(63) { let k = i + 1; vis[k / 4][k % 4] = if self.only < 0 || self.only == i as i32 { 1.0 } else { 0.0 }; }
                g.groups = vis;
                rs.queue.write_buffer(gb, 0, bytemuck::bytes_of(&g));
                let mut enc = rs.device.create_command_encoder(&Default::default());
                let bgc = lin4("#eef2f4", 0.0);
                r.draw_mesh_only(&mut enc, target, bg, vb, *n, [bgc[0] as f64, bgc[1] as f64, bgc[2] as f64]);
                rs.queue.submit([enc.finish()]);
                let p = ui.painter_at(rect);
                p.image(*tid, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                // 標註（避讓）
                let mut placed: Vec<Rect> = vec![];
                for (i, l) in meta.levels.iter().enumerate() {
                    if self.only >= 0 && self.only != i as i32 { continue; }
                    for t in &l.tags {
                        let c = vp * Vec4::new(t.p[0], t.p[1] + offs[i], t.p[2], 1.0);
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
                ui.label(egui::RichText::new(&meta.note).size(11.5).color(MUTED));
                ui.label(egui::RichText::new("拖曳旋轉、滾輪縮放；紅色為剖切面。").size(11.0).color(MUTED));
            });
    }
}
