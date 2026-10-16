//! 人口＋就業密度熱區貼圖（運輸模式）。
use crate::data::Data;

pub fn build(d: &Data) -> (u32, Vec<u8>, [f32; 4]) {
    const N: usize = 512;
    let cells = d.meta.cells.as_array().cloned().unwrap_or_default();
    let mut pts = vec![];
    for c in &cells {
        let ll = &c["ll"];
        let (lon, lat) = (ll[0].as_f64().unwrap_or(0.0), ll[1].as_f64().unwrap_or(0.0));
        let (x, y) = d.geo.to_local(lon, lat);
        let v = c["pop"].as_f64().unwrap_or(0.0) + c["jobs"].as_f64().unwrap_or(0.0);
        pts.push((x as f32, y as f32, v as f32));
    }
    if pts.is_empty() {
        return (N as u32, vec![0; N * N], [0.0, 1.0, 0.0, 1.0]);
    }
    let b = [
        pts.iter().map(|p| p.0).fold(f32::MAX, f32::min) - 2000.0,
        pts.iter().map(|p| p.0).fold(f32::MIN, f32::max) + 2000.0,
        pts.iter().map(|p| p.1).fold(f32::MAX, f32::min) - 2000.0,
        pts.iter().map(|p| p.1).fold(f32::MIN, f32::max) + 2000.0,
    ];
    let maxv = pts.iter().map(|p| p.2).fold(0.0, f32::max).max(1.0);
    let mut acc = vec![0f32; N * N];
    let r = 900.0 / (b[1] - b[0]) * N as f32;
    for &(x, y, v) in &pts {
        let px = (x - b[0]) / (b[1] - b[0]) * N as f32;
        let py = (b[3] - y) / (b[3] - b[2]) * N as f32;
        let a = (v / maxv * 2.2).min(1.0) * 0.45;
        let (x0, x1) = ((px - r).max(0.0) as usize, ((px + r) as usize).min(N - 1));
        let (y0, y1) = ((py - r).max(0.0) as usize, ((py + r) as usize).min(N - 1));
        for j in y0..=y1 {
            for i in x0..=x1 {
                let d = ((i as f32 + 0.5 - px).powi(2) + (j as f32 + 0.5 - py).powi(2)).sqrt() / r;
                if d < 1.0 {
                    acc[j * N + i] += a * (1.0 - d);
                }
            }
        }
    }
    (N as u32, acc.iter().map(|v| (v.min(1.0) * 255.0) as u8).collect(), b)
}
