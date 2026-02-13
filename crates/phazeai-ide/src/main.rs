use anyhow::Result;
use eframe::egui;

mod app;
mod panels;
mod themes;
mod keybindings;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let settings = phazeai_core::Settings::load();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 800.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("PhazeAI IDE"),
        ..Default::default()
    };

    eframe::run_native(
        "PhazeAI IDE",
        options,
        Box::new(move |cc| Ok(Box::new(app::PhazeApp::new(cc, settings)))),
    )
    .map_err(|e| anyhow::anyhow!("Failed to start IDE: {e}"))?;

    Ok(())
}
