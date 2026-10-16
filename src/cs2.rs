//! CS2 高度圖輸出：可建造範圍 4096² 16-bit 灰階 PNG（高度範圍 4096 m）。
use crate::data::Data;
use std::path::Path;

pub fn export(d: &Data, frame: (f64, f64, f32, f32), sea: f32, path: &Path) -> Result<String, String> {
    const N: usize = 4096;
    let (lon, lat, span, rot) = frame;
    let (cx, cy) = d.geo.to_local(lon, lat);
    let a = (rot as f64).to_radians();
    let span = span as f64;
    let mut v = vec![0u16; N * N];
    let (mut mn, mut mx) = (f32::MAX, f32::MIN);
    for j in 0..N {
        let gy = span / 2.0 - (j as f64 + 0.5) / N as f64 * span;
        for i in 0..N {
            let gx = -span / 2.0 + (i as f64 + 0.5) / N as f64 * span;
            let (x, y) = (cx + gx * a.cos() - gy * a.sin(), cy + gx * a.sin() + gy * a.cos());
            let e = d.hg.at(x, y) + sea;
            mn = mn.min(e);
            mx = mx.max(e);
            v[j * N + i] = ((e / 4096.0 * 65535.0).round()).clamp(0.0, 65535.0) as u16;
        }
    }
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), N as u32, N as u32);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(N * N * 2);
    for x in v { bytes.extend_from_slice(&x.to_be_bytes()); }
    w.write_image_data(&bytes).map_err(|e| e.to_string())?;
    Ok(format!("已存檔：{}（高程 {:.0} ~ {:.0} m）", path.display(), mn, mx))
}
