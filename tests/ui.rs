//! UI tests. These run the real `eframe::App` headlessly via `egui_kittest`
//! and interact with widgets through their accessible labels. No GPU needed.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use promptbox::PromptBoxApp;

fn harness() -> Harness<'static, PromptBoxApp> {
    Harness::new_eframe(|cc| PromptBoxApp::new(cc))
}

#[test]
fn shows_heading_and_default_greeting() {
    let harness = harness();
    harness.get_by_label("Prompt Box");
    harness.get_by_label("Hello, World!");
    harness.get_by_label("Greeted 0 times");
}

#[test]
fn greet_button_increments_counter() {
    let mut harness = harness();

    harness.get_by_label("Greet").click();
    harness.run();
    harness.get_by_label("Greet").click();
    harness.run();

    harness.get_by_label("Greeted 2 times");
    assert_eq!(harness.state().greet_count(), 2);
}

#[test]
fn typing_a_name_updates_greeting() {
    let mut harness = harness();

    let input = harness.get_by_role_and_label(Role::TextInput, "Name");
    input.focus();
    input.type_text("Ada");
    harness.run();

    harness.get_by_label("Hello, Ada!");
}
