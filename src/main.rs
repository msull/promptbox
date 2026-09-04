//! Desktop entry point for Prompt Box.

use promptbox::PromptBoxApp;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("promptbox=info"))
        .init();
    whisper_rs::install_logging_hooks();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([280.0, 200.0]),
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
            Ok(Box::new(app))
        }),
    )
}
