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
    pub meta: [u16; 2], // 等級、通車日
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
        let zv = b[p + 3];
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
            xs.push(x);
            ys.push(yy);
            cx += x;
            cyy += yy;
        }
        if nv < 3 { continue; }
        cx /= nv as f32;
        cyy /= nv as f32;
        let g = hg.at(cx as f64, cyy as f64).max(0.3);
        let z0 = g + b0 - if b0 > 0.0 { 0.0 } else { 3.0 };
        let z1 = g + h.max(b0 + 2.5);
        let meta = [t, cy, (zv >> 4) & 15, 0];
        let days = [sd, ed, dd, 0];
        let mut ar = 0.0;
        let mut j = nv - 1;
        for k in 0..nv {
            ar += xs[j] * ys[k] - xs[k] * ys[j];
            j = k;
        }
        if ar < 0.0 {
            xs.reverse();
            ys.reverse();
        }
        let part = &mut parts[if h >= SMALL_CUT { 0 } else { 1 }];
        let mut perim = 0.0;
        for k in 0..nv {
            let k2 = (k + 1) % nv;
            let (ax, ay, bx, by) = (xs[k], ys[k], xs[k2], ys[k2]);
            let l = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            if l < 0.05 { continue; }
            let nx = (by - ay) / l;
            let nz = (bx - ax) / l;
            let base = part.0.len() as u32;
            let vb = z0 - g;
            let nn = [(nx * 127.0) as i8, 0, (nz * 127.0) as i8, 0];
            for (x, yy, z, u, v) in [(ax, ay, z0, perim, vb), (bx, by, z0, perim + l, vb), (bx, by, z1, perim + l, z1 - g), (ax, ay, z1, perim, z1 - g)] {
                part.0.push(BVertex { pos: [x, z, -yy], nrm: nn, uv: [u, v], meta, days, base: z0 });
            }
            part.1.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            perim += l;
        }
        let tri = earcut(&xs, &ys);
        let base = part.0.len() as u32;
        for k in 0..nv {
            part.0.push(BVertex { pos: [xs[k], z1, -ys[k]], nrm: [0, 127, 0, 0], uv: [0.0, -1.0], meta: [meta[0], meta[1], meta[2], 1], days, base: z0 });
        }
        for c in tri.chunks(3) {
            part.1.extend_from_slice(&[base + c[0], base + c[2], base + c[1]]);
        }
        // 屋頂細節：女兒牆（12 m 以上）、屋頂機房（15 m 以上，非透天）
        if h >= 12.0 && t != 9 {
            let ph = if h > 60.0 { 1.6 } else { 1.0 };
            for k in 0..nv {
                let k2 = (k + 1) % nv;
                let (ax, ay, bx, by) = (xs[k], ys[k], xs[k2], ys[k2]);
                let l = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                if l < 0.3 { continue; }
                let (nx, nz) = ((by - ay) / l, (bx - ax) / l);
                for (sgn, off) in [(1.0f32, 0.0f32), (-1.0, 0.35)] {
                    let (ix, iy) = (-(by - ay) / l * off, (bx - ax) / l * off);
                    let base = part.0.len() as u32;
                    let nn = [(nx * 127.0 * sgn) as i8, 0, (nz * 127.0 * sgn) as i8, 0];
                    for (x, yy, z) in [(ax + ix, ay + iy, z1), (bx + ix, by + iy, z1), (bx + ix, by + iy, z1 + ph), (ax + ix, ay + iy, z1 + ph)] {
                        part.0.push(BVertex { pos: [x, z, -yy], nrm: nn, uv: [0.0, -2.0], meta: [meta[0], meta[1], meta[2], 2], days, base: z0 });
                    }
                    part.1.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
            if h >= 15.0 && t != 3 && t != 10 {
                let (mnx, mxx) = xs.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
                let (mny, mxy) = ys.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
                let sz = ((mxx - mnx).min(mxy - mny) * 0.3).clamp(2.5, 12.0);
                let (bh, cx2, cy2) = (if t == 8 || t == 0 { 5.0 } else { 3.0 }, cx, cyy);
                let q = [(cx2 - sz / 2.0, cy2 - sz / 2.0), (cx2 + sz / 2.0, cy2 - sz / 2.0), (cx2 + sz / 2.0, cy2 + sz / 2.0), (cx2 - sz / 2.0, cy2 + sz / 2.0)];
                for k in 0..4 {
                    let (a_, b_) = (q[k], q[(k + 1) % 4]);
                    let l = ((b_.0 - a_.0).powi(2) + (b_.1 - a_.1).powi(2)).sqrt();
                    let (nx, nz) = ((b_.1 - a_.1) / l, (b_.0 - a_.0) / l);
                    let base = part.0.len() as u32;
                    for (x, yy, z) in [(a_.0, a_.1, z1), (b_.0, b_.1, z1), (b_.0, b_.1, z1 + bh), (a_.0, a_.1, z1 + bh)] {
                        part.0.push(BVertex { pos: [x, z, -yy], nrm: [(nx * 127.0) as i8, 0, (nz * 127.0) as i8, 0], uv: [0.0, -2.0], meta: [meta[0], meta[1], meta[2], 2], days, base: z0 });
                    }
                    part.1.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
                let base = part.0.len() as u32;
                for &(x, yy) in &q { part.0.push(BVertex { pos: [x, z1 + bh, -yy], nrm: [0, 127, 0, 0], uv: [0.0, -1.0], meta: [meta[0], meta[1], meta[2], 1], days, base: z0 }); }
                part.1.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
        info.push(BInfo { xs, ys, z0, z1, t, floors: fl, uw, ud, h, s: sd, e: ed, dd });
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
        let o = s * 20;
        if o + 20 > b.len() { break; }
        let (k, br) = (b[o] as usize, b[o + 1]);
        let day = u16::from_le_bytes([b[o + 2], b[o + 3]]);
        let (ax, ay, bx, by) = (f(o + 4), f(o + 8), f(o + 12), f(o + 16));
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
            let (h0, h1) = (hh(x0, y0) + lift, hh(x1, y1) + lift);
            let base = v.len() as u32;
            let meta = [k as u16, day];
            let col = [c[0], c[1], c[2], 255];
            for (x, yy, h) in [(x0 + nx, y0 + ny, h0), (x0 - nx, y0 - ny, h0), (x1 - nx, y1 - ny, h1), (x1 + nx, y1 + ny, h1)] {
                v.push(RVertex { pos: [x, h, -yy], col, meta });
            }
            idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
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
