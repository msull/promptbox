//! Application state and rendering.
//!
//! Design rule: [`PromptBoxApp`] owns plain data and pure methods. The
//! [`eframe::App::ui`] implementation only reads/writes that data and
//! draws widgets. This keeps logic unit-testable without egui and lets UI
//! tests (see `tests/ui.rs`) drive the real app through its widgets.

/// Top-level application state.
#[derive(Debug, Default)]
pub struct PromptBoxApp {
    name: String,
    greet_count: u32,
}

impl PromptBoxApp {
    /// Creates the app. The creation context gives access to egui settings
    /// (fonts, storage, etc.) when we need them later.
    #[must_use]
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    /// The greeting shown for the current name.
    #[must_use]
    pub fn greeting(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            "Hello, World!".to_owned()
        } else {
            format!("Hello, {name}!")
        }
    }

    /// Records one press of the Greet button.
    pub fn greet(&mut self) {
        self.greet_count += 1;
    }

    /// How many times Greet has been pressed.
    #[must_use]
    pub fn greet_count(&self) -> u32 {
        self.greet_count
    }
}

impl eframe::App for PromptBoxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Prompt Box");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let label = ui.label("Name");
                ui.text_edit_singleline(&mut self.name)
                    .labelled_by(label.id);
            });

            if ui.button("Greet").clicked() {
                self.greet();
            }

            ui.add_space(8.0);
            ui.label(self.greeting());
            ui.label(format!("Greeted {} times", self.greet_count));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_defaults_to_world() {
        let app = PromptBoxApp::default();
        assert_eq!(app.greeting(), "Hello, World!");
    }

    #[test]
    fn greeting_uses_name() {
        let app = PromptBoxApp {
            name: "  Sully ".to_owned(),
            ..Default::default()
        };
        assert_eq!(app.greeting(), "Hello, Sully!");
    }

    #[test]
    fn greet_increments_count() {
        let mut app = PromptBoxApp::default();
        app.greet();
        app.greet();
        assert_eq!(app.greet_count(), 2);
    }
}
