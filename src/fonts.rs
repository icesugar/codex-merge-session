use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

const SYSTEM_CJK_FONT_NAME: &str = "system-cjk";
const BUNDLED_CJK_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansSC-Regular.otf");

pub fn build_bundled_font_definitions() -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    definitions.font_data.insert(
        SYSTEM_CJK_FONT_NAME.to_string(),
        Arc::new(FontData::from_static(BUNDLED_CJK_FONT_BYTES)),
    );

    let cjk_font_name = SYSTEM_CJK_FONT_NAME.to_string();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let entry = definitions.families.entry(family).or_default();
        if !entry.contains(&cjk_font_name) {
            entry.push(cjk_font_name.clone());
        }
    }

    definitions
}

pub fn install_bundled_cjk_font(ctx: &egui::Context) {
    ctx.set_fonts(build_bundled_font_definitions());
}
