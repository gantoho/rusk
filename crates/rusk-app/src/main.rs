#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod editor;
#[cfg(target_os = "windows")]
mod native_menu;
mod theme;

use eframe::egui;

fn main() -> eframe::Result {
    let icon = load_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rusk")
            .with_inner_size([1280.0, 880.0])
            .with_min_inner_size([760.0, 540.0])
            .with_app_id("rusk")
            .with_icon(icon),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        "Rusk",
        options,
        Box::new(|cc| Ok(Box::new(app::RuskApp::new(cc)))),
    )
}

/// Intel's Vulkan driver (`igvk64.dll`) faults intermittently, so Windows uses
/// D3D12 instead of wgpu's default Vulkan-first order. Set `WGPU_BACKEND`
/// (e.g. `vulkan`) to override.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut config = eframe::egui_wgpu::WgpuConfiguration::default();
    #[cfg(target_os = "windows")]
    if std::env::var_os("WGPU_BACKEND").is_none()
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup
    {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
    config
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../../../assets/icon.png");
    match image::load_from_memory(bytes) {
        Ok(image) => {
            let rgba = image.to_rgba8();
            egui::IconData {
                width: rgba.width(),
                height: rgba.height(),
                rgba: rgba.into_raw(),
            }
        }
        Err(_) => egui::IconData::default(),
    }
}
