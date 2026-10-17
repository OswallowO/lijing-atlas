//! 視覺：白城瓷面板＋CeraCore 青色強調；光線配色；字型；共用小元件。
use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke, Vec2};
use glam::Vec3;

pub const INK: Color32 = Color32::from_rgb(0x16, 0x20, 0x2a);
pub const MUTED: Color32 = Color32::from_rgb(0x5b, 0x68, 0x75);
pub const LINE: Color32 = Color32::from_rgb(0xd6, 0xde, 0xe4);
pub const ACCENT: Color32 = Color32::from_rgb(0x0d, 0x7f, 0x86);
pub const WARN: Color32 = Color32::from_rgb(0xb5, 0x53, 0x2f);
pub const PAPER: Color32 = Color32::from_rgb(0xf7, 0xf9, 0xfa);

pub fn hex32(c: &str) -> Color32 {
    let v = crate::mesher::hex(c);
    Color32::from_rgb(v[0], v[1], v[2])
}
fn s2l(c: f32) -> f32 { if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
pub fn lin4(hex: &str, w: f32) -> [f32; 4] {
    let v = crate::mesher::hex(hex);
    [s2l(v[0] as f32 / 255.0), s2l(v[1] as f32 / 255.0), s2l(v[2] as f32 / 255.0), w]
}

pub struct Light {
    pub sun_dir: Vec3,
    pub sun: &'static str,
    pub sky: &'static str,
    pub ground: &'static str,
    pub fog: &'static str,
    pub night: f32,
    pub conc: &'static str,
    pub glass: &'static str,
    pub types: [&'static str; 12],
}
fn sun_v(az: f32, el: f32) -> Vec3 {
    let (a, e) = (az.to_radians(), el.to_radians());
    Vec3::new(a.sin() * e.cos(), e.sin(), -a.cos() * e.cos())
}
const fn types(w: &'static str, w2: &'static str, glass: &'static str, house: &'static str, stone: &'static str, real: &'static str) -> [&'static str; 12] {
    [w, w, w2, house, "#efe9df", w2, "#e9eef0", "#efebe4", glass, "#c8d0d8", real, stone]
}
pub fn light_preset(t: crate::app::Tod) -> Light {
    use crate::app::Tod::*;
    match t {
        Day => Light { sun_dir: sun_v(150.0, 52.0), sun: "#fffaf0", sky: "#c3d8ea", ground: "#a8a594", fog: "#d8e5ee", night: 0.0, conc: "#b6babd", glass: "#86a7c0", types: types("#f6f7f9", "#eef1f4", "#dbe7ef", "#efe6da", "#c3b7a6", "#e3ddd2") },
        Dawn => Light { sun_dir: sun_v(80.0, 14.0), sun: "#ffd2a8", sky: "#c9cbe0", ground: "#a89a8c", fog: "#f0ddd2", night: 0.0, conc: "#c2b9b1", glass: "#a49cb0", types: types("#fbf3ec", "#f1e4da", "#efe2dc", "#f4e3d4", "#c8b6a2", "#e6d8cc") },
        Dusk => Light { sun_dir: sun_v(280.0, 10.0), sun: "#ffaa78", sky: "#8f98c4", ground: "#7d6a62", fog: "#d6a690", night: 0.25, conc: "#9d958f", glass: "#8e7f94", types: types("#f2e2d8", "#e0cdc2", "#e8cfc4", "#e6cfbf", "#a99886", "#cbbcb0") },
        Night => Light { sun_dir: sun_v(160.0, 40.0), sun: "#8fa8ff", sky: "#1d2a44", ground: "#10141c", fog: "#0c1522", night: 1.0, conc: "#2a2f36", glass: "#2b3a52", types: types("#4a5868", "#3e4a59", "#3d4f6b", "#463f3c", "#3b3a37", "#2d343d") },
    }
}

/// 城市演變階段（只描述都市本身，不含劇情）
pub fn stages() -> Vec<(i32, i32, &'static str, &'static str)> {
    vec![
        (2014, 2014, "戰前・平潭縣", "Landsat 衛星影像判讀的 2014–15 年平潭：潭城、蘇澳、流水等老街區與鄉村聚落，兩三層石厝與透天厝為主，道路沿海岸與山腳發展；中心區的新路網才剛開始鋪設。"),
        (2015, 2015, "接收", "聯邦接收、劃設特別市。城市實體與戰前相同；全島重新測量，訂定總體規劃：沿島長軸的棋盤路網、約 1 km 的幹道超級街廓，每個超級街廓配一座鄰里公園與一所學校。"),
        (2016, 2018, "第一期建設", "先拆遷整地、再鋪幹道，房子在道路通車後才動工。北端 CBD 與中央政經區率先成形，金井港開港、西岸產業區設廠，市府與第一批大學校區落成。"),
        (2019, 2021, "骨幹成形", "M1 南北幹線、M2 東西幹線、M4 北端環線、M6 機場快線與填海機場啟用；高鐵跨海段通車，CBD 超高層群拔起，車站周邊率先開發。"),
        (2022, 2024, "白城", "M3 橘線、M7 環狀線與首批直通線通車，幹線公車（BRT）遍布每條幹道；全城建物陸續包覆白色外牆，議事堂與麗台跨海公路完工。"),
        (2025, 2025, "故事開場", "常住人口約 205 萬（規劃居住容量約 230 萬）。M8–M14 直通線補齊各區，十三條捷運線（約 240 km，全部在陸地上，跨海只有機場快線與離島輕軌）與離島輕軌、高鐵、幹線公車（BRT）與接駁公車構成完整路網，呈現故事開場時的面貌。"),
    ]
}

pub fn cat(c: &str) -> (&'static str, Color32, &'static str) {
    match c {
        "transit" => ("交通", hex32("#2e86ab"), "交"),
        "gov" => ("政府", hex32("#7a4fb0"), "政"),
        "edu" => ("大學", hex32("#c0573e"), "學"),
        "biz" => ("集團／商務", hex32("#16202a"), "商"),
        "res" => ("住所", hex32("#0d7f86"), "居"),
        "culture" => ("文化", hex32("#b88a2a"), "文"),
        "mil" => ("軍事", hex32("#5f7d3a"), "軍"),
        _ => ("機密", hex32("#8b1e1e"), "密"),
    }
}
pub fn type_cs2(t: u8) -> (&'static str, &'static str) {
    match t {
        0 => ("辦公", "Office（高密度）"),
        1 => ("高密度住宅", "High Density Residential"),
        2 => ("中密度住宅", "Medium Density Residential"),
        3 => ("低密度住宅（透天）", "Low Density Residential"),
        4 => ("混合住宅（店屋）", "Mixed Housing"),
        5 => ("公共服務／辦公", "Office / Service"),
        6 => ("研發／低密度辦公", "Office（低密度）/ Industrial"),
        7 => ("教育設施", "Education"),
        8 => ("超高層塔樓", "Office／Residential High"),
        9 => ("塔頂", "—"),
        _ => ("既有建物（戰前／平潭）", "Low Density Residential"),
    }
}

pub fn install_fonts(ctx: &egui::Context, data_dir: &std::path::Path) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(b) = std::fs::read(data_dir.join("font.ttf")) {
        fonts.font_data.insert("tc".into(), std::sync::Arc::new(egui::FontData::from_owned(b)));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().insert(0, "tc".into());
        }
    }
    ctx.set_fonts(fonts);
}
pub fn apply_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::light());
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 5.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.visuals.widgets.inactive.corner_radius = CornerRadius::same(7);
        s.visuals.widgets.hovered.corner_radius = CornerRadius::same(7);
        s.visuals.widgets.active.corner_radius = CornerRadius::same(7);
        s.visuals.selection.bg_fill = ACCENT;
        s.visuals.override_text_color = Some(INK);
    });
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(247, 249, 250, 242))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .shadow(egui::Shadow { offset: [0, 4], blur: 18, spread: 0, color: Color32::from_black_alpha(40) })
}
pub fn tool_button(ui: &mut egui::Ui, name: &str, key: &str, on: bool) -> egui::Response {
    let txt = egui::RichText::new(format!("{name}\n{key}")).size(12.0).color(if on { Color32::WHITE } else { INK });
    ui.add(egui::Button::new(txt).fill(if on { INK } else { Color32::TRANSPARENT }).min_size(Vec2::new(62.0, 38.0)))
}
pub fn seg(ui: &mut egui::Ui, name: &str, on: bool) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(name).size(12.5).color(if on { Color32::WHITE } else { INK })).fill(if on { INK } else { Color32::WHITE })).clicked()
}
pub fn eyebrow(ui: &mut egui::Ui, t: &str) {
    ui.label(egui::RichText::new(t).size(10.5).color(MUTED));
}
pub fn kv(ui: &mut egui::Ui, rows: &[(&str, String)]) {
    egui::Grid::new(ui.next_auto_id()).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(egui::RichText::new(*k).size(12.5).color(MUTED));
            ui.label(egui::RichText::new(v).size(12.5));
            ui.end_row();
        }
    });
}
pub fn chip(ui: &mut egui::Ui, t: &str, bg: Color32, fg: Color32) {
    egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(99)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.label(egui::RichText::new(t).size(11.5).color(fg));
    });
}
pub fn list_item(ui: &mut egui::Ui, bar: Color32, title: &str, sub: &str, n: &str) -> egui::Response {
    let r = egui::Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(5.0, 34.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 3.0, bar);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).size(13.0).strong());
                ui.label(egui::RichText::new(sub).size(11.0).color(MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(egui::RichText::new(n).size(11.0).color(MUTED)); });
        });
    });
    ui.interact(r.response.rect, ui.next_auto_id(), egui::Sense::click())
}
