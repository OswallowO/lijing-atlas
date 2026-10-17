//! 背景執行緒：把建物／道路二進位資料轉成網格；軌道路線帶。
use crate::data::Data;
use crate::geo::HeightGrid;
use std::sync::{mpsc, Arc, Mutex};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BVertex {
    pub pos: [f32; 3],
    pub nrm: [i8; 4],
    pub uv: [f32; 2],
    pub meta: [u8; 4],   // 類型、外牆包覆年（-1900）、變化、屋頂旗標
    pub days: [u16; 4],  // 開工日、竣工日、拆除日、0（2014-01-01 起算）
    pub base: f32,       // 建物底部高度（施工中依進度由此長高）
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RVertex {
    pub pos: [f32; 3],
    pub col: [u8; 4],
    pub meta: [u16; 4], // 等級、通車日、拆除日、0
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LVertex {
    pub pos: [f32; 3],
    pub perp: [f32; 2],
    pub col: [u8; 4],
    pub sdkg: [f32; 4],
}

pub struct Job {
    pub key: (u8, i32, i32),
}
pub struct MeshData {
    pub verts: Vec<u8>,
    pub indices: Vec<u32>,
}
/// 點選用：每棟建物的外框與屬性
#[derive(Clone)]
pub struct BInfo {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    pub z0: f32,
    pub z1: f32,
    pub t: u8,
    pub floors: u8,
    pub uw: u8,
    pub ud: u8,
    pub h: f32,
    pub s: u16,
    pub e: u16,
    pub dd: u16,
    pub cz: u8,
}
pub struct ChunkInfo {
    pub b: Vec<BInfo>,
}
pub struct Done {
    pub key: (u8, i32, i32),
    pub meshes: Vec<(u8, MeshData)>,
    pub info: Option<ChunkInfo>,
}

pub const SMALL_CUT: f32 = 28.0;

struct Shared {
    hg: Arc<HeightGrid>,
    bld: Arc<Vec<u8>>,
    roads: Arc<Vec<u8>>,
    bidx: std::collections::HashMap<(i32, i32), (usize, usize, usize)>,
    ridx: std::collections::HashMap<(i32, i32), (usize, usize, usize)>,
    chunk: f32,
    widths: Vec<f32>,
    colors: Vec<[u8; 3]>,
}

pub fn spawn(d: &Data) -> (mpsc::Sender<Job>, mpsc::Receiver<Done>) {
    let (tx, rx) = mpsc::channel::<Job>();
    let (dtx, drx) = mpsc::channel::<Done>();
    let mut widths = vec![];
    let mut colors = vec![];
    let roads_def = &d.story["CS2_ROADS"];
    for k in &d.meta.road_keys {
        let r = &roads_def[k];
        widths.push(r["m"].as_f64().unwrap_or(16.0) as f32);
        let c = r["color"].as_str().unwrap_or("#ffffff");
        colors.push(hex(c));
    }
    let sh = Arc::new(Shared {
        hg: Arc::new(HeightGrid { w: d.hg.w, h: d.hg.h, xmin: d.hg.xmin, ymax: d.hg.ymax, pxm: d.hg.pxm, data: d.hg.data.clone() }),
        bld: Arc::new(d.bld.clone()),
        roads: Arc::new(d.roads.clone()),
        bidx: d.meta.bld.iter().map(|c| ((c[0] as i32, c[1] as i32), (c[2] as usize, c[3] as usize, c[4] as usize))).collect(),
        ridx: d.meta.roads.iter().map(|c| ((c[0] as i32, c[1] as i32), (c[2] as usize, c[3] as usize, c[4] as usize))).collect(),
        chunk: d.meta.chunk as f32,
        widths,
        colors,
    });
    let rx = Arc::new(Mutex::new(rx));
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(1).clamp(1, 6);
    for _ in 0..n {
        let rx = rx.clone();
        let dtx = dtx.clone();
        let sh = sh.clone();
        std::thread::spawn(move || loop {
            let job = { let g = rx.lock().unwrap(); g.recv() };
            let Ok(job) = job else { break };
            let (set, i, j) = job.key;
            let ox = (i as f32 + 0.5) * sh.chunk;
            let oy = (j as f32 + 0.5) * sh.chunk;
            let done = if set == 2 {
                let m = sh.ridx.get(&(i, j)).map(|&(off, len, n)| build_roads(&sh, &sh.roads[off..off + len], n, ox, oy));
                Done { key: job.key, meshes: m.into_iter().map(|m| (0u8, m)).collect(), info: None }
            } else {
                match sh.bidx.get(&(i, j)) {
                    Some(&(off, len, n)) => {
                        let (a, b, info) = build_buildings(&sh.hg, &sh.bld[off..off + len], n, ox, oy);
                        Done { key: job.key, meshes: vec![(0, a), (1, b)], info: Some(info) }
                    }
                    None => Done { key: job.key, meshes: vec![], info: None },
                }
            };
            if dtx.send(done).is_err() { break; }
        });
    }
    (tx, drx)
}

pub fn hex(c: &str) -> [u8; 3] {
    let c = c.trim_start_matches('#');
    let v = u32::from_str_radix(c, 16).unwrap_or(0xffffff);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

fn earcut(px: &[f32], py: &[f32]) -> Vec<u32> {
    let n = px.len();
    let mut idx = vec![];
    let mut area = 0.0;
    let mut j = n - 1;
    for i in 0..n {
        area += (px[j] - px[i]) * (py[j] + py[i]);
        j = i;
    }
    let mut v: Vec<usize> = (0..n).map(|i| if area > 0.0 { i } else { n - 1 - i }).collect();
    let cross = |a: usize, b: usize, c: usize| (px[b] - px[a]) * (py[c] - py[a]) - (py[b] - py[a]) * (px[c] - px[a]);
    let mut guard = 0;
    while v.len() > 3 && guard < 5000 {
        guard += 1;
        let mut cut = false;
        let m = v.len();
        for i in 0..m {
            let (a, b, c) = (v[(i + m - 1) % m], v[i], v[(i + 1) % m]);
            if cross(a, b, c) <= 0.0 { continue; }
            let mut ok = true;
            for &p in &v {
                if p == a || p == b || p == c { continue; }
                if cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0 { ok = false; break; }
            }
            if !ok { continue; }
            idx.extend_from_slice(&[a as u32, b as u32, c as u32]);
            v.remove(i);
            cut = true;
            break;
        }
        if !cut {
            for i in 1..v.len() - 1 { idx.extend_from_slice(&[v[0] as u32, v[i] as u32, v[i + 1] as u32]); }
            return idx;
        }
    }
    if v.len() == 3 { idx.extend_from_slice(&[v[0] as u32, v[1] as u32, v[2] as u32]); }
    idx
}

/// 網格輸出輔助：牆、頂面、任意四邊形／三角形（法線朝外／朝上）
struct Emit<'a> { v: &'a mut Vec<BVertex>, i: &'a mut Vec<u32>, meta: [u8; 4], days: [u16; 4], z0: f32, g: f32, cx: f32, cy: f32 }
fn nrm8(n: [f32; 3]) -> [i8; 4] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
    [(n[0] / l * 127.0) as i8, (n[1] / l * 127.0) as i8, (n[2] / l * 127.0) as i8, 0]
}
impl<'a> Emit<'a> {
    fn walls(&mut self, xs: &[f32], ys: &[f32], za: f32, zb: f32, flag: u8) {
        let nv = xs.len();
        let mut perim = 0.0;
        for k in 0..nv {
            let k2 = (k + 1) % nv;
            let (ax, ay, bx, by) = (xs[k], ys[k], xs[k2], ys[k2]);
            let l = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            if l < 0.05 { continue; }
            let nn = [((by - ay) / l * 127.0) as i8, 0, ((bx - ax) / l * 127.0) as i8, 0];
            let base = self.v.len() as u32;
            let m = [self.meta[0], self.meta[1], self.meta[2], flag];
            for (x, yy, z, u) in [(ax, ay, za, perim), (bx, by, za, perim + l), (bx, by, zb, perim + l), (ax, ay, zb, perim)] {
                self.v.push(BVertex { pos: [x, z, -yy], nrm: nn, uv: [u, z - self.g], meta: m, days: self.days, base: self.z0 });
            }
            self.i.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            perim += l;
        }
    }
    fn cap(&mut self, xs: &[f32], ys: &[f32], z: f32, flag: u8) {
        let tri = earcut(xs, ys);
        let base = self.v.len() as u32;
        for k in 0..xs.len() {
            self.v.push(BVertex { pos: [xs[k], z, -ys[k]], nrm: [0, 127, 0, 0], uv: [0.0, -1.0], meta: [self.meta[0], self.meta[1], self.meta[2], flag], days: self.days, base: self.z0 });
        }
        for c in tri.chunks(3) { self.i.extend_from_slice(&[base + c[0], base + c[2], base + c[1]]); }
    }
    /// 多邊形面（點為 (x, y平面, z高)）；法線由叉積決定，屋面朝上、立面朝外
    fn face(&mut self, p: &[[f32; 3]], flag: u8, wall_uv: bool) {
        let w = |q: [f32; 3]| [q[0], q[2], -q[1]];
        let (a, b, c) = (w(p[0]), w(p[1]), w(p[p.len() - 1]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let mx = p.iter().map(|q| q[0]).sum::<f32>() / p.len() as f32 - self.cx;
        let my = p.iter().map(|q| q[1]).sum::<f32>() / p.len() as f32 - self.cy;
        let out = n[0] * mx + n[2] * (-my);
        if n[1] < -1e-4 || (n[1].abs() < 1e-3 && out < 0.0) { n = [-n[0], -n[1], -n[2]]; }
        let nn = nrm8(n);
        let base = self.v.len() as u32;
        let mut perim = 0.0;
        for (k, q) in p.iter().enumerate() {
            if k > 0 { perim += ((q[0] - p[k - 1][0]).powi(2) + (q[1] - p[k - 1][1]).powi(2)).sqrt(); }
            let uv = if wall_uv { [perim, q[2] - self.g] } else { [0.0, -1.0] };
            self.v.push(BVertex { pos: w(*q), nrm: nn, uv, meta: [self.meta[0], self.meta[1], self.meta[2], flag], days: self.days, base: self.z0 });
        }
        for k in 1..p.len() as u32 - 1 { self.i.extend_from_slice(&[base, base + k, base + k + 1]); }
    }
    fn bx(&mut self, cx: f32, cy: f32, ax: (f32, f32), hw: f32, hd: f32, za: f32, zb: f32, flag: u8) {
        let bxv = (-ax.1, ax.0);
        let q: Vec<(f32, f32)> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|&(a, b)| (cx + ax.0 * hw * a + bxv.0 * hd * b, cy + ax.1 * hw * a + bxv.1 * hd * b)).collect();
        let xs: Vec<f32> = q.iter().map(|p| p.0).collect(); let ys: Vec<f32> = q.iter().map(|p| p.1).collect();
        self.walls(&xs, &ys, za, zb, flag);
        self.cap(&xs, &ys, zb, if flag == 4 || flag == 5 { flag } else { 1 });
    }
}

/// 最小外接矩形：中心、長軸單位向量、半長、半寬、填滿率
fn obb(xs: &[f32], ys: &[f32], area: f32) -> (f32, f32, (f32, f32), f32, f32, f32) {
    let n = xs.len();
    let mut best = (f32::MAX, 0.0f32, 0.0f32, (1.0f32, 0.0f32), 0.0f32, 0.0f32);
    for k in 0..n {
        let k2 = (k + 1) % n;
        let (dx, dy) = (xs[k2] - xs[k], ys[k2] - ys[k]);
        let l = (dx * dx + dy * dy).sqrt();
        if l < 0.5 { continue; }
        let (ux, uy) = (dx / l, dy / l);
        let (mut a0, mut a1, mut b0, mut b1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for j in 0..n {
            let a = xs[j] * ux + ys[j] * uy; let b = -xs[j] * uy + ys[j] * ux;
            a0 = a0.min(a); a1 = a1.max(a); b0 = b0.min(b); b1 = b1.max(b);
        }
        let ar = (a1 - a0) * (b1 - b0);
        if ar < best.0 {
            let (ca, cb) = ((a0 + a1) / 2.0, (b0 + b1) / 2.0);
            let (cx, cy) = (ca * ux - cb * uy, ca * uy + cb * ux);
            let (hw, hd) = ((a1 - a0) / 2.0, (b1 - b0) / 2.0);
            best = if hw >= hd { (ar, cx, cy, (ux, uy), hw, hd) } else { (ar, cx, cy, (-uy, ux), hd, hw) };
        }
    }
    (best.1, best.2, best.3, best.4, best.5, (area / best.0.max(1e-3)).min(1.0))
}
/// 沿外接矩形軸向內縮 d 公尺（任意形狀）
fn inset(xs: &[f32], ys: &[f32], o: &(f32, f32, (f32, f32), f32, f32, f32), da: f32, db: f32) -> (Vec<f32>, Vec<f32>) {
    let (cx, cy, (ux, uy), hw, hd, _) = *o;
    let (sa, sb) = (((hw - da) / hw).max(0.2), ((hd - db) / hd).max(0.2));
    let mut ox = vec![]; let mut oy = vec![];
    for k in 0..xs.len() {
        let (x, y) = (xs[k] - cx, ys[k] - cy);
        let a = (x * ux + y * uy) * sa; let b = (-x * uy + y * ux) * sb;
        ox.push(cx + a * ux - b * uy); oy.push(cy + a * uy + b * ux);
    }
    (ox, oy)
}
fn hash2(x: f32, y: f32) -> u32 {
    let mut h = (x.floor() as i32 as u32).wrapping_mul(0x9E3779B1) ^ (y.floor() as i32 as u32).wrapping_mul(0x85EBCA77);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B3C6D); h ^= h >> 12; h
}

fn build_buildings(hg: &HeightGrid, b: &[u8], n: usize, ox: f32, oy: f32) -> (MeshData, MeshData, ChunkInfo) {
    let mut parts: [(Vec<BVertex>, Vec<u32>); 2] = [(vec![], vec![]), (vec![], vec![])];
    let mut info = Vec::with_capacity(n);
    let mut p = 0usize;
    let u16_ = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let i16_ = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    for _ in 0..n {
        if p + 18 > b.len() { break; }
        let nv = b[p] as usize;
        let t = b[p + 1];
        let cy = b[p + 2];
        let cz = b[p + 3];   // CS2 分區碼
        let h = u16_(p + 4) as f32 / 10.0;
        let b0 = u16_(p + 6) as f32 / 10.0;
        let (uw, ud, fl) = (b[p + 8], b[p + 9], b[p + 10]);
        let (sd, ed, dd) = (u16_(p + 12), u16_(p + 14), u16_(p + 16));
        p += 18;
        let mut xs = Vec::with_capacity(nv);
        let mut ys = Vec::with_capacity(nv);
        let (mut cx, mut cyy) = (0.0f32, 0.0f32);
        for _ in 0..nv {
            let x = i16_(p) as f32 / 10.0 + ox;
            let yy = i16_(p + 2) as f32 / 10.0 + oy;
            p += 4;
            xs.push(x); ys.push(yy);
            cx += x; cyy += yy;
        }
        if nv < 3 { continue; }
        cx /= nv as f32;
        cyy /= nv as f32;
        let g = hg.at(cx as f64, cyy as f64).max(0.3);
        let z0 = g + b0 - if b0 > 0.0 { 0.0 } else { 3.0 };
        let z1 = g + h.max(b0 + 2.5);
        let mut ar = 0.0;
        let mut j = nv - 1;
        for k in 0..nv { ar += xs[j] * ys[k] - xs[k] * ys[j]; j = k; }
        if ar < 0.0 { xs.reverse(); ys.reverse(); }
        let area = ar.abs() / 2.0;
        // 樣式庫：依位置雜湊選樣式（0..7）與色票（0..15）
        let hs = hash2(cx * 0.37, cyy * 0.37);
        let style = (hs % 8) as u8;
        let pal = ((hs >> 8) % 16) as u8;
        let meta = [t, cy, style | (pal << 4), 0];
        let days = [sd, ed, dd, cz as u16];
        let part = &mut parts[if h >= SMALL_CUT { 0 } else { 1 }];
        let o = obb(&xs, &ys, area);
        let (ocx, ocy, ax, hw, hd, fill) = o;
        let mut e = Emit { v: &mut part.0, i: &mut part.1, meta, days, z0, g, cx, cy: cyy };
        let floor_h = if t == 0 || t == 8 { 4.0 } else { 3.2 };
        let rect = fill > 0.82 && nv <= 8;
        // 頂部形式：0 平頂（女兒牆）、1 雙斜、2 四坡、3 單斜、4 鋸齒、5 筒拱、9 無頂部處理
        let mut top_xs = xs.clone(); let mut top_ys = ys.clone();
        let mut roof = 0u8;
        let hgt = z1 - g;
        match t {
            // 透天、戰前民居：斜屋頂為主
            3 | 10 | 11 => {
                if rect && hgt < 16.0 && hd > 2.5 { roof = [1, 2, 1, 3, 2, 0, 1, 2][style as usize]; }
                e.walls(&xs, &ys, z0, z1, 0);
            }
            // 中高層住宅
            1 | 2 => {
                let k = style % 5;
                if k == 1 && hgt > 15.0 && hd > 6.0 {
                    // 頂樓退縮
                    let zt = z1 - floor_h * if t == 1 { 3.0 } else { 1.0 };
                    e.walls(&xs, &ys, z0, zt, 0); e.cap(&xs, &ys, zt, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, 2.4, 2.4);
                    e.walls(&ix, &iy, zt, z1, 0); top_xs = ix; top_ys = iy;
                } else if k == 2 && hgt > 12.0 && hd > 7.0 {
                    // 店鋪裙樓＋住宅主體
                    let zp = (g + floor_h * if t == 1 { 3.0 } else { 1.0 } + 0.6).min(z1 - 3.0);
                    e.walls(&xs, &ys, z0, zp, 0); e.cap(&xs, &ys, zp, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, 2.0, 2.0);
                    e.walls(&ix, &iy, zp, z1, 0); top_xs = ix; top_ys = iy;
                } else if k == 3 && t == 1 && hgt > 30.0 && hd > 8.0 {
                    // 兩段退縮
                    let za = g + hgt * 0.62; let zb = g + hgt * 0.84;
                    e.walls(&xs, &ys, z0, za, 0); e.cap(&xs, &ys, za, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, 2.5, 1.5);
                    e.walls(&ix, &iy, za, zb, 0); e.cap(&ix, &iy, zb, 1);
                    let (jx, jy) = inset(&xs, &ys, &o, 5.0, 3.0);
                    e.walls(&jx, &jy, zb, z1, 0); top_xs = jx; top_ys = jy;
                } else {
                    e.walls(&xs, &ys, z0, z1, 0);
                    if k == 4 && rect && t == 2 && hgt < 26.0 { roof = 2; }
                }
            }
            // 辦公、超高層
            0 | 8 => {
                let k = style % 5;
                if k == 1 && hgt > 40.0 && hd > 10.0 {
                    // 婚禮蛋糕式退縮
                    let za = g + hgt * 0.55; let zb = g + hgt * 0.8;
                    e.walls(&xs, &ys, z0, za, 0); e.cap(&xs, &ys, za, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, hw * 0.12, hd * 0.12);
                    e.walls(&ix, &iy, za, zb, 0); e.cap(&ix, &iy, zb, 1);
                    let (jx, jy) = inset(&xs, &ys, &o, hw * 0.25, hd * 0.25);
                    e.walls(&jx, &jy, zb, z1, 0); top_xs = jx; top_ys = jy;
                } else if k == 2 && hgt > 30.0 && hd > 12.0 {
                    // 裙樓＋塔樓
                    let zp = g + floor_h * 5.0;
                    e.walls(&xs, &ys, z0, zp, 0); e.cap(&xs, &ys, zp, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, hw * 0.28, hd * 0.25);
                    e.walls(&ix, &iy, zp, z1, 0); top_xs = ix; top_ys = iy;
                } else if k == 3 && hgt > 50.0 {
                    // 發光冠頂＋尖塔
                    let zc = z1 - floor_h * 2.0;
                    e.walls(&xs, &ys, z0, zc, 0); e.cap(&xs, &ys, zc, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, 1.2, 1.2);
                    e.walls(&ix, &iy, zc, z1, 4); top_xs = ix; top_ys = iy;
                    if hgt > 120.0 { e.bx(ocx, ocy, ax, 0.8, 0.8, z1, z1 + hgt * 0.12, 2); }
                } else if k == 4 && rect && hgt > 30.0 {
                    roof = 3; e.walls(&xs, &ys, z0, z1, 0);   // 斜切屋頂
                } else {
                    e.walls(&xs, &ys, z0, z1, 0);
                }
            }
            // 商住混合：店面裙樓＋退縮住宅
            4 => {
                let zp = (g + 3.2 * 2.0 + 1.0).min(z1 - 2.0);
                e.walls(&xs, &ys, z0, zp, 0); e.cap(&xs, &ys, zp, 1);
                if hd > 6.0 && hgt > 10.0 {
                    let d = 1.5 + (style % 3) as f32;
                    let (ix, iy) = inset(&xs, &ys, &o, d, d);
                    e.walls(&ix, &iy, zp, z1, 0); top_xs = ix; top_ys = iy;
                } else { e.walls(&xs, &ys, zp, z1, 0); }
            }
            // 工業：鋸齒、筒拱、低斜
            6 => {
                if rect && hd > 6.0 { roof = [4, 5, 1, 0, 4, 5, 1, 4][style as usize]; }
                e.walls(&xs, &ys, z0, z1, 0);
            }
            // 公共、學校：四坡、退縮、平頂
            5 | 7 => {
                let k = style % 4;
                if k == 1 && rect && hgt < 22.0 { roof = 2; e.walls(&xs, &ys, z0, z1, 0); }
                else if k == 2 && hgt > 12.0 && hd > 6.0 {
                    let zt = z1 - 3.6;
                    e.walls(&xs, &ys, z0, zt, 0); e.cap(&xs, &ys, zt, 1);
                    let (ix, iy) = inset(&xs, &ys, &o, hw * 0.2, 2.0);
                    e.walls(&ix, &iy, zt, z1, 0); top_xs = ix; top_ys = iy;
                } else { e.walls(&xs, &ys, z0, z1, 0); }
            }
            _ => { e.walls(&xs, &ys, z0, z1, 0); }
        }
        e.cap(&top_xs, &top_ys, z1, 1);
        // 斜屋頂（以外接矩形為準，略出簷）
        let (ux, uy) = ax; let (vx, vy) = (-uy, ux);
        let c4 = |a: f32, bb: f32, z: f32| [ocx + ux * a + vx * bb, ocy + uy * a + vy * bb, z];
        let (ew, eh) = (hw + 0.4, hd + 0.4);
        let rise = (hd * if t == 6 { 0.22 } else { 0.55 }).min(6.0);
        match roof {
            1 => {
                e.face(&[c4(-ew, -eh, z1), c4(ew, -eh, z1), c4(ew, 0.0, z1 + rise), c4(-ew, 0.0, z1 + rise)], 3, false);
                e.face(&[c4(-ew, eh, z1), c4(ew, eh, z1), c4(ew, 0.0, z1 + rise), c4(-ew, 0.0, z1 + rise)], 3, false);
                e.face(&[c4(-hw, -hd, z1), c4(-hw, hd, z1), c4(-hw, 0.0, z1 + rise)], 0, true);
                e.face(&[c4(hw, -hd, z1), c4(hw, hd, z1), c4(hw, 0.0, z1 + rise)], 0, true);
            }
            2 => {
                let r = (ew - eh).max(0.0);
                e.face(&[c4(-ew, -eh, z1), c4(ew, -eh, z1), c4(r, 0.0, z1 + rise), c4(-r, 0.0, z1 + rise)], 3, false);
                e.face(&[c4(-ew, eh, z1), c4(ew, eh, z1), c4(r, 0.0, z1 + rise), c4(-r, 0.0, z1 + rise)], 3, false);
                e.face(&[c4(-ew, -eh, z1), c4(-ew, eh, z1), c4(-r, 0.0, z1 + rise)], 3, false);
                e.face(&[c4(ew, -eh, z1), c4(ew, eh, z1), c4(r, 0.0, z1 + rise)], 3, false);
            }
            3 => {
                let rs = rise * if t == 0 || t == 8 { 2.2 } else { 0.7 };
                let fl3 = if t == 0 || t == 8 { 4 } else { 3 };
                e.face(&[c4(-ew, -eh, z1), c4(ew, -eh, z1), c4(ew, eh, z1 + rs), c4(-ew, eh, z1 + rs)], fl3, false);
                e.face(&[c4(-hw, hd, z1), c4(hw, hd, z1), c4(hw, hd, z1 + rs), c4(-hw, hd, z1 + rs)], 0, true);
                e.face(&[c4(-hw, -hd, z1), c4(-hw, hd, z1), c4(-hw, hd, z1 + rs)], 0, true);
                e.face(&[c4(hw, -hd, z1), c4(hw, hd, z1), c4(hw, hd, z1 + rs)], 0, true);
            }
            4 => {
                // 鋸齒：沿長軸每 9 m 一齒，北向採光玻璃
                let teeth = ((2.0 * hw) / 9.0).floor().max(2.0) as i32;
                let step = 2.0 * hw / teeth as f32; let rs = 3.2;
                for k in 0..teeth {
                    let a0 = -hw + step * k as f32; let a1 = a0 + step;
                    e.face(&[c4(a0, -hd, z1), c4(a0, hd, z1), c4(a1, hd, z1 + rs), c4(a1, -hd, z1 + rs)], 3, false);
                    e.face(&[c4(a1, -hd, z1), c4(a1, hd, z1), c4(a1, hd, z1 + rs), c4(a1, -hd, z1 + rs)], 4, false);
                    e.face(&[c4(a0, -hd, z1), c4(a1, -hd, z1), c4(a1, -hd, z1 + rs)], 0, true);
                    e.face(&[c4(a0, hd, z1), c4(a1, hd, z1), c4(a1, hd, z1 + rs)], 0, true);
                }
            }
            5 => {
                // 筒拱：橫斷面 8 段圓弧
                let rs = (hd * 0.45).min(7.0);
                let seg = 8;
                let pt = |k: i32| { let th = std::f32::consts::PI * k as f32 / seg as f32; (-hd * th.cos(), rs * th.sin()) };
                for k in 0..seg {
                    let (b0_, z0_) = pt(k); let (b1_, z1_) = pt(k + 1);
                    e.face(&[c4(-hw, b0_, z1 + z0_), c4(hw, b0_, z1 + z0_), c4(hw, b1_, z1 + z1_), c4(-hw, b1_, z1 + z1_)], 3, false);
                }
                for sgn in [-1.0f32, 1.0] {
                    let pts: Vec<[f32; 3]> = (0..=seg).map(|k| { let (bb, zz) = pt(k); c4(sgn * hw, bb, z1 + zz) }).collect();
                    e.face(&pts, 0, true);
                }
            }
            _ => {}
        }
        // 屋頂細節：女兒牆（12 m 以上平頂）、屋頂機房（15 m 以上）
        if roof == 0 && h >= 12.0 && t != 9 {
            let ph = if h > 60.0 { 1.6 } else { 1.0 };
            let (ix, iy) = inset(&top_xs, &top_ys, &obb(&top_xs, &top_ys, area), 0.35, 0.35);
            e.walls(&top_xs, &top_ys, z1, z1 + ph, 2);
            e.walls(&ix, &iy, z1, z1 + ph, 2);
            if h >= 15.0 && t != 3 && t != 10 {
                let sz = (hd * 0.6).clamp(2.5, 12.0);
                let bh = if t == 8 || t == 0 { 5.0 } else { 3.0 };
                let off = ((style as f32) - 3.5) / 3.5 * (hw * 0.4);
                e.bx(ocx + ux * off, ocy + uy * off, ax, sz / 2.0 * 1.3, sz / 2.0, z1, z1 + bh, 2);
                if style % 3 == 0 && hd > 10.0 { e.bx(ocx - ux * off * 0.8 + vx * hd * 0.4, ocy - uy * off * 0.8 + vy * hd * 0.4, ax, 1.6, 1.6, z1, z1 + 3.0, 2); }   // 水塔
            }
        }
        // Cyberpunk 細節（仍在 CS2 可建造範圍）：商業與商住的直立霓虹招牌、屋頂看板；住宅屋頂水塔與冷氣
        let hs2 = hash2(cx * 1.7 + 3.0, cyy * 1.3 - 5.0);
        if matches!(cz, 4 | 7 | 8) || (cz == 6 && hs2 % 3 == 0) {
            let nsign = 1 + (hs2 % 2) as i32;
            for k in 0..nsign {
                let along = if k == 0 { -hw * 0.6 } else { hw * 0.55 };
                let side = if (hs2 >> 3) % 2 == 0 { 1.0 } else { -1.0 };
                let (sx, sy) = (ocx + ux * along + vx * (hd + 0.8) * side, ocy + uy * along + vy * (hd + 0.8) * side);
                let sh = (4.0 + (hs2 >> (5 + k)) as f32 % 9.0).min(hgt - 3.0);
                if sh > 2.5 { e.bx(sx, sy, (vx, vy), 0.9, 0.35, g + 3.5, g + 3.5 + sh, 5); }
            }
        }
        if roof == 0 && (cz == 8 || (matches!(cz, 4 | 6 | 10) && hs2 % 4 == 1)) && hgt > 12.0 {
            let bw = (hw * 0.9).min(14.0); let bh = 5.0 + (hs2 % 5) as f32;
            let (bxc, byc) = (ocx + vx * hd * 0.7, ocy + vy * hd * 0.7);
            e.bx(bxc, byc, ax, bw, 0.3, z1 + 2.0, z1 + 2.0 + bh, 5);
            e.bx(bxc - ux * bw * 0.7, byc - uy * bw * 0.7, ax, 0.2, 0.2, z1, z1 + 2.0, 2);
            e.bx(bxc + ux * bw * 0.7, byc + uy * bw * 0.7, ax, 0.2, 0.2, z1, z1 + 2.0, 2);
        }
        if roof == 0 && matches!(cz, 3 | 4 | 5 | 6 | 0) && hgt > 9.0 && hd > 4.0 {
            let n = 2 + (hs2 % 4) as i32;
            for k in 0..n {
                let fa = ((hs2 >> (k * 3)) % 7) as f32 / 7.0 - 0.45; let fb = ((hs2 >> (k * 3 + 9)) % 5) as f32 / 5.0 - 0.4;
                let (px_, py_) = (ocx + ux * hw * fa * 1.4 + vx * hd * fb * 1.4, ocy + uy * hw * fa * 1.4 + vy * hd * fb * 1.4);
                if k == 0 { e.bx(px_, py_, ax, 1.3, 1.3, z1, z1 + 3.2, 2); } else { e.bx(px_, py_, ax, 0.6, 0.5, z1, z1 + 0.9, 2); }
            }
        }
        info.push(BInfo { xs, ys, z0, z1, t, floors: fl, uw, ud, h, s: sd, e: ed, dd, cz });
    }
    let [a, bb] = parts;
    (
        MeshData { verts: bytemuck::cast_slice(&a.0).to_vec(), indices: a.1 },
        MeshData { verts: bytemuck::cast_slice(&bb.0).to_vec(), indices: bb.1 },
        ChunkInfo { b: info },
    )
}

fn build_roads(sh: &Shared, b: &[u8], n: usize, _ox: f32, _oy: f32) -> MeshData {
    let mut v: Vec<RVertex> = vec![];
    let mut idx: Vec<u32> = vec![];
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    for s in 0..n {
        let o = s * 28;
        if o + 28 > b.len() { break; }
        let (k, br) = (b[o] as usize, b[o + 1]);
        let day = u16::from_le_bytes([b[o + 2], b[o + 3]]);
        let dd = u16::from_le_bytes([b[o + 4], b[o + 5]]);
        // 立體層級：端點離地高度（分米）；+1 高架約 9 m、+2 上層高架約 17 m，匝道沿線內插
        let za = i16::from_le_bytes([b[o + 6], b[o + 7]]) as f32 / 10.0;
        let zb = i16::from_le_bytes([b[o + 8], b[o + 9]]) as f32 / 10.0;
        let (ax, ay, bx, by) = (f(o + 12), f(o + 16), f(o + 20), f(o + 24));
        let l = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        if l < 0.5 { continue; }
        let w = sh.widths.get(k).copied().unwrap_or(12.0) / 2.0;
        let (ux, uy) = ((bx - ax) / l, (by - ay) / l);
        let (nx, ny) = (-uy * w, ux * w);
        let (ex, ey) = (ux * w * 0.5, uy * w * 0.5);
        let steps = ((l / 25.0).ceil() as usize).max(1);
        let c = sh.colors.get(k).copied().unwrap_or([250, 250, 250]);
        let lift = 0.15 * (8.0 - k as f32);
        for q in 0..steps {
            let (t0, t1) = (q as f32 / steps as f32, (q + 1) as f32 / steps as f32);
            let (mut x0, mut y0, mut x1, mut y1) = (ax + (bx - ax) * t0, ay + (by - ay) * t0, ax + (bx - ax) * t1, ay + (by - ay) * t1);
            if q == 0 { x0 -= ex; y0 -= ey; }
            if q == steps - 1 { x1 += ex; y1 += ey; }
            let hh = |x: f32, y: f32| {
                let h = sh.hg.at(x as f64, y as f64);
                if br > 0 || h < 0.3 { h.max(0.0) + if br > 0 { 9.0 } else { 0.6 } } else { h + 0.6 }
            };
            let (z0, z1) = (za + (zb - za) * t0, za + (zb - za) * t1);
            let (h0, h1) = (hh(x0, y0) + lift + z0, hh(x1, y1) + lift + z1);
            let base = v.len() as u32;
            let meta = [k as u16, day, dd, 0];
            let col = [c[0], c[1], c[2], 255];
            for (x, yy, h) in [(x0 + nx, y0 + ny, h0), (x0 - nx, y0 - ny, h0), (x1 - nx, y1 - ny, h1), (x1 + nx, y1 + ny, h1)] {
                v.push(RVertex { pos: [x, h, -yy], col, meta });
            }
            idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            // 高架：橋面側板與橋墩（每 50 m）
            if z0 > 3.0 || z1 > 3.0 {
                let side = [(c[0] as f32 * 0.7) as u8, (c[1] as f32 * 0.7) as u8, (c[2] as f32 * 0.7) as u8, 255];
                for sg in [1.0f32, -1.0] {
                    let b2 = v.len() as u32;
                    for (x, yy, h) in [(x0 + nx * sg, y0 + ny * sg, h0), (x1 + nx * sg, y1 + ny * sg, h1), (x1 + nx * sg, y1 + ny * sg, h1 - 1.6), (x0 + nx * sg, y0 + ny * sg, h0 - 1.6)] {
                        v.push(RVertex { pos: [x, h, -yy], col: side, meta });
                    }
                    idx.extend_from_slice(&[b2, b2 + 1, b2 + 2, b2, b2 + 2, b2 + 3]);
                }
                let pier_d = (ax - x0).hypot(ay - y0);
                if (pier_d / 50.0).fract() < (25.0 / 50.0) && q % 2 == 0 && z0 > 3.0 {
                    let g0 = sh.hg.at(x0 as f64, y0 as f64).max(-2.0);
                    let pw = 1.4f32;
                    for (dx, dy) in [(pw, 0.0f32), (0.0, pw)] {
                        let b2 = v.len() as u32;
                        for (x, yy, h) in [(x0 - dx, y0 - dy, g0), (x0 + dx, y0 + dy, g0), (x0 + dx, y0 + dy, h0 - 1.6), (x0 - dx, y0 - dy, h0 - 1.6)] {
                            v.push(RVertex { pos: [x, h, -yy], col: side, meta });
                        }
                        idx.extend_from_slice(&[b2, b2 + 1, b2 + 2, b2, b2 + 2, b2 + 3]);
                    }
                }
            }
        }
    }
    MeshData { verts: bytemuck::cast_slice(&v).to_vec(), indices: idx }
}

/// 軌道路線帶：中心線＋垂直向量，寬度在頂點著色器依每像素公尺換算。
pub fn line_ribbons(d: &Data) -> (Vec<LVertex>, Vec<u32>) {
    let mut v = vec![];
    let mut idx = vec![];
    for (li, l) in d.scene_meta.lines.iter().enumerate() {
        let c = hex(&l.color);
        let group = d.scene_meta.groups.iter().position(|g| g == &format!("line:{}", l.id)).unwrap_or(0) as f32;
        let _ = li;
        for s in &l.segs {
            let pts = &s.pts;
            let mut dist = 0.0f32;
            for i in 0..pts.len() {
                let a = pts[i.saturating_sub(1)];
                let b = pts[(i + 1).min(pts.len() - 1)];
                let (mut dx, mut dz) = (b[0] - a[0], b[2] - a[2]);
                let ln = (dx * dx + dz * dz).sqrt().max(1e-3);
                dx /= ln;
                dz /= ln;
                if i > 0 {
                    dist += ((pts[i][0] - pts[i - 1][0]).powi(2) + (pts[i][2] - pts[i - 1][2]).powi(2)).sqrt();
                }
                let kind = *s.kinds.get(i).unwrap_or(&0) as f32;
                for side in [-1.0f32, 1.0] {
                    v.push(LVertex { pos: pts[i], perp: [-dz, dx], col: [c[0], c[1], c[2], (l.y - 1900).clamp(0, 255) as u8], sdkg: [side, dist, kind, group] });
                }
                if i > 0 {
                    let k = (v.len() - 4) as u32;
                    idx.extend_from_slice(&[k, k + 1, k + 2, k + 1, k + 3, k + 2]);
                }
            }
        }
    }
    (v, idx)
}
