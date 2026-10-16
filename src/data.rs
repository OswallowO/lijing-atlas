//! 資料包讀取：data/ 目錄（與網頁版同一套前處理產物）。
use crate::geo::{Geo, HeightGrid};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone)]
pub struct TerrainMeta {
    pub xmin: f64,
    pub xmax: f64,
    pub ymin: f64,
    pub ymax: f64,
    pub w: usize,
    pub h: usize,
    pub pxm: f64,
}

#[derive(Deserialize, Clone)]
pub struct Meta {
    pub origin: [f64; 2],
    pub chunk: f64,
    pub terrain: TerrainMeta,
    pub bld: Vec<[i64; 5]>,
    pub roads: Vec<[i64; 5]>,
    #[serde(rename = "roadKeys")]
    pub road_keys: Vec<String>,
    pub stats: Value,
    pub cells: Value,
    #[serde(rename = "keepCount", default)]
    pub keep_count: u64,
    #[serde(default)]
    pub version: String,
}

/// 匯出網格：位置、法向、顏色（a＝自發光強度）、群組與年份。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    pub col: [u8; 4],
    pub group: u16,
    pub year: u16,
}

#[derive(Deserialize, Clone)]
pub struct TransitSeg {
    pub pts: Vec<[f32; 3]>,
    pub kinds: Vec<u8>,
}
#[derive(Deserialize, Clone)]
pub struct TransitLine {
    pub id: String,
    pub name: String,
    pub sub: String,
    pub color: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub y: i32,
    pub segs: Vec<TransitSeg>,
    #[serde(default)]
    pub stations: Vec<Vec<String>>,
    #[serde(default)]
    pub note: String,
}
#[derive(Deserialize, Clone)]
pub struct Station {
    pub n: String,
    pub ll: [f64; 2],
    pub lines: Vec<String>,
    pub yr: i32,
    pub kind: String,
}
#[derive(Deserialize, Clone)]
pub struct SceneMeta {
    pub groups: Vec<String>,
    pub lines: Vec<TransitLine>,
    pub stations: Vec<Station>,
    pub interiors: Vec<InteriorMeta>,
}
#[derive(Deserialize, Clone)]
pub struct InteriorTag {
    pub text: String,
    pub p: [f32; 3],
}
#[derive(Deserialize, Clone)]
pub struct InteriorLevel {
    pub name: String,
    pub y: f32,
    pub h: f32,
    pub off: usize,
    pub count: usize,
    pub tags: Vec<InteriorTag>,
}
#[derive(Deserialize, Clone)]
pub struct InteriorMeta {
    pub kind: String,
    pub note: String,
    pub size: f32,
    pub levels: Vec<InteriorLevel>,
}

pub struct Data {
    pub dir: PathBuf,
    pub meta: Meta,
    pub geo: Geo,
    pub hg: HeightGrid,
    pub coast: (usize, usize, Vec<u8>),
    pub ground: image::RgbaImage,
    pub ground_pre: image::RgbaImage,
    pub ground_dev: image::RgbaImage,
    pub sites: Value,
    pub zone: Value,
    pub bld: Vec<u8>,
    pub roads: Vec<u8>,
    pub story: Value,
    pub scene_meta: SceneMeta,
    pub scene_verts: Vec<MeshVertex>,
    pub interior_verts: Vec<MeshVertex>,
    pub villages: Value,
    pub islands: Value,
    pub peaks: Value,
}

fn read(dir: &Path, f: &str) -> Result<Vec<u8>, String> {
    std::fs::read(dir.join(f)).map_err(|e| format!("讀取 {f} 失敗：{e}"))
}
fn json(dir: &Path, f: &str) -> Result<Value, String> {
    serde_json::from_slice(&read(dir, f)?).map_err(|e| format!("{f}: {e}"))
}
fn verts(b: &[u8]) -> Vec<MeshVertex> {
    let n = b.len() / std::mem::size_of::<MeshVertex>();
    let mut v = vec![MeshVertex { pos: [0.0; 3], nrm: [0.0; 3], col: [0; 4], group: 0, year: 0 }; n];
    bytemuck::cast_slice_mut::<MeshVertex, u8>(&mut v).copy_from_slice(&b[..n * std::mem::size_of::<MeshVertex>()]);
    v
}

pub fn find_data_dir() -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    for c in [exe.join("data"), PathBuf::from("data"), exe.join("../../data"), exe.join("../../../data")] {
        if c.join("meta.json").exists() {
            return c;
        }
    }
    exe.join("data")
}

impl Data {
    pub fn load(dir: PathBuf, progress: &dyn Fn(f32, &str)) -> Result<Data, String> {
        progress(0.05, "讀取資料索引…");
        let meta: Meta = serde_json::from_slice(&read(&dir, "meta.json")?).map_err(|e| format!("meta.json: {e}"))?;
        let geo = Geo::new(meta.origin[0], meta.origin[1]);
        progress(0.1, "解碼高程…");
        let t = image::load_from_memory(&read(&dir, "terrain.png")?).map_err(|e| e.to_string())?.to_rgb8();
        let tm = &meta.terrain;
        let mut hd = vec![0f32; tm.w * tm.h];
        for (i, p) in t.pixels().enumerate().take(hd.len()) {
            hd[i] = p[0] as f32 * 256.0 + p[1] as f32 + p[2] as f32 / 256.0 - 32768.0;
        }
        let hg = HeightGrid { w: tm.w, h: tm.h, xmin: tm.xmin, ymax: tm.ymax, pxm: tm.pxm, data: hd };
        let c = image::load_from_memory(&read(&dir, "coast.png")?).map_err(|e| e.to_string())?.to_luma8();
        let coast = (c.width() as usize, c.height() as usize, c.into_raw());
        progress(0.25, "讀取地表貼圖…");
        let ground = image::load_from_memory(&read(&dir, "ground.jpg")?).map_err(|e| e.to_string())?.to_rgba8();
        let ground_pre = image::load_from_memory(&read(&dir, "ground_pre.jpg")?).map_err(|e| e.to_string())?.to_rgba8();
        let ground_dev = image::load_from_memory(&read(&dir, "ground_dev.png")?).map_err(|e| e.to_string())?.to_rgba8();
        let sites = json(&dir, "sites.json").unwrap_or(Value::Null);
        let zone = json(&dir, "zone.json").unwrap_or(Value::Null);
        progress(0.4, "讀取建物與道路…");
        let bld = read(&dir, "bld.bin")?;
        let roads = read(&dir, "roads.bin")?;
        let story = json(&dir, "story.json")?;
        progress(0.5, "讀取地標與路線模型…");
        let scene_meta: SceneMeta = serde_json::from_slice(&read(&dir, "scene.json")?).map_err(|e| format!("scene.json: {e}"))?;
        let scene_verts = verts(&read(&dir, "scene.bin")?);
        let interior_verts = verts(&read(&dir, "interiors.bin")?);
        let villages = json(&dir, "villages_real.json")?;
        let islands = json(&dir, "islands_real.json")?;
        let peaks = json(&dir, "peaks.json")?;
        Ok(Data { dir, meta, geo, hg, coast, ground, ground_pre, ground_dev, sites, zone, bld, roads, story, scene_meta, scene_verts, interior_verts, villages, islands, peaks })
    }
}
