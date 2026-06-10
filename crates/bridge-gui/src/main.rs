mod app;
mod i18n;
mod logging;

use eframe::egui;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [400.0, 560.0];
const MIN_WINDOW_SIZE: [f32; 2] = [360.0, 420.0];

fn main() -> eframe::Result<()> {
    let log_sink: Box<dyn basement_bridge_core::ClientLogSink> =
        Box::new(logging::ClientLogFiles::new());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        basement_bridge_core::PRODUCT_NAME,
        options,
        Box::new(move |creation_context| {
            Ok(Box::new(app::BridgeApp::new(creation_context, log_sink)))
        }),
    )
}
