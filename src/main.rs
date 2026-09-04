//! Desktop entry point for Prompt Box.

use promptbox::PromptBoxApp;

fn main() -> eframe::Result {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Prompt Box",
        options,
        Box::new(|cc| Ok(Box::new(PromptBoxApp::new(cc)))),
    )
}
