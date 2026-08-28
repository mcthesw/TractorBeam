#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

rust_i18n::i18n!("locales", fallback = "en_US");

mod app;
mod application;
mod i18n;
mod logging;
mod update;

use eframe::egui;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [480.0, 720.0];
const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 480.0];

fn main() -> eframe::Result<()> {
    prefer_x11_on_cosmic();
    let app_title = format!(
        "{} {}",
        tractor_beam_core::PRODUCT_NAME,
        tractor_beam_core::build_info::version_label()
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        &app_title,
        options,
        Box::new(|creation_context| Ok(Box::new(app::BridgeApp::new(creation_context)))),
    )
}

fn prefer_x11_on_cosmic() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WINIT_UNIX_BACKEND").is_some() {
            return;
        }
        let cosmic = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_uppercase()
            .contains("COSMIC");
        if cosmic && std::env::var_os("DISPLAY").is_some() {
            // COSMIC's Wayland compositor is rejected by glutin's native window path.
            unsafe {
                std::env::set_var("WINIT_UNIX_BACKEND", "x11");
            }
        }
    }
}
