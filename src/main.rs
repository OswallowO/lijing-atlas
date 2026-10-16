//! 麗京特別市 城市圖鑑（原生版）
#![allow(deprecated)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod app;
mod camera;
mod cs2;
mod data;
mod geo;
mod heat;
mod interior;
mod mesher;
mod render;
mod style;
mod update;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(ref mut c) = wgpu_options.wgpu_setup {
        c.power_preference = egui_wgpu::wgpu::PowerPreference::HighPerformance; // 筆電優先用獨立顯卡
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("麗京特別市 城市圖鑑").with_inner_size([1440.0, 900.0]).with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native("麗京特別市 城市圖鑑", opts, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
