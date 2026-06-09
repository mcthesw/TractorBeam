use std::{fs, path::Path};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

pub(super) fn configure_fonts(context: &egui::Context) {
    let Some(font_bytes) = load_cjk_font() else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "system_cjk".to_owned(),
        FontData::from_owned(font_bytes).into(),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "system_cjk".to_owned());
    }
    context.set_fonts(fonts);
}

fn load_cjk_font() -> Option<Vec<u8>> {
    [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ]
    .iter()
    .map(Path::new)
    .find_map(|path| fs::read(path).ok())
}
