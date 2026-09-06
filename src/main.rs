//! Desktop entry point for Prompt Box.

use promptbox::PromptBoxApp;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("promptbox=info"))
        .init();
    whisper_rs::install_logging_hooks();

    // The running process sets its own Dock icon; without this macOS shows
    // a generic one while the app is open. Raw RGBA from scripts/icon.sh.
    let icon = egui::IconData {
        rgba: include_bytes!("../assets/icon-256.rgba").to_vec(),
        width: 256,
        height: 256,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([280.0, 200.0])
            .with_icon(icon)
            // Transparency support is decided once, here, for every
            // viewport; the caption overlay depends on it. The main
            // window's panels paint opaque backgrounds regardless.
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "Prompt Box",
        options,
        Box::new(|cc| {
            let mut app = PromptBoxApp::new(cc);
            // Dev aid: `PROMPTBOX_AUTOSTART=1 cargo run` begins listening at launch.
            if std::env::var_os("PROMPTBOX_AUTOSTART").is_some() {
                app.start_listening();
            }
            // `PROMPTBOX_DEMO=1` plays the fake dictation script at launch.
            if std::env::var_os("PROMPTBOX_DEMO").is_some() {
                app.start_demo(false);
            }
            Ok(Box::new(app))
        }),
    )
}
