use anyhow::Result;
use eframe::egui::FontFamily;

use codex_merge_session::fonts::build_bundled_font_definitions;

#[test]
fn bundled_font_definitions_include_cjk_font() -> Result<()> {
    let definitions = build_bundled_font_definitions();

    assert!(definitions.font_data.contains_key("system-cjk"));
    assert!(definitions
        .families
        .get(&FontFamily::Proportional)
        .expect("proportional family")
        .contains(&"system-cjk".to_string()));
    assert!(definitions
        .families
        .get(&FontFamily::Monospace)
        .expect("monospace family")
        .contains(&"system-cjk".to_string()));
    Ok(())
}
