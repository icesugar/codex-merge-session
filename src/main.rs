#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use codex_merge_session::app::CodexMergeApp;
use codex_merge_session::codex_store::CodexStore;
use codex_merge_session::fonts::install_bundled_cjk_font;
use eframe::egui;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let user_profile = std::env::var("USERPROFILE").map_err(|error| error.to_string())?;
    let codex_root = std::path::PathBuf::from(user_profile).join(".codex");
    let store = CodexStore::new(codex_root);
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1120.0, 760.0]);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/app-icon.png"))
    {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "codex-merge-session",
        options,
        Box::new(move |creation_context| {
            install_bundled_cjk_font(&creation_context.egui_ctx);
            Ok(Box::new(CodexMergeApp::new(store)))
        }),
    )
    .map_err(|error| error.to_string())
}
