//! UI tests. These run the real `eframe::App` headlessly via `egui_kittest`
//! with fake clipboard/history adapters and interact through accessible
//! labels. Core state transitions are covered in unit tests; these check the
//! important flows are wired to widgets.

use std::time::Duration;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use promptbox::PromptBoxApp;
use promptbox::adapters::clipboard::FakeClipboard;
use promptbox::adapters::persistence::MemoryStore;
use promptbox::core::SessionStatus;

fn harness_with(clipboard: FakeClipboard, store: MemoryStore) -> Harness<'static, PromptBoxApp> {
    Harness::new_eframe(move |_cc| {
        PromptBoxApp::with_services(Box::new(clipboard), Box::new(store))
    })
}

fn harness() -> Harness<'static, PromptBoxApp> {
    harness_with(FakeClipboard::default(), MemoryStore::default())
}

fn type_prompt(harness: &mut Harness<'static, PromptBoxApp>, text: &str) {
    let input = harness.get_by_role_and_label(Role::MultilineTextInput, "Prompt");
    input.focus();
    input.type_text(text);
    harness.run_steps(2);
}

#[test]
fn starts_idle_with_empty_prompt() {
    let harness = harness();
    harness.get_by_label("○ Idle");
    assert_eq!(harness.state().core().status(), &SessionStatus::Idle);
    assert!(harness.state().core().doc().is_empty());
}

#[test]
fn typing_edits_the_document_through_the_core() {
    let mut harness = harness();
    type_prompt(&mut harness, "Add tests");
    assert_eq!(harness.state().core().doc().committed(), "Add tests");
    assert_eq!(harness.state().core().doc().history().len(), 1);
}

#[test]
fn send_copies_clears_and_toasts() {
    let mut harness = harness();
    type_prompt(&mut harness, "ship it");
    harness.get_by_label("Send →").click();
    harness.run_steps(2);
    harness.get_by_label("Prompt copied");
    assert_eq!(harness.state().core().doc().committed(), "");
    assert_eq!(harness.state().core().recent()[0].text, "ship it");
}

#[test]
fn failed_clipboard_keeps_prompt_and_shows_error() {
    let clipboard = FakeClipboard {
        fail_with: Some("no clipboard".into()),
        ..Default::default()
    };
    let mut harness = harness_with(clipboard, MemoryStore::default());
    type_prompt(&mut harness, "keep me");
    harness.get_by_label("Send →").click();
    harness.run_steps(2);
    harness.get_by_label("Send failed: no clipboard. Prompt kept.");
    assert_eq!(harness.state().core().doc().committed(), "keep me");
    assert!(harness.state().core().recent().is_empty());
}

#[test]
fn copy_keeps_text() {
    let mut harness = harness();
    type_prompt(&mut harness, "twice");
    harness.get_by_label("Copy").click();
    harness.run_steps(2);
    harness.get_by_label("Copied");
    assert_eq!(harness.state().core().doc().committed(), "twice");
}

#[test]
fn demo_dictation_shows_provisional_then_committed_text() {
    let mut harness = harness();
    harness.get_by_label("Debug").click();
    harness.run_steps(2);
    harness.get_by_label("Demo dictation").click();
    harness.run_steps(2);
    harness.get_by_label("● Listening");

    // First partials arrive after ~850 ms of demo time.
    harness
        .state_mut()
        .advance_time(Duration::from_millis(1000));
    harness.run_steps(2);
    let doc = harness.state().core().doc();
    assert!(doc.provisional().is_some(), "expected provisional text");
    assert_eq!(doc.committed(), "");

    // Far enough for every sentence to be finalized and the demo to stop.
    harness.state_mut().advance_time(Duration::from_secs(120));
    harness.run_steps(3);
    let doc = harness.state().core().doc();
    assert!(doc.provisional().is_none());
    assert!(doc.committed().starts_with("Add a Pydantic model"));
    assert!(doc.committed().ends_with("refactor anything."));
    harness.get_by_label("○ Idle");
}

#[test]
fn typing_after_dictation_continues_after_the_dictated_text() {
    // Regression: egui's cursor stayed at the pre-dictation position, so
    // later input (typed or dictated) landed before the previous sentence.
    let mut harness = harness();
    type_prompt(&mut harness, "Intro.");
    harness.get_by_label("Debug").click();
    harness.run_steps(2);
    harness.get_by_label("Demo dictation").click();
    harness.run_steps(2);
    harness.state_mut().advance_time(Duration::from_secs(120));
    harness.run_steps(3);
    let committed = harness.state().core().doc().committed().to_owned();
    assert!(
        committed.starts_with("Intro. Add a Pydantic"),
        "{committed}"
    );

    let input = harness.get_by_role_and_label(Role::MultilineTextInput, "Prompt");
    input.focus();
    input.type_text(" Tail.");
    harness.run_steps(2);
    let after = harness.state().core().doc().committed().to_owned();
    assert_eq!(after, format!("{committed} Tail."));

    // A second dictation also lands at the end, not at the old spot.
    harness.get_by_label("Debug").click();
    harness.run_steps(2);
    harness.get_by_label("Demo dictation").click();
    harness.run_steps(2);
    harness.state_mut().advance_time(Duration::from_secs(120));
    harness.run_steps(3);
    let final_text = harness.state().core().doc().committed().to_owned();
    assert!(final_text.starts_with(&after), "{final_text}");
    assert!(final_text.ends_with("refactor anything."));
}

#[test]
fn demo_with_gap_shows_sticky_degraded_state_until_dismissed() {
    let mut harness = harness();
    harness.get_by_label("Debug").click();
    harness.run_steps(2);
    harness.get_by_label("Demo with gap").click();
    harness.run_steps(2);
    harness.state_mut().advance_time(Duration::from_secs(120));
    harness.run_steps(3);
    assert!(matches!(
        harness.state().core().status(),
        SessionStatus::Degraded(_)
    ));
    harness.get_by_label("Dismiss").click();
    harness.run_steps(2);
    harness.get_by_label("○ Idle");
}

#[test]
fn pin_toggle_persists_always_on_top() {
    let mut harness = harness();
    assert!(!harness.state().always_on_top());
    harness.get_by_label("📌").click();
    harness.run_steps(2);
    assert!(harness.state().always_on_top());
    harness.get_by_label("📌").click();
    harness.run_steps(2);
    assert!(!harness.state().always_on_top());
}

#[test]
fn dock_button_cycles_corners_starting_top_right() {
    use promptbox::app::Corner;
    let mut harness = harness();
    assert_eq!(harness.state().docked_corner(), None);
    let expected = [
        Corner::TopRight,
        Corner::BottomRight,
        Corner::BottomLeft,
        Corner::TopLeft,
        Corner::TopRight,
    ];
    for corner in expected {
        harness.get_by_label("Dock").click();
        harness.run_steps(2);
        assert_eq!(harness.state().docked_corner(), Some(corner));
    }
}

#[test]
fn draft_is_restored_on_startup() {
    let store = MemoryStore {
        draft: Some("unsent draft".into()),
        ..Default::default()
    };
    let harness = harness_with(FakeClipboard::default(), store);
    assert_eq!(harness.state().core().doc().committed(), "unsent draft");
    harness.get_by_label("Restored unsaved draft");
}
