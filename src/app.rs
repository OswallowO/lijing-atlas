//! 主程式：載入、3D 視圖、介面（遊戲式浮動面板）、標籤、時間軸、運輸、CS2、剖面、量測、巡禮、自動更新。
use crate::camera::Camera;
use crate::data::{find_data_dir, Data};
use crate::interior::InteriorView;
use crate::render::{Renderer, Target};
use crate::style::*;
use crate::update::{State as UState, Updater, VERSION};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use egui_wgpu::wgpu;
use glam::Vec3;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode { Plan, ThreeD, Contour, Transit, Cs2, Overview }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tod { Dawn, Day, Dusk, Night }
#[derive(Clone, PartialEq)]
pub enum Sel { None, Landmark(usize), Station(usize), District(usize), Line(String) }

#[derive(Clone, Copy, PartialEq)]
enum LKind { Landmark, Station, District, Island, Village, Peak }
struct Label {
    p: Vec3,
    text: String,
    kind: LKind,
    pri: i32,
    max_d: f32,
    min_d: f32,
    color: Color32,
    glyph: Option<(String, Color32)>,
    idx: usize,
    year: i32,
    lines: Vec<String>,
}

pub struct Landmark {
    pub id: String,
    pub name: String,
    pub en: String,
    pub cat: String,
    pub d: String,
    pub at: (f64, f64),
    pub h: f32,
    pub fp: (f32, f32),
    pub desc: String,
    pub floors: String,
    pub built: i32,
    pub chars: Vec<String>,
    pub design: bool,
    pub rot: f32,
}

pub const END_DAY: f32 = 4382.0; // 2025-12-31
const SPEEDS: [(f32, &str); 5] = [(1.0, "每秒 1 天"), (7.0, "每秒 1 週"), (30.0, "每秒 1 月"), (91.0, "每秒 1 季"), (365.0, "每秒 1 年")];
/// 2014-01-01 起算的日數 → 日期字串
pub fn date_of(day: f32) -> String {
    let mut d = day.max(0.0) as i32;
    let mut y = 2014;
    loop { let n = if y % 4 == 0 { 366 } else { 365 }; if d < n { break; } d -= n; y += 1; }
    let ml = [31, if y % 4 == 0 { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while m < 11 && d >= ml[m] { d -= ml[m]; m += 1; }
    format!("{y}-{:02}-{:02}", m + 1, d + 1)
}
pub fn day_of_year(y: i32) -> f32 { (y - 2014) as f32 * 365.25 }

pub struct World {
    pub d: Data,
    pub r: Renderer,
    pub cam: Camera,
    target: Option<Target>,
    tex: Option<egui::TextureId>,
    pub mode: Mode,
    pub year: i32,
    pub day: f32,
    speed: usize,
    tod: Tod,
    pub lines_on: HashSet<String>,
    lines_hide: HashSet<String>,
    focus_line: Option<String>,
    sel: Sel,
    labels: Vec<Label>,
    pub lms: Vec<Landmark>,
    keys: HashSet<egui::Key>,
    left_tilt: bool,
    playing: bool,
    play_t: f32,
    scale: f32,
    auto_res: bool,
    fps: f32,
    fps_acc: (f32, u32),
    frame_ms: f32,
    search: String,
    tour: Option<usize>,
    measure: Option<Vec<Vec3>>,
    hover: Option<(Pos2, String)>,
    hover_t: f32,
    last_mouse: Pos2,
    side_open: bool,
    side_w: f32,
    tl_open: bool,
    cs2: (f64, f64, f32, f32), // 框心 lon/lat、可建造邊長 m、旋轉度
    cs2_msg: String,
    hm_sea: f32,
    interior: Option<InteriorView>,
    orbit: bool,
    layers: [bool; 4], // 地標、地名、分區、熱區
    outlines: Vec<Outline>,
    bounds: bool,            // 顯示分區邊界
    bound_sel: Option<usize>, // 只顯示某一分區
    zone_line: bool,         // 顯示特區範圍
}

/// 地面外框（分區、特區、地標基地）
struct Outline { pts: Vec<Vec3>, color: Color32, kind: u8, idx: usize, key: String }

enum Load { Loading(mpsc::Receiver<Result<Data, String>>, Arc<Mutex<(f32, String)>>), Ready(Box<World>), Failed(String) }

pub struct App {
    load: Load,
    upd: Updater,
    upd_checked: bool,
    upd_dismissed: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> App {
        let dir = find_data_dir();
        install_fonts(&cc.egui_ctx, &dir);
        apply_style(&cc.egui_ctx);
        let prog = Arc::new(Mutex::new((0.0f32, "讀取資料…".to_string())));
        let (tx, rx) = mpsc::channel();
        let p2 = prog.clone();
        let ctx = cc.egui_ctx.clone();
        let dir2 = dir.clone();
        std::thread::spawn(move || {
            let r = Data::load(dir2, &|f, m| { *p2.lock().unwrap() = (f, m.to_string()); ctx.request_repaint(); });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        App { load: Load::Loading(rx, prog), upd: Updater::new(&dir), upd_checked: false, upd_dismissed: false }
    }
}

fn s(v: &Value) -> String { v.as_str().unwrap_or("").to_string() }

impl World {
    fn new(d: Data, rs: &egui_wgpu::RenderState) -> World {
        let r = Renderer::new(&rs.device, &rs.queue, &d);
        let mut cam = Camera::new();
        let built = d.story["BUILT"].clone();
        let lms: Vec<Landmark> = d.story["LANDMARKS"].as_array().cloned().unwrap_or_default().iter().map(|l| Landmark {
            id: s(&l["id"]), name: s(&l["name"]), en: s(&l["en"]), cat: s(&l["cat"]), d: s(&l["d"]),
            at: (l["at"][0].as_f64().unwrap_or(0.0), l["at"][1].as_f64().unwrap_or(0.0)), h: l["h"].as_f64().unwrap_or(20.0) as f32,
            fp: (l["fp"][0].as_f64().unwrap_or(4.0) as f32, l["fp"][1].as_f64().unwrap_or(4.0) as f32), desc: s(&l["desc"]), floors: s(&l["floors"]),
            built: built[s(&l["id"])].as_i64().unwrap_or(2015) as i32, chars: l["chars"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default(),
            design: l["design"].as_bool().unwrap_or(false), rot: l["rot"].as_f64().unwrap_or(0.0) as f32,
        }).collect();
        let (x, y) = d.geo.to_local(119.762, 25.545);
        cam.target = Vec3::new(x as f32, 0.0, -y as f32);
        cam.dist = 26000.0;
        cam.pitch = 42f32.to_radians();
        cam.yaw = 18f32.to_radians();
        let mut w = World {
            r, cam, target: None, tex: None, mode: Mode::ThreeD, year: 2025, day: END_DAY, speed: 1, tod: Tod::Day, lines_on: HashSet::new(), lines_hide: HashSet::new(), focus_line: None, sel: Sel::None,
            labels: vec![], lms, keys: HashSet::new(), left_tilt: false, playing: false, play_t: 0.0, scale: 1.0, auto_res: true, fps: 0.0, fps_acc: (0.0, 0), frame_ms: 0.0,
            search: String::new(), tour: None, measure: None, hover: None, hover_t: 0.0, last_mouse: Pos2::ZERO, side_open: true, side_w: 354.0, tl_open: true,
            cs2: (119.773, 25.500, 57344.0, 0.0), cs2_msg: String::new(), hm_sea: 40.0, interior: None, orbit: false, layers: [true, true, true, true], outlines: vec![], bounds: false, bound_sel: None, zone_line: false, d,
        };
        w.build_labels();
        w.build_outlines();
        w.set_time(Tod::Day);
        if std::env::var("LIJING_BOUNDS").is_ok() { w.bounds = true; w.zone_line = true; }
        if std::env::var("LIJING_SIDE_CLOSED").is_ok() { w.side_open = false; }
        if std::env::var("LIJING_TRANSIT").is_ok() { w.mode = Mode::Transit; }
        // 測試用：LIJING_VIEW=經度,緯度,距離,俯角,方位,年份；LIJING_INTERIOR=種類
        if let Ok(v) = std::env::var("LIJING_VIEW") {
            let a: Vec<f64> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            if a.len() >= 6 {
                let (x, y) = w.d.geo.to_local(a[0], a[1]);
                w.cam.target = Vec3::new(x as f32, 0.0, -y as f32);
                w.cam.dist = a[2] as f32;
                w.cam.pitch = (a[3] as f32).to_radians();
                w.cam.yaw = -(a[4] as f32).to_radians();
                w.day = if (2014.0..2030.0).contains(&a[5]) { day_of_year(a[5] as i32) + 364.0 } else { a[5] as f32 };
            }
        }
        if let Ok(k) = std::env::var("LIJING_INTERIOR") { w.interior = Some(InteriorView::new(&k, "測試・內部剖面", None)); }
        if std::env::var("LIJING_OVERVIEW").is_ok() { w.set_mode(Mode::Overview); w.cam.anim = None; let (x, y) = w.d.geo.to_local(119.772, 25.535); w.cam.target = Vec3::new(x as f32, 0.0, -(y as f32 - 9000.0)); w.cam.dist = 52000.0; w.cam.pitch = 48f32.to_radians(); w.cam.yaw = 12f32.to_radians(); }
        if std::env::var("LIJING_LINES").is_ok() { w.lines_on = w.d.scene_meta.lines.iter().map(|l| l.id.clone()).collect(); }
        w
    }

    fn build_labels(&mut self) {
        let d = &self.d;
        let major: HashSet<&str> = ["cecilia", "beizhan", "zhongyang", "yishitang", "cityhall", "univ2", "univ4", "airport", "caoyu", "beilou", "univ1", "univ3", "univ5", "rnd", "tangyu", "niushan", "dongxiang", "changyu", "csd"].into_iter().collect();
        let wp = |lon: f64, lat: f64, lift: f32| { let (x, y) = d.geo.to_local(lon, lat); Vec3::new(x as f32, d.hg.at(x, y).max(0.0) + lift, -y as f32) };
        let mut out = vec![];
        for (i, l) in self.lms.iter().enumerate() {
            let (n, c, g) = cat(&l.cat);
            let mj = major.contains(l.id.as_str());
            let _ = n;
            out.push(Label { p: wp(l.at.0, l.at.1, l.h.min(200.0) * 0.6 + 10.0), text: l.name.clone(), kind: LKind::Landmark, pri: if mj { 9 } else { 6 }, max_d: if mj { 45000.0 } else { 9000.0 }, min_d: 0.0, color: INK, glyph: Some((g.to_string(), c)), idx: i, year: l.built, lines: vec![] });
        }
        for (i, st) in d.scene_meta.stations.iter().enumerate() {
            let col = if st.lines.len() > 1 { INK } else { d.scene_meta.lines.iter().find(|l| l.id == st.lines[0]).map(|l| hex32(&l.color)).unwrap_or(INK) };
            out.push(Label { p: wp(st.ll[0], st.ll[1], if st.kind == "e" { 14.0 } else { 4.0 }), text: st.n.clone(), kind: LKind::Station, pri: if st.lines.len() > 1 { 5 } else { 4 }, max_d: 14000.0, min_d: 0.0, color: col, glyph: None, idx: i, year: st.yr, lines: st.lines.clone() });
        }
        for (i, dd) in d.story["DISTRICTS"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let poly = dd["poly"].as_array().cloned().unwrap_or_default();
            let n = poly.len().max(1) as f64;
            let lon = poly.iter().map(|p| p[0].as_f64().unwrap_or(0.0)).sum::<f64>() / n;
            let lat = poly.iter().map(|p| p[1].as_f64().unwrap_or(0.0)).sum::<f64>() / n;
            out.push(Label { p: wp(lon, lat, 0.0), text: s(&dd["name"]), kind: LKind::District, pri: 7, max_d: 80000.0, min_d: 9000.0, color: hex32(&s(&dd["color"])), glyph: None, idx: i, year: 2016, lines: vec![] });
        }
        for (arr, kind, pri, maxd) in [(&d.islands, LKind::Island, 3, 50000.0), (&d.villages, LKind::Village, 1, 6000.0)] {
            for f in arr["features"].as_array().cloned().unwrap_or_default() {
                let c = &f["geometry"]["coordinates"];
                let mut n = s(&f["properties"]["n"]);
                n = match n.as_str() { "Dongxiang Dao" => "東庠島".into(), "Yutou Dao" => "嶼頭島".into(), "Kemen Dao" => "嶼門島".into(), _ => n };
                out.push(Label { p: wp(c[0].as_f64().unwrap_or(0.0), c[1].as_f64().unwrap_or(0.0), 0.0), text: n, kind, pri, max_d: maxd, min_d: 0.0, color: if kind == LKind::Island { Color32::from_rgb(0x2c, 0x5a, 0x76) } else { Color32::from_rgb(0x55, 0x60, 0x6b) }, glyph: None, idx: 0, year: 0, lines: vec![] });
            }
        }
        let names = [("將軍山", 119.8052, 25.4707), ("將軍山西峰", 119.7930, 25.4675), ("君山", 119.8138, 25.6002), ("三十六腳湖北嶺", 119.7090, 25.6440), ("娘宮山", 119.7120, 25.4970), ("草嶼山", 119.7103, 25.3645), ("東庠島山", 119.8979, 25.5911)];
        for f in d.peaks["features"].as_array().cloned().unwrap_or_default() {
            let e = f["properties"]["e"].as_f64().unwrap_or(0.0);
            if e <= 120.0 { continue; }
            let c = &f["geometry"]["coordinates"];
            let (lon, lat) = (c[0].as_f64().unwrap_or(0.0), c[1].as_f64().unwrap_or(0.0));
            let nm = names.iter().find(|n| (n.1 - lon).abs() < 0.002 && (n.2 - lat).abs() < 0.002).map(|n| n.0.to_string()).unwrap_or_default();
            out.push(Label { p: wp(lon, lat, 0.0), text: format!("▲ {nm} {e:.0} m"), kind: LKind::Peak, pri: 2, max_d: 25000.0, min_d: 0.0, color: Color32::from_rgb(0x6b, 0x4a, 0x26), glyph: None, idx: 0, year: 0, lines: vec![] });
        }
        self.labels = out;
    }

    fn build_outlines(&mut self) {
        let d = &self.d;
        // 沿多邊形邊每 40 m 取樣並貼地
        let ring = |r: &Value| -> Vec<Vec3> {
            let pts: Vec<(f64, f64)> = r.as_array().cloned().unwrap_or_default().iter().map(|p| d.geo.to_local(p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0))).collect();
            let mut out = vec![];
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                let n = ((((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() / 40.0).ceil() as usize).max(1);
                for k in 0..n {
                    let t = k as f64 / n as f64;
                    let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                    out.push(Vec3::new(x as f32, d.hg.at(x, y).max(0.0) + 4.0, -y as f32));
                }
            }
            if let Some(f) = out.first().cloned() { out.push(f); }
            out
        };
        let mut v = vec![];
        for (i, dd) in d.story["DISTRICTS"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let col = hex32(&s(&dd["color"]));
            v.push(Outline { pts: ring(&dd["poly"]), color: col, kind: 0, idx: i, key: s(&dd["id"]) });
            for e in dd["extra"].as_array().cloned().unwrap_or_default() { v.push(Outline { pts: ring(&e), color: col, kind: 0, idx: i, key: s(&dd["id"]) }); }
        }
        for z in d.zone.as_array().cloned().unwrap_or_default() { v.push(Outline { pts: ring(&z), color: Color32::from_rgb(0xc2, 0x3b, 0x3b), kind: 1, idx: 0, key: "zone".into() }); }
        if let Some(m) = d.sites.as_object() {
            for (k, r) in m { v.push(Outline { pts: ring(r), color: Color32::from_rgb(0x4a, 0x55, 0x60), kind: 2, idx: self.lms.iter().position(|l| &l.id == k).unwrap_or(usize::MAX), key: k.clone() }); }
        }
        self.outlines = v;
    }

    fn draw_outlines(&self, p: &egui::Painter, rect: Rect) {
        let story = self.year >= 2016;
        let sel_lm = if let Sel::Landmark(i) = self.sel { Some(i) } else { None };
        for o in &self.outlines {
            let (w, dash) = match o.kind {
                0 => {
                    if !story || !(self.bounds || self.bound_sel.is_some()) { continue; }
                    if let Some(b) = self.bound_sel { if b != o.idx { continue; } }
                    (if self.bound_sel.is_some() { 3.2 } else { 2.2 }, false)
                }
                1 => { if !self.zone_line || !story { continue; } (2.4, true) }
                _ => {
                    let l = self.lms.get(o.idx);
                    let built = l.map(|l| l.built <= self.year && story).unwrap_or(false);
                    let sel = sel_lm == Some(o.idx);
                    if !built || !self.layers[0] || !(sel || self.cam.dist < 2600.0) { continue; }
                    (if sel { 2.4 } else { 1.3 }, true)
                }
            };
            // 投影；鏡頭後方的點切斷折線
            let mut runs: Vec<Vec<Pos2>> = vec![vec![]];
            for q in &o.pts {
                match self.cam.project(*q) {
                    Some(sp) => runs.last_mut().unwrap().push(rect.min + Vec2::new(sp.x, sp.y)),
                    None => if !runs.last().unwrap().is_empty() { runs.push(vec![]); },
                }
            }
            for r in runs.iter().filter(|r| r.len() > 1) {
                if !r.iter().any(|q| rect.expand(200.0).contains(*q)) { continue; }
                let halo = Stroke::new(w + 2.5, Color32::from_rgba_unmultiplied(255, 255, 255, 150));
                if dash {
                    p.extend(egui::Shape::dashed_line(r, halo, 9.0, 6.0));
                    p.extend(egui::Shape::dashed_line(r, Stroke::new(w, o.color), 9.0, 6.0));
                } else {
                    p.add(egui::Shape::line(r.clone(), halo));
                    p.add(egui::Shape::line(r.clone(), Stroke::new(w, o.color)));
                }
            }
        }
    }

    fn line_on(&self, id: &str) -> bool {
        // 運輸模式／總圖：預設全部顯示，點選切換隱藏；其他模式：預設純地圖，點選切換顯示
        if matches!(self.mode, Mode::Transit | Mode::Overview) { !self.lines_hide.contains(id) } else { self.lines_on.contains(id) }
    }

    pub fn set_time(&mut self, t: Tod) {
        self.tod = t;
        let lp = light_preset(t);
        let g = &mut self.r.globals;
        let sd = lp.sun_dir.normalize();
        g.sun_dir = [sd.x, sd.y, sd.z, lp.night];
        g.sun_col = lin4(lp.sun, g.sun_col[3]);
        g.sky_col = lin4(lp.sky, g.sky_col[3]);
        g.ground_col = lin4(lp.ground, g.ground_col[3]);
        g.fog_col = lin4(lp.fog, g.fog_col[3]);
        g.conc = lin4(lp.conc, 0.0);
        g.glass = lin4(lp.glass, 0.0);
        for (i, c) in lp.types.iter().enumerate() { g.type_col[i] = lin4(c, 0.0); }
    }

    fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        self.orbit = false;
        if m == Mode::Overview {
            // 總圖：整座島的鳥瞰＋所有路線與交通樞紐
            let (x, y) = self.d.geo.to_local(119.772, 25.535);
            self.cam.fly_to(x as f32, y as f32 - 9000.0, Some(52000.0), Some(-12.0), Some(48.0), 1.4);
            self.side_open = true;
            self.sel = Sel::None;
            return;
        }
        let top = m != Mode::ThreeD;
        let dist = match m { Mode::Transit => Some(self.cam.dist.max(26000.0)), Mode::Cs2 => Some(self.cam.dist.max(70000.0)), _ => None };
        let (x, y) = if m == Mode::Cs2 { let p = self.d.geo.to_local(self.cs2.0, self.cs2.1); (p.0 as f32, p.1 as f32) } else { (self.cam.target.x, -self.cam.target.z) };
        self.cam.fly_to(x, y, dist, if top { Some(0.0) } else { None }, Some(if top { 89.9 } else { 55.0 }), 0.9);
        self.side_open = true;
        self.sel = Sel::None;
    }

    fn fly_ll(&mut self, lon: f64, lat: f64, dist: f32, pitch: Option<f32>) {
        let (x, y) = self.d.geo.to_local(lon, lat);
        self.cam.fly_to(x as f32, y as f32, Some(dist), None, pitch, 1.6);
    }

    fn select_landmark(&mut self, i: usize, fly: bool) {
        self.sel = Sel::Landmark(i);
        self.side_open = true;
        if fly {
            let l = &self.lms[i];
            let (at, d) = (l.at, (l.h * 2.6 + l.fp.0 * 12.0).max(450.0));
            self.fly_ll(at.0, at.1, d, if self.mode == Mode::ThreeD { Some(38.0) } else { None });
        }
    }

    fn update_globals(&mut self, t: f32) {
        // 群組可見度：0 永遠；地標依圖層；路線依開關／聚焦
        let mut vis = [[1.0f32; 4]; 16];
        for (i, gname) in self.d.scene_meta.groups.iter().enumerate().take(64) {
            let v = if let Some(id) = gname.strip_prefix("line:") {
                if !self.line_on(id) { 0.0 } else if self.focus_line.as_deref().map(|f| f != id).unwrap_or(false) { 0.25 } else { 1.0 }
            } else if gname.starts_with("lm") { if self.layers[0] { 1.0 } else { 0.0 } } else { 1.0 };
            vis[i / 4][i % 4] = v;
        }
        let c = &self.cam;
        let g = &mut self.r.globals;
        g.view_proj = c.vp.to_cols_array_2d();
        g.cam_pos = [c.pos.x, c.pos.y, c.pos.z, t];
        let top = ((c.pitch - 1.0) / 0.5).clamp(0.0, 1.0);
        g.sun_col[3] = 1.0 / (c.dist * 1.2 + 10000.0) * (1.0 - 0.65 * top);
        g.sky_col[3] = self.year as f32;
        g.day = [self.day, t, 0.0, 0.0];
        g.ground_col[3] = match self.mode { Mode::Contour => 2.0, Mode::Transit => 1.0, _ => 0.0 };
        g.fog_col[3] = match self.mode { Mode::Transit => 0.85, Mode::Contour => 0.6, _ => 0.0 };
        let (cx, cy) = self.d.geo.to_local(self.cs2.0, self.cs2.1);
        g.cs2 = [cx as f32, cy as f32, self.cs2.2 / 2.0, self.cs2.3.to_radians()];
        g.cs2b = [(21.0 * self.cs2.2 / 14336.0).round(), if self.mode == Mode::Cs2 { 1.0 } else { 0.0 }, if self.mode == Mode::Transit && self.layers[3] { 1.0 } else { 0.0 }, c.meters_per_pixel()];
        g.tex[3] = c.vh;
        g.groups = vis;
    }

    /// 繪製 3D 場景到離屏貼圖並交給 egui 顯示。
    fn render(&mut self, rs: &egui_wgpu::RenderState, size_px: (u32, u32), t: f32) {
        let need = self.target.as_ref().map(|tg| tg.size != size_px).unwrap_or(true);
        if need {
            let tg = Target::new(&rs.device, size_px.0, size_px.1);
            let mut er = rs.renderer.write();
            match self.tex {
                Some(id) => er.update_egui_texture_from_wgpu_texture(&rs.device, &tg.color_view, wgpu::FilterMode::Linear, id),
                None => self.tex = Some(er.register_native_texture(&rs.device, &tg.color_view, wgpu::FilterMode::Linear)),
            }
            self.target = Some(tg);
        }
        self.update_globals(t);
        let vp = self.cam.vp;
        self.r.update(&rs.device, &rs.queue, &vp, self.cam.pos);
        self.r.write_globals(&rs.queue);
        let fog = self.r.globals.fog_col;
        let mut enc = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let show_lines = self.d.scene_meta.lines.iter().any(|l| self.line_on(&l.id));
        self.r.draw(&mut enc, self.target.as_ref().unwrap(), &vp, self.cam.pos, [fog[0] as f64, fog[1] as f64, fog[2] as f64], show_lines, true);
        rs.queue.submit([enc.finish()]);
    }

    fn handle_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect, dt: f32) {
        let ctx = ui.ctx();
        let (pointer, scroll, mods, keys_down) = ctx.input(|i| (i.pointer.clone(), i.smooth_scroll_delta.y, i.modifiers, i.keys_down.clone()));
        let to_local = |p: Pos2| (p.x - rect.min.x, p.y - rect.min.y);
        if resp.drag_started() || scroll != 0.0 { self.cam.anim = None; self.orbit = false; self.tour = None; }
        let primary = pointer.primary_down();
        let secondary = pointer.secondary_down() || pointer.middle_down();
        if resp.dragged() {
            let dlt = resp.drag_delta();
            let rot = secondary || mods.shift || mods.ctrl || mods.alt || (primary && self.left_tilt);
            if rot {
                self.cam.yaw -= dlt.x * 0.005;
                self.cam.pitch = (self.cam.pitch + dlt.y * 0.004).clamp(self.cam.min_pitch, self.cam.max_pitch);
            } else if let Some(pos) = pointer.interact_pos() {
                // 地面點保持在游標下（以水平面近似）
                let (px, py) = to_local(pos);
                let (qx, qy) = to_local(pos - dlt);
                let gy = self.cam.target.y;
                let hit = |sx: f32, sy: f32| { let (o, d) = self.cam.ray(sx, sy); if d.y < -1e-3 { let t = (gy - o.y) / d.y; Some(o + d * t) } else { None } };
                if let (Some(a), Some(b)) = (hit(qx, qy), hit(px, py)) {
                    self.cam.target.x += a.x - b.x;
                    self.cam.target.z += a.z - b.z;
                }
            }
        }
        if resp.hovered() && scroll != 0.0 {
            let f = (-scroll * 0.0025).exp();
            let nd = (self.cam.dist * f).clamp(60.0, 160000.0);
            if let Some(pos) = pointer.hover_pos() {
                let (sx, sy) = to_local(pos);
                if let Some(g) = self.cam.ground_at(&self.d.hg, sx, sy) {
                    let k = 1.0 - nd / self.cam.dist;
                    self.cam.target.x += (g.x - self.cam.target.x) * k;
                    self.cam.target.z += (g.z - self.cam.target.z) * k;
                }
            }
            self.cam.dist = nd;
        }
        // 鍵盤（實體鍵，與輸入法無關）
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing {
            use egui::Key as K;
            let s = self.cam.dist * 0.9 * dt;
            let (fx, fz, rx, rz) = (-self.cam.yaw.sin(), -self.cam.yaw.cos(), self.cam.yaw.cos(), -self.cam.yaw.sin());
            let k = |key: K| keys_down.contains(&key);
            let mut any = false;
            if k(K::W) || k(K::ArrowUp) { self.cam.target.x += fx * s; self.cam.target.z += fz * s; any = true; }
            if k(K::S) || k(K::ArrowDown) { self.cam.target.x -= fx * s; self.cam.target.z -= fz * s; any = true; }
            if k(K::D) || k(K::ArrowRight) { self.cam.target.x += rx * s; self.cam.target.z += rz * s; any = true; }
            if k(K::A) || k(K::ArrowLeft) { self.cam.target.x -= rx * s; self.cam.target.z -= rz * s; any = true; }
            if k(K::Q) { self.cam.yaw += dt * 1.2; any = true; }
            if k(K::E) { self.cam.yaw -= dt * 1.2; any = true; }
            if k(K::R) { self.cam.pitch = (self.cam.pitch + dt).min(self.cam.max_pitch); any = true; }
            if k(K::F) { self.cam.pitch = (self.cam.pitch - dt).max(self.cam.min_pitch); any = true; }
            if k(K::Equals) || k(K::Plus) { self.cam.dist = (self.cam.dist * (1.0 - dt * 1.5)).max(60.0); any = true; }
            if k(K::Minus) { self.cam.dist = (self.cam.dist * (1.0 + dt * 1.5)).min(160000.0); any = true; }
            if any { self.cam.anim = None; self.orbit = false; }
            ctx.input(|i| {
                for (key, m) in [(K::Num1, Mode::Plan), (K::Num2, Mode::ThreeD), (K::Num3, Mode::Contour), (K::Num4, Mode::Transit), (K::Num5, Mode::Cs2), (K::Num6, Mode::Overview)] {
                    if i.key_pressed(key) { self.keys.insert(key); let _ = m; }
                }
            });
            let pressed: Vec<(egui::Key, Mode)> = [(K::Num1, Mode::Plan), (K::Num2, Mode::ThreeD), (K::Num3, Mode::Contour), (K::Num4, Mode::Transit), (K::Num5, Mode::Cs2), (K::Num6, Mode::Overview)].into_iter().filter(|(k, _)| self.keys.remove(k)).collect();
            for (_, m) in pressed { self.set_mode(m); }
            if ctx.input(|i| i.key_pressed(K::T)) { self.left_tilt = !self.left_tilt; }
            if ctx.input(|i| i.key_pressed(K::M)) { self.measure = if self.measure.is_some() { None } else { Some(vec![]) }; }
            if ctx.input(|i| i.key_pressed(K::Escape)) { self.measure = None; self.tour = None; self.orbit = false; self.interior = None; }
        }
        // 量測：右鍵刪除上一點，Shift＋右鍵全部清除
        if resp.secondary_clicked() {
            if let Some(m) = &mut self.measure { if mods.shift || mods.ctrl { m.clear(); } else { m.pop(); } }
        }
        // 點擊
        if resp.clicked() {
            if let Some(pos) = pointer.interact_pos() {
                let (sx, sy) = to_local(pos);
                if let Some(m) = &mut self.measure {
                    if let Some(g) = self.cam.ground_at(&self.d.hg, sx, sy) { m.push(g); }
                } else {
                    // 地標（以投影位置判斷）
                    let mut best: Option<(usize, f32)> = None;
                    if self.layers[0] {
                        for (i, l) in self.lms.iter().enumerate() {
                            if l.built > self.year { continue; }
                            let (x, y) = self.d.geo.to_local(l.at.0, l.at.1);
                            let p = Vec3::new(x as f32, self.d.hg.at(x, y).max(0.0) + l.h * 0.4, -y as f32);
                            if let Some(s2) = self.cam.project(p) {
                                let dd = ((s2.x - sx).powi(2) + (s2.y - sy).powi(2)).sqrt();
                                if dd < 30.0 && best.map(|b| dd < b.1).unwrap_or(true) { best = Some((i, dd)); }
                            }
                        }
                    }
                    if let Some((i, _)) = best { self.select_landmark(i, false); }
                }
            }
        }
        // 滑鼠停留：建物資訊
        if let Some(pos) = pointer.hover_pos() {
            if (pos - self.last_mouse).length() > 1.0 { self.hover = None; self.hover_t = 0.0; self.last_mouse = pos; }
            else if resp.hovered() && self.hover.is_none() && self.cam.dist < 3500.0 && !resp.dragged() {
                self.hover_t += dt;
                if self.hover_t > 0.18 {
                    let (sx, sy) = to_local(pos);
                    self.hover = self.pick_building(sx, sy).map(|t| (pos, t));
                    self.hover_t = -1e9;
                }
            }
        }
    }

    fn pick_building(&self, sx: f32, sy: f32) -> Option<String> {
        let (o, dir) = self.cam.ray(sx, sy);
        let mut best: Option<(f32, &crate::mesher::BInfo)> = None;
        let yr = self.year;
        for c in self.r.chunks.values() {
            let Some(info) = &c.info else { continue };
            let dc = ((c.cx - o.x).powi(2) + (c.cz - o.z).powi(2)).sqrt();
            if dc > 6000.0 { continue; }
            for b in &info.b {
                let _ = yr;
                if b.e > 0 && self.day < b.s as f32 { continue; }
                if b.dd > 0 && self.day >= b.dd as f32 { continue; }
                // 射線與稜柱：取外框 bbox 粗判後，以高度切片點在多邊形內判斷
                let (mnx, mxx) = b.xs.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
                let (mny, mxy) = b.ys.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
                // 射線在 xz 平面上與 bbox 相交區間
                let (mut t0, mut t1) = (0.0f32, 1e7f32);
                for (oo, dd, lo, hi) in [(o.x, dir.x, mnx, mxx), (-o.z, -dir.z, mny, mxy), (o.y, dir.y, b.z0, b.z1)] {
                    if dd.abs() < 1e-6 { if oo < lo || oo > hi { t0 = 1.0; t1 = 0.0; } continue; }
                    let (a, bb) = ((lo - oo) / dd, (hi - oo) / dd);
                    t0 = t0.max(a.min(bb));
                    t1 = t1.min(a.max(bb));
                }
                if t0 > t1 { continue; }
                // 在區間內取樣，找第一個落在多邊形內的點
                let n = 12;
                for k in 0..=n {
                    let t = t0 + (t1 - t0) * k as f32 / n as f32;
                    let p = o + dir * t;
                    if pip(&b.xs, &b.ys, p.x, -p.z) && p.y >= b.z0 && p.y <= b.z1 + 0.1 {
                        if best.map(|q| t < q.0).unwrap_or(true) { best = Some((t, b)); }
                        break;
                    }
                }
            }
        }
        let (_, b) = best?;
        let (zh, en) = type_cs2(b.t);
        let mut s = zh.to_string();
        if b.uw > 0 { s += &format!("\n地塊 {}u × {}u（{}×{} m）", b.uw, b.ud, b.uw as u32 * 8, b.ud as u32 * 8); }
        s += &format!("\n{}F・高 {:.0} m", b.floors, b.h);
        if self.mode == Mode::Cs2 { s += &format!("\nCS2：{en}"); }
        if b.e == 0 { s += "\n戰前既有建物"; } else if self.day < b.e as f32 {
            s += &format!("\n施工中 {:.0}%（{} 開工，預計 {} 完工）", ((self.day - b.s as f32) / (b.e - b.s).max(1) as f32 * 100.0).clamp(0.0, 100.0), date_of(b.s as f32), date_of(b.e as f32));
        } else { s += &format!("\n{} 開工・{} 竣工", date_of(b.s as f32), date_of(b.e as f32)); }
        if b.dd > 0 { s += &format!("\n{} 拆除（都市更新）", date_of(b.dd as f32)); }
        Some(s)
    }

    // ---------------- 介面 ----------------
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, upd: &Updater, upd_show: &mut bool) {
        let ctx = ui.ctx().clone();
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let t = ctx.input(|i| i.time) as f32;
        // FPS 與動態解析度
        self.fps_acc.0 += dt;
        self.fps_acc.1 += 1;
        if self.fps_acc.0 >= 1.0 {
            self.fps = self.fps_acc.1 as f32 / self.fps_acc.0;
            self.fps_acc = (0.0, 0);
            if self.auto_res {
                if self.fps < 40.0 { self.scale = (self.scale - if self.fps < 25.0 { 0.15 } else { 0.07 }).max(0.5); }
                else if self.fps > 57.0 { self.scale = (self.scale + 0.05).min(1.0); }
            }
        }
        if self.playing {
            self.play_t += dt;
            self.day += dt * SPEEDS[self.speed].0;
            if self.day >= END_DAY { self.day = END_DAY; self.playing = false; }
        }
        self.year = 2014 + (self.day / 365.25) as i32;
        if self.orbit { self.cam.yaw += dt * 0.18; }
        // 3D 視圖：全畫面
        let full = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let resp = ui.interact(full, egui::Id::new("view3d"), Sense::click_and_drag());
        self.handle_input(ui, &resp, full, dt);
        let moving = self.cam.update(&self.d.hg, dt, full.width(), full.height());
        if let Some(rs) = frame.wgpu_render_state() {
            let px = ((full.width() * ppp * self.scale) as u32, (full.height() * ppp * self.scale) as u32);
            let t0 = std::time::Instant::now();
            self.render(rs, px, t);
            self.frame_ms = self.frame_ms * 0.9 + t0.elapsed().as_secs_f32() * 1000.0 * 0.1;
        }
        let painter = ui.painter_at(full);
        if let Some(id) = self.tex {
            painter.image(id, full, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        self.draw_outlines(&painter, full);
        self.draw_labels(&painter, full);
        self.draw_measure(&painter, full);
        let _ = moving;
        // 浮動面板
        self.top_bar(&ctx);
        self.tool_bar(&ctx);
        self.bottom_bar(&ctx);
        if self.tl_open { self.timeline(&ctx); } else { self.timeline_tab(&ctx); }
        self.side_panel(&ctx);
        if let Some(i) = self.tour { self.tour_caption(&ctx, i); }
        if let Some((p, s)) = &self.hover {
            egui::Area::new(egui::Id::new("tip")).fixed_pos(*p + Vec2::new(14.0, 14.0)).order(egui::Order::Tooltip).interactable(false).show(&ctx, |ui| {
                panel_frame().show(ui, |ui| { ui.label(egui::RichText::new(s).size(12.5)); });
            });
        }
        if let Some(iv) = &mut self.interior {
            let mut open = true;
            if let Some(rs) = frame.wgpu_render_state() { iv.window(&ctx, rs, &mut self.r, &self.d, &mut open); }
            if !open { self.interior = None; }
        }
        self.update_banner(&ctx, upd, upd_show);
        ctx.request_repaint();
    }

    fn draw_labels(&self, p: &egui::Painter, rect: Rect) {
        let mut items: Vec<(&Label, f32)> = vec![];
        let story = self.year >= 2016;
        for l in &self.labels {
            let on = match l.kind {
                LKind::Landmark => self.layers[0] && l.year <= self.year && (story || l.year < 2000),
                LKind::Station => story && l.year <= self.year && l.lines.iter().any(|id| self.line_on(id)),
                LKind::District => story && self.layers[2],
                LKind::Island | LKind::Village => self.layers[1],
                LKind::Peak => matches!(self.mode, Mode::Contour | Mode::ThreeD),
            };
            if !on { continue; }
            let d = (l.p - self.cam.pos).length();
            let ov = self.mode == Mode::Overview;
            if ov && l.kind == LKind::Village { continue; }
            let maxd = if l.kind == LKind::Station && ov { if l.lines.len() > 1 || l.text.ends_with("站") || l.text == "機場" { 120000.0 } else { 9000.0 } }
                else if l.kind == LKind::Landmark && ov { if l.pri >= 9 { 120000.0 } else { 0.0 } }
                else if l.kind == LKind::Station && self.mode == Mode::Transit { 30000.0 } else if l.kind == LKind::Peak && self.mode == Mode::Contour { 60000.0 } else { l.max_d };
            if d > maxd || d < l.min_d { continue; }
            items.push((l, d));
        }
        items.sort_by(|a, b| b.0.pri.cmp(&a.0.pri).then(a.1.partial_cmp(&b.1).unwrap()));
        let mut placed: Vec<Rect> = vec![];
        for (l, _) in items {
            let Some(s) = self.cam.project(l.p) else { continue };
            let pos = rect.min + Vec2::new(s.x, s.y);
            if !rect.expand(40.0).contains(pos) { continue; }
            let fs = match l.kind { LKind::District => 16.0, LKind::Landmark => 12.5, LKind::Island => 12.5, _ => 11.5 };
            let font = FontId::proportional(fs);
            let galley = p.layout_no_wrap(l.text.clone(), font.clone(), l.color);
            let extra = if l.glyph.is_some() { 26.0 } else if l.kind == LKind::Station { 14.0 } else { 0.0 };
            let w = galley.size().x + extra + if l.kind == LKind::Landmark { 14.0 } else { 0.0 };
            let h = galley.size().y + if l.kind == LKind::Landmark { 6.0 } else { 0.0 };
            let r = Rect::from_min_size(Pos2::new(pos.x - w / 2.0, pos.y - h - if l.kind == LKind::Landmark { 7.0 } else { 0.0 }), Vec2::new(w, h));
            if placed.iter().any(|q| q.intersects(r)) { continue; }
            placed.push(r);
            if placed.len() > 140 { break; }
            let night = self.tod == Tod::Night;
            match l.kind {
                LKind::Landmark => {
                    let sel = self.sel == Sel::Landmark(l.idx);
                    let bg = if sel { INK } else if night { Color32::from_rgba_unmultiplied(16, 24, 34, 230) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 238) };
                    p.line_segment([Pos2::new(pos.x, r.max.y), pos], Stroke::new(1.0, Color32::from_rgba_unmultiplied(22, 32, 42, 130)));
                    p.rect(r, 99.0, bg, Stroke::new(1.0, Color32::from_rgba_unmultiplied(22, 32, 42, 45)), egui::StrokeKind::Inside);
                    if let Some((g, c)) = &l.glyph {
                        let cc = Pos2::new(r.min.x + 12.0, r.center().y);
                        p.circle_filled(cc, 9.0, *c);
                        p.text(cc, Align2::CENTER_CENTER, g, FontId::proportional(10.0), Color32::WHITE);
                    }
                    let tc = if sel || night { Color32::from_rgb(234, 242, 247) } else { INK };
                    p.text(Pos2::new(r.min.x + 25.0, r.center().y), Align2::LEFT_CENTER, &l.text, font, tc);
                }
                _ => {
                    let halo = if night { Color32::from_rgb(11, 21, 34) } else { Color32::WHITE };
                    let tc = if night && l.kind != LKind::District { Color32::from_rgb(219, 230, 239) } else { l.color };
                    let mut tp = Pos2::new(r.min.x + extra, r.center().y);
                    if l.kind == LKind::Station {
                        let c = Pos2::new(r.min.x + 6.0, r.center().y);
                        p.circle_filled(c, 5.0, Color32::WHITE);
                        p.circle_stroke(c, 4.5, Stroke::new(2.5, l.color));
                    }
                    if l.kind == LKind::District { tp = Pos2::new(r.min.x, r.center().y); }
                    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                        p.text(tp + Vec2::new(dx, dy), Align2::LEFT_CENTER, &l.text, font.clone(), halo);
                    }
                    p.text(tp, Align2::LEFT_CENTER, &l.text, font, tc);
                }
            }
        }
    }

    fn draw_measure(&self, p: &egui::Painter, rect: Rect) {
        let Some(m) = &self.measure else { return };
        let pts: Vec<Pos2> = m.iter().filter_map(|w| self.cam.project(*w + Vec3::Y * 6.0)).map(|s| rect.min + Vec2::new(s.x, s.y)).collect();
        for w in pts.windows(2) { p.line_segment([w[0], w[1]], Stroke::new(2.5, WARN)); }
        for q in &pts { p.circle_filled(*q, 4.0, WARN); }
        let mut d = 0.0;
        for w in m.windows(2) { d += ((w[1].x - w[0].x).powi(2) + (w[1].z - w[0].z).powi(2)).sqrt(); }
        if let Some(last) = pts.last() {
            p.text(*last + Vec2::new(10.0, -10.0), Align2::LEFT_BOTTOM, format!("{:.0} m・{:.1} u", d, d / 8.0), FontId::proportional(13.0), WARN);
        }
        if m.len() < 2 || true {
            p.text(rect.center_bottom() - Vec2::new(0.0, 250.0), Align2::CENTER_BOTTOM, "量測：左鍵加點・右鍵刪除上一點・Shift＋右鍵全部清除・Esc 結束", FontId::proportional(13.0), INK);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("crest")).anchor(Align2::LEFT_TOP, Vec2::new(12.0, 10.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                    ui.painter().circle_stroke(r.center(), 17.0, Stroke::new(1.5, INK));
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, "麗", FontId::proportional(17.0), INK);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("麗京特別市").size(18.0).strong().color(INK));
                        ui.label(egui::RichText::new(format!("LIJING · 白城 · 原生版 v{VERSION}")).size(10.0).color(MUTED));
                    });
                    ui.add_space(10.0);
                    let st = &self.d.meta.stats;
                    let km: f64 = st["roadKm"].as_object().map(|o| o.values().filter_map(|v| v.as_f64()).sum()).unwrap_or(0.0);
                    let stat = |ui: &mut egui::Ui, a: String, b: &str| {
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(egui::RichText::new(a).size(14.5).monospace().color(INK)).wrap_mode(egui::TextWrapMode::Extend));
                            ui.add(egui::Label::new(egui::RichText::new(b).size(10.5).color(MUTED)).wrap_mode(egui::TextWrapMode::Extend));
                        });
                    };
                    stat(ui, format!("{:.0} 萬", self.d.story["FACTS"]["pop"].as_f64().unwrap_or(0.0) / 1e4), "常住人口");
                    stat(ui, format!("{:.0} 萬", st["pop"].as_f64().unwrap_or(0.0) / 1e4), "模擬居住");
                    stat(ui, format!("{:.0} 萬", st["jobs"].as_f64().unwrap_or(0.0) / 1e4), "就業崗位");
                    stat(ui, format!("{:.0} km", km), "新設道路");
                    stat(ui, format!("{} 線 {} 站", self.d.scene_meta.lines.len(), self.d.scene_meta.stations.len()), "軌道");
                    stat(ui, format!("{:>3.0}", self.fps), "FPS");
                });
            });
        });
        egui::Area::new(egui::Id::new("search")).anchor(Align2::RIGHT_TOP, Vec2::new(-12.0, 10.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width(300.0);
                let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("搜尋地標、車站、村落…").desired_width(290.0));
                let q = self.search.trim().to_string();
                if !q.is_empty() {
                    let mut hits: Vec<(String, String, u8, usize)> = vec![];
                    for (i, l) in self.lms.iter().enumerate() { if l.name.contains(&q) { hits.push((l.name.clone(), cat(&l.cat).0.to_string(), 0, i)); } }
                    for (i, st) in self.d.scene_meta.stations.iter().enumerate() { if st.n.contains(&q) { hits.push((format!("{}站", st.n), st.lines.join(" "), 1, i)); } }
                    for (i, l) in self.labels.iter().enumerate() { if l.kind == LKind::Village && l.text.contains(&q) { hits.push((l.text.clone(), "村落".into(), 2, i)); } }
                    hits.truncate(12);
                    for (n, sub, k, i) in hits {
                        if ui.add(egui::Button::new(egui::RichText::new(format!("{n}　{sub}")).size(13.0)).frame(false)).clicked() {
                            match k {
                                0 => self.select_landmark(i, true),
                                1 => { let st = self.d.scene_meta.stations[i].clone(); self.sel = Sel::Station(i); self.side_open = true; self.fly_ll(st.ll[0], st.ll[1], 900.0, None); }
                                _ => { let p = self.labels[i].p; let (lon, lat) = self.d.geo.to_ll(p.x as f64, -p.z as f64); self.fly_ll(lon, lat, 1800.0, None); }
                            }
                            self.search.clear();
                            r.surrender_focus();
                        }
                    }
                }
            });
        });
    }

    fn tool_bar(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("tools")).anchor(Align2::LEFT_CENTER, Vec2::new(12.0, 0.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.vertical(|ui| {
                    for (m, n, k) in [(Mode::Overview, "總圖", "6"), (Mode::Plan, "平面", "1"), (Mode::ThreeD, "立體", "2"), (Mode::Contour, "等高線", "3"), (Mode::Transit, "運輸", "4"), (Mode::Cs2, "CS2 規劃", "5")] {
                        if tool_button(ui, n, k, self.mode == m).clicked() { self.set_mode(m); }
                    }
                    ui.separator();
                    if tool_button(ui, "量測", "M", self.measure.is_some()).clicked() { self.measure = if self.measure.is_some() { None } else { Some(vec![]) }; }
                });
            });
        });
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("bottom")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -12.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("時代").size(11.0).color(MUTED));
                    if seg(ui, "戰前 2014", self.year < 2016) { self.day = 0.0; self.playing = false; }
                    if seg(ui, "麗京 2025", self.year >= 2016) { self.day = END_DAY; self.playing = false; }
                    ui.separator();
                    ui.label(egui::RichText::new("光線").size(11.0).color(MUTED));
                    for (t, n) in [(Tod::Dawn, "晨"), (Tod::Day, "日"), (Tod::Dusk, "暮"), (Tod::Night, "夜")] {
                        if seg(ui, n, self.tod == t) { self.set_time(t); }
                    }
                    ui.separator();
                    if seg(ui, "分區邊界", self.bounds || self.bound_sel.is_some()) { if self.bound_sel.is_some() { self.bound_sel = None; self.bounds = false; } else { self.bounds = !self.bounds; } }
                    if seg(ui, if self.left_tilt { "左鍵：轉視角" } else { "左鍵：平移" }, self.left_tilt) { self.left_tilt = !self.left_tilt; }
                    if seg(ui, &format!("解析度 {:.0}%{}", self.scale * 100.0, if self.auto_res { "（自動）" } else { "" }), false) {
                        if self.auto_res { self.auto_res = false; self.scale = 1.0; } else if self.scale >= 1.0 { self.scale = 0.7; } else { self.auto_res = true; }
                    }
                    if ui.add(egui::Button::new(egui::RichText::new("城市導覽").color(Color32::WHITE)).fill(ACCENT)).clicked() { self.start_tour(0); }
                });
            });
        });
    }

    fn timeline_tab(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("tltab")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -64.0)).show(ctx, |ui| {
            if ui.add(egui::Button::new(format!("時間軸 {}", date_of(self.day))).fill(PAPER)).clicked() { self.tl_open = true; }
        });
    }

    fn timeline(&mut self, ctx: &egui::Context) {
        let stages = stages();
        egui::Area::new(egui::Id::new("timeline")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -64.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width(720.0f32.min(ctx.content_rect().width() - 520.0).max(360.0));
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(if self.playing { "❚❚" } else { "▶" }).min_size(Vec2::splat(36.0))).clicked() {
                        self.playing = !self.playing;
                        if self.playing && self.day >= END_DAY - 1.0 { self.day = 0.0; }
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(date_of(self.day)).size(19.0).strong().monospace());
                            for (a, b, n, _) in stages.iter() {
                                let on = self.year >= *a && self.year <= *b;
                                if ui.add(egui::Button::new(egui::RichText::new(*n).size(11.0).color(if on { INK } else { MUTED }).strong()).frame(false)).clicked() { self.day = day_of_year(*a); self.playing = false; }
                            }
                            if ui.small_button("收合").clicked() { self.tl_open = false; }
                        });
                        let mut dd = self.day;
                        ui.spacing_mut().slider_width = ui.available_width() - 20.0;
                        if ui.add(egui::Slider::new(&mut dd, 0.0..=END_DAY).show_value(false)).changed() { self.day = dd.round(); self.playing = false; self.year = 2014 + (self.day / 365.25) as i32; }
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("播放速度").size(11.0).color(MUTED));
                            for (k, (_, n)) in SPEEDS.iter().enumerate() { if seg(ui, n, self.speed == k) { self.speed = k; } }
                        });
                        let st = stages.iter().find(|s| self.year >= s.0 && self.year <= s.1).unwrap_or(&stages[stages.len() - 1]);
                        ui.label(egui::RichText::new(st.2).strong().size(13.0));
                        ui.label(egui::RichText::new(st.3).size(12.0).color(MUTED));
                        if self.year >= 2016 {
                            let lm: Vec<String> = self.lms.iter().filter(|l| l.built == self.year).map(|l| l.name.clone()).collect();
                            let ln: Vec<String> = self.d.scene_meta.lines.iter().filter(|l| l.y == self.year).map(|l| l.name.clone()).collect();
                            let mut ex = vec![];
                            if !lm.is_empty() { ex.push(format!("竣工：{}", lm.join("、"))); }
                            if !ln.is_empty() { ex.push(format!("通車：{}", ln.join("、"))); }
                            if !ex.is_empty() { ui.label(egui::RichText::new(ex.join("　")).size(11.0).color(ACCENT)); }
                        }
                    });
                });
            });
        });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        let h = ctx.content_rect().height() - 170.0;
        // 向右收合的滑動動畫；左緣的箭頭頁籤可開合
        let t = ctx.animate_bool_with_time(egui::Id::new("side_anim"), self.side_open, 0.22);
        let off = (1.0 - t) * (self.side_w + 16.0);
        let tab_x = -12.0 - (self.side_w + 6.0) * t;
        egui::Area::new(egui::Id::new("sidetab")).anchor(Align2::RIGHT_TOP, Vec2::new(tab_x, 84.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::new().fill(Color32::from_rgba_unmultiplied(247, 249, 250, 242)).corner_radius(egui::CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }).stroke(Stroke::new(1.0, LINE)).inner_margin(egui::Margin::symmetric(2, 10)).show(ui, |ui| {
                let r = ui.add(egui::Button::new(egui::RichText::new(if self.side_open { "›" } else { "‹" }).size(18.0).color(INK)).frame(false).min_size(Vec2::new(18.0, 40.0)));
                if r.on_hover_text(if self.side_open { "收合資訊面板" } else { "展開資訊面板" }).clicked() { self.side_open = !self.side_open; }
            });
        });
        if t < 0.002 { return; }
        let resp = egui::Area::new(egui::Id::new("side")).anchor(Align2::RIGHT_TOP, Vec2::new(-12.0 + off, 72.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width(330.0);
                ui.set_max_height(h);
                let sel = self.sel.clone();
                let (eb, title, sub) = match &sel {
                    Sel::Landmark(i) => { let l = &self.lms[*i]; (district_name(&self.d.story, &l.d), l.name.clone(), l.en.clone()) }
                    Sel::Station(i) => { let st = &self.d.scene_meta.stations[*i]; ("車站".into(), format!("{}站", st.n), if st.lines.len() > 1 { format!("{} 線轉乘", st.lines.len()) } else { String::new() }) }
                    _ => match self.mode {
                        Mode::Transit => ("運輸模式".into(), "大眾運輸與聯外交通".into(), "點選路線可聚焦".into()),
                        Mode::Contour => ("等高線模式".into(), "地形與高程".into(), "AWS Terrain Tiles（約 17 m）".into()),
                        Mode::Cs2 => ("CS2 規劃模式".into(), "《城市天際線2》尺度".into(), "1u = 8 m（一個分區格）".into()),
                        Mode::Plan => ("平面模式".into(), "俯視地圖".into(), "聯邦麗京特別市".into()),
                        Mode::Overview => ("總圖".into(), "全島鳥瞰與交通樞紐".into(), "所有路線、轉乘站與主要地標".into()),
                        Mode::ThreeD => ("立體模式".into(), "白城全景".into(), "聯邦麗京特別市・Lijing Special Municipality".into()),
                    },
                };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(eb).size(10.5).color(MUTED));
                        ui.label(egui::RichText::new(title).size(18.0).strong());
                        if !sub.is_empty() { ui.label(egui::RichText::new(sub).size(12.0).color(MUTED)); }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.button("×").clicked() { if sel != Sel::None { self.sel = Sel::None } else { self.side_open = false } }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    match sel {
                        Sel::Landmark(i) => self.landmark_body(ui, i),
                        Sel::Station(i) => self.station_body(ui, i),
                        _ => match self.mode {
                            Mode::Transit => self.transit_body(ui),
                            Mode::Contour => self.contour_body(ui),
                            Mode::Cs2 => self.cs2_body(ui),
                            Mode::Overview => self.overview_map_body(ui),
                            _ => self.overview_body(ui),
                        },
                    }
                });
            });
        });
        self.side_w = resp.response.rect.width().max(200.0);
    }

    fn set_line(&mut self, id: &str, v: bool) {
        if matches!(self.mode, Mode::Transit | Mode::Overview) { if v { self.lines_hide.remove(id); } else { self.lines_hide.insert(id.to_string()); } }
        else if v { self.lines_on.insert(id.to_string()); } else { self.lines_on.remove(id); }
    }

    fn line_chips(&mut self, ui: &mut egui::Ui) {
        // 依類別分組：捷運、輕軌、高鐵、幹線公車、接駁公車
        let cats: [(&str, &[&str]); 5] = [("捷運", &["metro", "express"]), ("輕軌", &["lrt"]), ("高鐵", &["hsr"]), ("幹線公車（BRT）", &["brt"]), ("接駁公車", &["feeder", "bus"])];
        for (ci, (cname, types)) in cats.iter().enumerate() {
            let lines: Vec<(String, String, String)> = self.d.scene_meta.lines.iter().filter(|l| types.contains(&l.kind.as_str())).map(|l| (l.id.clone(), l.name.clone(), l.color.clone())).collect();
            if lines.is_empty() { continue; }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(*cname).size(12.5).strong());
                ui.label(egui::RichText::new(format!("{} 條", lines.len())).size(11.0).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("全開／全關").clicked() {
                        let any = lines.iter().any(|(id, _, _)| self.line_on(id));
                        for (id, _, _) in &lines { self.set_line(id, !any); }
                    }
                });
            });
            egui::Grid::new(format!("linechips{ci}")).num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
                for (k, (id, name, c)) in lines.iter().enumerate() {
                    let on = self.line_on(id);
                    let col = hex32(c);
                    let label = if id == "HSR" { "高鐵".to_string() } else if id.starts_with("BUS") { "公車".to_string() } else { id.clone() };
                    let txt = egui::RichText::new(format!("{}  {}", label, name.split_whitespace().last().unwrap_or(""))).size(12.0).color(if on { Color32::WHITE } else { INK });
                    if ui.add(egui::Button::new(txt).fill(if on { col } else { Color32::WHITE }).stroke(Stroke::new(1.5, col)).min_size(Vec2::new(150.0, 26.0))).clicked() {
                        self.set_line(id, !on);
                    }
                    if k % 2 == 1 { ui.end_row(); }
                }
            });
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            let ids: Vec<String> = self.d.scene_meta.lines.iter().map(|l| l.id.clone()).collect();
            if ui.button("全部顯示").clicked() { for id in &ids { self.set_line(id, true); } }
            if ui.button("全部隱藏").clicked() { for id in &ids { self.set_line(id, false); } }
        });
    }

    fn overview_body(&mut self, ui: &mut egui::Ui) {
        let f = self.d.story["FACTS"].clone();
        eyebrow(ui, "城市概況");
        kv(ui, &[("舊名", s(&f["old"])), ("俗稱", format!("{}（CeraCore-9 外牆）", s(&f["alias"]))), ("陸地／海域", format!("{} km² ／ {} km²", f["land"], f["sea"])), ("常住人口", format!("{:.0} 萬", f["pop"].as_f64().unwrap_or(0.0) / 1e4)), ("最高點", s(&f["peak"])), ("主導風", s(&f["wind"]))]);
        ui.add_space(8.0);
        eyebrow(ui, "功能分區");
        for (i, d) in self.d.story["DISTRICTS"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let z = &self.d.meta.stats["byZone"][s(&d["id"])];
            let n = z["bldg"].as_u64().map(|n| format!("{n} 棟")).unwrap_or("保留地".into());
            if list_item(ui, hex32(&s(&d["color"])), &s(&d["name"]), &s(&d["desc"]).chars().take(18).collect::<String>(), &n).clicked() {
                let poly = d["poly"].as_array().cloned().unwrap_or_default();
                let xs: Vec<f64> = poly.iter().map(|p| p[0].as_f64().unwrap_or(0.0)).collect();
                let ys: Vec<f64> = poly.iter().map(|p| p[1].as_f64().unwrap_or(0.0)).collect();
                let (mnx, mxx) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
                let (mny, mxy) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
                let span = ((mxx - mnx) * 100000.0).max((mxy - mny) * 110000.0) as f32;
                self.fly_ll((mnx + mxx) / 2.0, (mny + mxy) / 2.0, span * 1.4, Some(50.0));
                self.bound_sel = Some(i);
            }
        }
        ui.add_space(8.0);
        eyebrow(ui, "圖層");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.layers[0], "地標");
            ui.checkbox(&mut self.layers[1], "地名");
            ui.checkbox(&mut self.layers[2], "功能分區");
            if ui.checkbox(&mut self.bounds, "分區邊界").changed() { self.bound_sel = None; }
            ui.checkbox(&mut self.zone_line, "特區範圍");
        });
        if self.bound_sel.is_some() && ui.button("顯示全部分區").clicked() { self.bound_sel = None; self.bounds = true; }
        ui.add_space(8.0);
        eyebrow(ui, "軌道路線（點選開啟，未開啟時為純地圖）");
        self.line_chips(ui);
        ui.add_space(8.0);
        ui.label(egui::RichText::new("操作：左鍵拖曳平移（T 切換成左鍵轉視角）；右鍵／中鍵／Shift＋拖曳旋轉傾斜；滾輪縮放；WASD 移動、Q／E 旋轉、R／F 傾斜；1–5 切換模式、M 量測。").size(11.5).color(MUTED));
    }

    fn overview_map_body(&mut self, ui: &mut egui::Ui) {
        eyebrow(ui, "交通樞紐（轉乘站，依路線數排序）");
        let mut hubs: Vec<(usize, usize)> = self.d.scene_meta.stations.iter().enumerate().filter(|(_, s)| s.lines.len() > 1).map(|(i, s)| (i, s.lines.len())).collect();
        hubs.sort_by(|a, b| b.1.cmp(&a.1));
        for (i, _) in hubs.into_iter().take(16) {
            let st = self.d.scene_meta.stations[i].clone();
            let col = self.d.scene_meta.lines.iter().find(|l| l.id == st.lines[0]).map(|l| hex32(&l.color)).unwrap_or(INK);
            if list_item(ui, col, &format!("{}站", st.n.trim_end_matches('站')), &st.lines.join("・"), &format!("{} 線", st.lines.len())).clicked() {
                self.sel = Sel::Station(i);
                self.fly_ll(st.ll[0], st.ll[1], 1600.0, Some(45.0));
            }
        }
        ui.add_space(6.0);
        eyebrow(ui, "路線");
        for l in self.d.scene_meta.lines.clone() {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(26.0, 6.0), Sense::hover());
                ui.painter().rect_filled(r, 3.0, hex32(&l.color));
                ui.label(egui::RichText::new(format!("{}　{}", l.name, l.sub)).size(12.0));
            });
        }
        ui.label(egui::RichText::new("左鍵拖曳平移、滾輪縮放；點選樞紐飛過去。按 2 回到一般立體檢視。").size(11.5).color(MUTED));
    }

    fn landmark_body(&mut self, ui: &mut egui::Ui, i: usize) {
        let l = &self.lms[i];
        let (cn, cc, _) = cat(&l.cat);
        ui.horizontal_wrapped(|ui| {
            chip(ui, cn, cc, Color32::WHITE);
            chip(ui, if l.design { "規劃補設計" } else { "聖經設定" }, Color32::from_rgb(0xee, 0xf2, 0xf4), INK);
        });
        ui.label(egui::RichText::new(&l.desc).size(13.0));
        ui.add_space(6.0);
        eyebrow(ui, "CS2 規格");
        let (x, y) = self.d.geo.to_local(l.at.0, l.at.1);
        let g = self.d.hg.at(x, y);
        kv(ui, &[("佔地", format!("{}u × {}u（{} × {} m）", l.fp.0, l.fp.1, l.fp.0 * 8.0, l.fp.1 * 8.0)), ("高度", format!("{} m {}", l.h, l.floors)), ("地面標高", format!("{g:.1} m")), ("竣工", l.built.to_string()), ("座標", format!("{:.4}°N {:.4}°E", l.at.1, l.at.0))]);
        let at = l.at;
        let (h, fp) = (l.h, l.fp);
        let kind = crate::interior::kind_for(&l.id);
        let name = l.name.clone();
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(egui::RichText::new("環繞觀看").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                if self.mode != Mode::ThreeD { self.mode = Mode::ThreeD; }
                self.fly_ll(at.0, at.1, (h * 2.8 + fp.0 * 12.0).max(500.0), Some(32.0));
                self.orbit = true;
            }
            if ui.button("俯視").clicked() { let (x, y) = self.d.geo.to_local(at.0, at.1); self.cam.fly_to(x as f32, y as f32, Some(900.0), Some(0.0), Some(89.9), 1.4); }
            if let Some(k) = kind {
                if ui.button("內部剖面").clicked() { self.interior = Some(InteriorView::new(k, &format!("{name}・內部剖面"), None)); }
            }
        });
    }

    fn station_body(&mut self, ui: &mut egui::Ui, i: usize) {
        let st = self.d.scene_meta.stations[i].clone();
        ui.horizontal_wrapped(|ui| {
            for id in &st.lines {
                if let Some(l) = self.d.scene_meta.lines.iter().find(|l| &l.id == id) { chip(ui, id, hex32(&l.color), Color32::WHITE); }
            }
            chip(ui, if st.kind == "u" || st.kind == "t" { "地下站" } else { "高架站" }, Color32::from_rgb(0xee, 0xf2, 0xf4), INK);
        });
        for id in &st.lines {
            if let Some(l) = self.d.scene_meta.lines.iter().find(|l| &l.id == id) {
                ui.label(egui::RichText::new(format!("{}　{}", l.name, l.sub)).size(12.0).color(MUTED));
            }
        }
        let (x, y) = self.d.geo.to_local(st.ll[0], st.ll[1]);
        kv(ui, &[("地面標高", format!("{:.1} m", self.d.hg.at(x, y))), ("通車", st.yr.to_string()), ("座標", format!("{:.4}°N {:.4}°E", st.ll[1], st.ll[0]))]);
        let color = self.d.scene_meta.lines.iter().find(|l| st.lines.iter().any(|s| s == &l.id && s != "HSR")).map(|l| l.color.clone());
        if ui.add(egui::Button::new(egui::RichText::new("內部剖面").color(Color32::WHITE)).fill(ACCENT)).clicked() {
            let k = if st.lines.len() > 2 { "hub" } else if st.kind == "u" || st.kind == "t" { "metroU" } else { "metroE" };
            self.interior = Some(InteriorView::new(k, &format!("{}站・內部剖面", st.n), color));
        }
    }

    fn transit_body(&mut self, ui: &mut egui::Ui) {
        eyebrow(ui, "顯示路線（點選切換開關）");
        self.line_chips(ui);
        ui.add_space(8.0);
        let lines = self.d.scene_meta.lines.clone();
        for l in &lines {
            let n: usize = l.stations.len();
            let on = self.focus_line.as_deref() == Some(&l.id);
            if list_item(ui, hex32(&l.color), &l.name, &l.sub, &format!("{n} 站")).clicked() {
                self.focus_line = if on { None } else { Some(l.id.clone()) };
                if !on {
                    let pts: Vec<[f32; 3]> = l.segs.iter().flat_map(|s| s.pts.clone()).collect();
                    if !pts.is_empty() {
                        let (mnx, mxx) = pts.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
                        let (mnz, mxz) = pts.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p[2]), a.1.max(p[2])));
                        self.cam.fly_to((mnx + mxx) / 2.0, -(mnz + mxz) / 2.0, Some((mxx - mnx).max(mxz - mnz) * 1.4 + 3000.0), None, None, 1.6);
                    }
                }
            }
            if on {
                ui.label(egui::RichText::new(l.stations.iter().map(|s| s[0].clone()).collect::<Vec<_>>().join(" — ")).size(11.5).color(MUTED));
                if !l.note.is_empty() { ui.label(egui::RichText::new(&l.note).size(11.0).color(MUTED)); }
            }
        }
        ui.add_space(6.0);
        ui.checkbox(&mut self.layers[3], "顯示人口＋就業密度熱區");
        eyebrow(ui, "立體層級");
        for (lv, t) in [("+2", "上層高架：跨越高架道路或高鐵的捷運、快速道路交叉匝道（約 17–20 m）"), ("+1", "高架：快速道路、高鐵、外圍捷運（約 9–12 m）"), ("0", "地面：一般道路、幹線與接駁公車"), ("-1", "地下：市心與高密度區的捷運（淺色＋中央虛線）"), ("-2", "深層地下：與其他地下線交叉的區段")] {
            ui.horizontal(|ui| { ui.label(egui::RichText::new(lv).monospace().strong()); ui.label(egui::RichText::new(t).size(11.5).color(MUTED)); });
        }
        ui.label(egui::RichText::new("規劃方針：首爾級密度（車站 800 m 內涵蓋約 93% 的人口與就業）、東京式直通（每線 14–28 km、穿越市心）、站距市心約 700 m／住宅區約 1 km／外圍約 1.4 km；公車分幹線（BRT，站距約 500 m）與接駁（站距約 300 m）兩層。").size(11.5).color(MUTED));
    }

    fn contour_body(&mut self, ui: &mut egui::Ui) {
        eyebrow(ui, "分層設色");
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 12.0), Sense::hover());
        let stops = [(0.0, (196, 214, 170)), (0.11, (230, 228, 180)), (0.22, (236, 214, 160)), (0.38, (222, 186, 130)), (0.56, (200, 150, 100)), (0.78, (170, 120, 90)), (1.0, (150, 110, 95))];
        for w in stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            let rr = Rect::from_min_max(Pos2::new(r.min.x + r.width() * a.0, r.min.y), Pos2::new(r.min.x + r.width() * b.0, r.max.y));
            ui.painter().rect_filled(rr, 0.0, Color32::from_rgb(a.1 .0, a.1 .1, a.1 .2));
        }
        ui.label(egui::RichText::new("0　　60　　140　　260　　420 m").size(10.5).color(MUTED));
        eyebrow(ui, "等高線（GPU 即時繪製）");
        kv(ui, &[("首曲線", "每 10 m".into()), ("計曲線", "每 50 m".into())]);
        ui.label(egui::RichText::new("君山實測 432 m（聖經 434.6 m）。將軍山系依聖經抬升至約 345 m。").size(11.5).color(MUTED));
    }

    fn cs2_body(&mut self, ui: &mut egui::Ui) {
        eyebrow(ui, "遊戲地圖框（地形上的青色網格）");
        ui.horizontal(|ui| {
            ui.label("可建造邊長");
            let mut km = self.cs2.2 / 1000.0;
            if ui.add(egui::DragValue::new(&mut km).range(5.0..=120.0).speed(0.1).suffix(" km")).changed() { self.cs2.2 = km * 1000.0; }
        });
        ui.horizontal(|ui| {
            for (n, v) in [("原版 14.336", 14336.0), ("2×", 28672.0), ("4×（57.344）", 57344.0)] { if ui.button(n).clicked() { self.cs2.2 = v; } }
        });
        ui.horizontal(|ui| { ui.label("旋轉"); ui.add(egui::Slider::new(&mut self.cs2.3, -45.0..=45.0).suffix("°")); });
        if ui.button("以目前畫面中心為框心").clicked() { let (lon, lat) = self.d.geo.to_ll(self.cam.target.x as f64, -self.cam.target.z as f64); self.cs2.0 = lon; self.cs2.1 = lat; }
        ui.label(egui::RichText::new(format!("地圖格數 {0} × {0}；拉近後可看到 8 m 分區格。", (21.0 * self.cs2.2 / 14336.0).round())).size(11.5).color(MUTED));
        ui.add_space(6.0);
        eyebrow(ui, "道路分級（台灣 × 日本 → CS2）");
        let roads = self.d.story["CS2_ROADS"].as_object().cloned().unwrap_or_default();
        egui::Grid::new("roads").striped(true).num_columns(3).show(ui, |ui| {
            for (k, r) in roads.iter() {
                ui.label(egui::RichText::new(s(&r["name"])).size(12.0));
                ui.label(egui::RichText::new(s(&r["cs2"])).size(11.0).color(MUTED));
                ui.label(egui::RichText::new(format!("{}u・{:.0} km", r["u"], self.d.meta.stats["roadKm"][k].as_f64().unwrap_or(0.0))).size(11.5).monospace());
                ui.end_row();
            }
        });
        ui.label(egui::RichText::new("CS2 格數慣例（含人行道）：2 車道 2 格、4 車道 3 格、6 車道 4 格。").size(11.5).color(MUTED));
        ui.add_space(6.0);
        eyebrow(ui, "輸出高度圖（16-bit PNG 4096²）");
        ui.horizontal(|ui| { ui.label("海平面"); ui.add(egui::Slider::new(&mut self.hm_sea, 0.0..=200.0).suffix(" m")); });
        if ui.add(egui::Button::new(egui::RichText::new("輸出可建造範圍高度圖…").color(Color32::WHITE)).fill(ACCENT)).clicked() {
            let name = format!("lijing_heightmap_{:.3}km.png", self.cs2.2 / 1000.0);
            if let Some(path) = rfd::FileDialog::new().set_file_name(&name).add_filter("PNG", &["png"]).save_file() {
                self.cs2_msg = match crate::cs2::export(&self.d, self.cs2, self.hm_sea, &path) { Ok(m) => m, Err(e) => format!("失敗：{e}") };
            }
        }
        if !self.cs2_msg.is_empty() { ui.label(egui::RichText::new(&self.cs2_msg).size(11.5).color(ACCENT)); }
        ui.label(egui::RichText::new("滑鼠停在建物上可看地塊 u 數與 CS2 分區。").size(11.5).color(MUTED));
    }

    fn start_tour(&mut self, i: usize) {
        let tour = self.d.story["TOUR"].as_array().cloned().unwrap_or_default();
        if i >= tour.len() { self.tour = None; return; }
        self.tour = Some(i);
        if self.mode != Mode::ThreeD { self.mode = Mode::ThreeD; }
        let t = &tour[i];
        let at = if let Some(id) = t["id"].as_str() { self.lms.iter().find(|l| l.id == id).map(|l| l.at) } else { None }.unwrap_or((t["at"][0].as_f64().unwrap_or(119.76), t["at"][1].as_f64().unwrap_or(25.53)));
        let z = t["z"].as_f64().unwrap_or(14.0) as f32;
        let dist = (78271.5 * 0.9026 / 2f32.powf(z) * self.cam.vh / 0.828).max(400.0);
        let (x, y) = self.d.geo.to_local(at.0, at.1);
        self.cam.fly_to(x as f32, y as f32, Some(dist), Some(t["b"].as_f64().unwrap_or(0.0) as f32), Some(90.0 - t["p"].as_f64().unwrap_or(50.0) as f32), 3.2);
        if let Some(id) = t["id"].as_str() { if let Some(k) = self.lms.iter().position(|l| l.id == id) { self.sel = Sel::Landmark(k); } }
    }
    fn tour_caption(&mut self, ctx: &egui::Context, i: usize) {
        let tour = self.d.story["TOUR"].as_array().cloned().unwrap_or_default();
        let Some(t) = tour.get(i) else { return };
        egui::Area::new(egui::Id::new("tour")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -210.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width(600.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(format!("{}／{}・{}", i + 1, tour.len(), s(&t["t"]))).size(17.0).strong());
                        ui.label(egui::RichText::new(s(&t["txt"])).size(13.5));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("結束").clicked() { self.tour = None; }
                        if ui.button("›").clicked() { self.start_tour(i + 1); }
                        if ui.button("‹").clicked() && i > 0 { self.start_tour(i - 1); }
                    });
                });
            });
        });
    }

    fn update_banner(&mut self, ctx: &egui::Context, upd: &Updater, show: &mut bool) {
        if !*show { return; }
        let st = upd.get();
        let msg = match &st {
            UState::Available { tag, notes, .. } => Some(format!("有新版本 {tag}（目前 v{VERSION}）\n{}", notes.lines().take(4).collect::<Vec<_>>().join("\n"))),
            UState::Downloading { got, total } => Some(format!("下載更新中… {:.1} / {:.1} MB", *got as f64 / 1e6, *total as f64 / 1e6)),
            UState::Ready => Some("更新已下載，重新啟動即可套用。".to_string()),
            UState::Failed(_) => None, // 查不到更新（離線等）時不打擾
            _ => None,
        };
        let Some(msg) = msg else { return };
        egui::Area::new(egui::Id::new("upd")).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 80.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width(380.0);
                ui.label(egui::RichText::new(msg).size(13.0));
                ui.horizontal(|ui| {
                    match st {
                        UState::Available { .. } => { if ui.add(egui::Button::new(egui::RichText::new("下載並更新").color(Color32::WHITE)).fill(ACCENT)).clicked() { upd.download(); } }
                        UState::Ready => { if ui.add(egui::Button::new(egui::RichText::new("立即重新啟動").color(Color32::WHITE)).fill(ACCENT)).clicked() { upd.restart(); } }
                        _ => {}
                    }
                    if ui.button("稍後").clicked() { *show = false; }
                });
            });
        });
    }
}

fn pip(xs: &[f32], ys: &[f32], x: f32, y: f32) -> bool {
    let mut c = false;
    let n = xs.len();
    let mut j = n - 1;
    for i in 0..n {
        if (ys[i] > y) != (ys[j] > y) && x < (xs[j] - xs[i]) * (y - ys[i]) / (ys[j] - ys[i]) + xs[i] { c = !c; }
        j = i;
    }
    c
}

fn district_name(story: &Value, id: &str) -> String {
    if id == "isl" { return "離島".into(); }
    story["DISTRICTS"].as_array().and_then(|a| a.iter().find(|d| d["id"].as_str() == Some(id))).map(|d| s(&d["name"])).unwrap_or("地標".into())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.upd_checked { self.upd_checked = true; self.upd.check(); }
        let mut next = None;
        match &mut self.load {
            Load::Loading(rx, prog) => {
                if let Ok(r) = rx.try_recv() {
                    next = Some(match r {
                        Ok(d) => match frame.wgpu_render_state() { Some(rs) => Load::Ready(Box::new(World::new(d, rs))), None => Load::Failed("無法初始化 GPU（wgpu）".into()) },
                        Err(e) => Load::Failed(e),
                    });
                }
                let (f, m) = prog.lock().unwrap().clone();
                loading_screen(ui, f, &m);
                ctx.request_repaint();
            }
            Load::Ready(w) => {
                let mut show = !self.upd_dismissed;
                w.ui(ui, frame, &self.upd, &mut show);
                if !show { self.upd_dismissed = true; }
            }
            Load::Failed(e) => { loading_screen(ui, 0.0, &format!("載入失敗：{e}")); }
        }
        if let Some(n) = next { self.load = n; }
    }
}

fn loading_screen(ui: &mut egui::Ui, f: f32, msg: &str) {
    let r = ui.ctx().content_rect();
    ui.painter().rect_filled(r, 0.0, Color32::from_rgb(0xee, 0xf2, 0xf3));
    let c = r.center();
    ui.painter().circle_stroke(c - Vec2::new(0.0, 60.0), 32.0, Stroke::new(2.0, INK));
    ui.painter().text(c - Vec2::new(0.0, 60.0), Align2::CENTER_CENTER, "環", FontId::proportional(28.0), INK);
    ui.painter().text(c, Align2::CENTER_CENTER, "麗京特別市", FontId::proportional(24.0), INK);
    let bar = Rect::from_center_size(c + Vec2::new(0.0, 36.0), Vec2::new(340.0, 3.0));
    ui.painter().rect_filled(bar, 2.0, LINE);
    ui.painter().rect_filled(Rect::from_min_size(bar.min, Vec2::new(340.0 * f, 3.0)), 2.0, ACCENT);
    ui.painter().text(c + Vec2::new(0.0, 60.0), Align2::CENTER_CENTER, msg, FontId::monospace(13.0), MUTED);
}
