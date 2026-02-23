use anyhow::Result;
use eframe::egui;

use phazeai_ide::PhazeApp;

/// Embedded icon bytes (PNG, 256×256 RGBA).
const ICON_BYTES: &[u8] = include_bytes!("../../../assets/branding/icon_256.png");

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_BYTES).ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let settings = phazeai_core::Settings::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1400.0, 800.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("PhazeAI IDE");

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "PhazeAI IDE",
        options,
        Box::new(move |cc| Ok(Box::new(PhazeApp::new(cc, settings)))),
    )
    .map_err(|e| anyhow::anyhow!("Failed to start IDE: {e}"))?;

    Ok(())
}
