mod app;
mod i18n;
mod logging;

use eframe::egui;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [480.0, 720.0];
const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 480.0];

fn main() -> eframe::Result<()> {
    let log_sink: Box<dyn basement_bridge_core::ClientLogSink> =
        Box::new(logging::ClientLogFiles::new());
    let app_title = format!(
        "{} {}",
        basement_bridge_core::PRODUCT_NAME,
        basement_bridge_core::build_info::version_label()
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
        Box::new(move |creation_context| {
            Ok(Box::new(app::BridgeApp::new(creation_context, log_sink)))
        }),
    )
}
