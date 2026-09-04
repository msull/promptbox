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
use promptbox::adapters::openai::FakeRewriter;
use promptbox::adapters::persistence::MemoryStore;
use promptbox::adapters::typist::FakeTypist;
use promptbox::core::SessionStatus;
use std::sync::Arc;

fn harness_with(clipboard: FakeClipboard, store: MemoryStore) -> Harness<'static, PromptBoxApp> {
    // Tall enough that the Settings window, including its Save button, is
    // on screen for clicks.
    Harness::builder()
        .with_size(egui::vec2(900.0, 900.0))
        .build_eframe(move |_cc| PromptBoxApp::with_services(Box::new(clipboard), Box::new(store)))
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
    // Later input, typed or dictated, must land after the dictated text,
    // not at wherever egui's own cursor was before dictation started.
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
fn delete_sentence_button_and_shortcuts_edit_through_the_core() {
    use egui::{Key, Modifiers};
    let mut harness = harness();
    type_prompt(&mut harness, "Keep this. Drop this.");
    harness.get_by_label("Delete sentence").click();
    harness.run_steps(2);
    assert_eq!(harness.state().core().doc().committed(), "Keep this.");

    // ⌘Z goes to the document history, not egui's text-box undo.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_eq!(
        harness.state().core().doc().committed(),
        "Keep this. Drop this."
    );
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    harness.run_steps(2);
    assert_eq!(harness.state().core().doc().committed(), "Keep this.");

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Backspace);
    harness.run_steps(2);
    assert_eq!(harness.state().core().doc().committed(), "");
    harness.get_by_label("Redo");
}

#[test]
fn shift_enter_starts_a_new_paragraph() {
    use egui::{Key, Modifiers};
    let mut harness = harness();
    type_prompt(&mut harness, "One.");
    harness.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    harness.run_steps(2);
    assert_eq!(harness.state().core().doc().committed(), "One.\n\n");
}

#[test]
fn commands_button_opens_the_voice_command_list() {
    let mut harness = harness();
    harness.get_by_label("Commands").click();
    harness.run_steps(2);
    harness.get_by_label("Voice commands");
    harness.get_by_label("Delete the last sentence");
    harness.get_by_label("Copy to the clipboard and clear");
    assert!(harness.state().show_commands);
    harness.get_by_label("Commands").click();
    harness.run_steps(2);
    assert!(!harness.state().show_commands);
}

fn wait_for_ai(harness: &mut Harness<'static, PromptBoxApp>) {
    for _ in 0..50 {
        harness.run_steps(2);
        if !harness.state().core().ai_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("AI rewrite did not finish");
}

#[test]
fn clean_up_button_replaces_prompt_with_ai_reply_and_is_undoable() {
    let mut harness = harness();
    harness.state_mut().set_rewriter(Arc::new(FakeRewriter {
        reply: Ok("Add a Pydantic model.".into()),
    }));
    type_prompt(&mut harness, "um add a a pydantic model");
    harness.get_by_label("Clean up").click();
    wait_for_ai(&mut harness);
    assert_eq!(
        harness.state().core().doc().committed(),
        "Add a Pydantic model."
    );
    harness.get_by_label("Rewritten. Undo restores the original.");
    harness.get_by_label("Undo").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().core().doc().committed(),
        "um add a a pydantic model"
    );
}

#[test]
fn ai_instruction_box_sends_and_failure_keeps_prompt() {
    use egui::Key;
    let mut harness = harness();
    harness.state_mut().set_rewriter(Arc::new(FakeRewriter {
        reply: Err("quota exceeded".into()),
    }));
    type_prompt(&mut harness, "keep me");
    let input = harness.get_by_role_and_label(Role::TextInput, "AI");
    input.focus();
    input.type_text("make it formal");
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    wait_for_ai(&mut harness);
    assert_eq!(harness.state().core().doc().committed(), "keep me");
    harness.get_by_label("AI rewrite failed: quota exceeded");
    assert_eq!(harness.state().ai_instruction, "");
}

#[test]
fn enhance_dictates_into_the_ai_bar_and_confirm_sends_it() {
    use promptbox::core::AppAction;
    use promptbox::ports::speech::{SpeechEvent, SpeechEventKind};
    let mut harness = harness();
    harness.state_mut().set_rewriter(Arc::new(FakeRewriter {
        reply: Ok("- make the tests pass".into()),
    }));
    type_prompt(&mut harness, "make the tests pass");
    let ev = |sequence, kind| {
        AppAction::SpeechEventReceived(SpeechEvent {
            session: 1,
            sequence,
            audio_range: 0..0,
            kind,
        })
    };
    harness.state_mut().dispatch(AppAction::SessionStarted(1));
    harness
        .state_mut()
        .dispatch(ev(1, SpeechEventKind::VoiceStarted { utterance: 1 }));
    harness.state_mut().dispatch(ev(
        2,
        SpeechEventKind::Final {
            utterance: 1,
            text: "Zevro enhance".into(),
            confidence: None,
        },
    ));
    harness.state_mut().dispatch(ev(
        3,
        SpeechEventKind::Partial {
            utterance: 2,
            revision: 1,
            text: "Turn this".into(),
        },
    ));
    harness.run_steps(2);
    harness.get_by_label("Turn this");
    assert_eq!(
        harness.state().core().doc().rendered(),
        "make the tests pass"
    );

    harness.state_mut().dispatch(ev(
        4,
        SpeechEventKind::Final {
            utterance: 2,
            text: "Turn this into a list, confirm.".into(),
            confidence: None,
        },
    ));
    harness.run_steps(2);
    assert!(harness.state().core().instruction_capture().is_none());
    wait_for_ai(&mut harness);
    assert_ne!(
        harness.state().core().doc().committed(),
        "make the tests pass"
    );
}

#[test]
fn project_editor_saves_persists_and_corrects_dictation() {
    use promptbox::core::AppAction;
    use promptbox::ports::speech::{SpeechEvent, SpeechEventKind};
    let mut harness = harness();
    harness.get_by_label("✎").click();
    harness.run_steps(4);
    harness.get_by_label("New").click();
    harness.run_steps(4);
    let name = harness.get_by_role_and_label(Role::TextInput, "Name");
    name.focus();
    name.type_text("Acme");
    harness.run_steps(2);
    let corrections = harness.get_by_role_and_label(Role::MultilineTextInput, "Corrections");
    corrections.focus();
    corrections.type_text("you never sheets => Univer Sheets");
    harness.run_steps(2);
    harness.get_by_label("Save").click();
    harness.run_steps(4);
    assert!(harness.state().project_editor.is_none());
    assert_eq!(harness.state().core().project().name, "Acme");
    assert_eq!(harness.state().settings().project, "Acme");
    let saved = harness.state().core().projects();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[1].corrections[0].to, "Univer Sheets");

    harness.state_mut().dispatch(AppAction::SessionStarted(1));
    harness
        .state_mut()
        .dispatch(AppAction::SpeechEventReceived(SpeechEvent {
            session: 1,
            sequence: 1,
            audio_range: 0..0,
            kind: SpeechEventKind::Final {
                utterance: 1,
                text: "open you never sheets".into(),
                confidence: None,
            },
        }));
    harness.run_steps(2);
    assert_eq!(
        harness.state().core().doc().committed(),
        "open Univer Sheets"
    );
}

#[test]
fn saved_projects_and_selection_are_restored_at_launch() {
    let mut store = MemoryStore::default();
    let mut acme = promptbox::core::Project::new("Acme");
    acme.vocabulary = vec!["Pydantic".into()];
    store.projects = vec![promptbox::core::Project::new("Default"), acme];
    store.settings.project = "Acme".into();
    let harness = harness_with(FakeClipboard::default(), store);
    assert_eq!(harness.state().core().project().name, "Acme");
    assert_eq!(
        harness.state().core().project().vocabulary,
        vec!["Pydantic"]
    );
}

#[test]
fn settings_window_saves_api_key_and_enables_ai() {
    let mut harness = harness();
    harness.get_by_label("⚙").click();
    // The window's grid needs a few frames to settle before its widgets
    // stop moving.
    harness.run_steps(4);
    harness.state_mut().settings_draft.openai_api_key = "sk-test".into();
    harness.get_by_label("Save").click();
    harness.run_steps(2);
    assert!(harness.state().ai_available());
    assert_eq!(harness.state().api_key_source(), "settings");
}

#[test]
fn settings_theme_toggle_applies_and_persists_immediately() {
    use promptbox::ports::history::ThemeChoice;
    let mut harness = harness();
    harness.get_by_label("⚙").click();
    // The window's grid needs a few frames to settle before its widgets
    // stop moving.
    harness.run_steps(4);
    harness.get_by_label("Dark").click();
    harness.run_steps(2);
    assert_eq!(harness.state().theme(), ThemeChoice::Dark, "no Save needed");
    assert_eq!(harness.ctx.theme(), egui::Theme::Dark);
}

#[test]
fn saved_theme_is_applied_at_launch() {
    use promptbox::ports::history::{Settings, ThemeChoice};
    let store = MemoryStore {
        settings: Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        },
        ..Default::default()
    };
    let mut harness = harness_with(FakeClipboard::default(), store);
    harness.run_steps(2);
    assert_eq!(harness.state().theme(), ThemeChoice::Dark);
    assert_eq!(harness.ctx.theme(), egui::Theme::Dark);
    // And the light override wins over a dark system too.
    let store = MemoryStore {
        settings: Settings {
            theme: ThemeChoice::Light,
            ..Settings::default()
        },
        ..Default::default()
    };
    let mut harness = harness_with(FakeClipboard::default(), store);
    harness.run_steps(2);
    assert_eq!(harness.ctx.theme(), egui::Theme::Light);
}

fn granted_typist() -> Box<dyn promptbox::ports::typist::Typist> {
    Box::new(FakeTypist {
        granted: true,
        ..Default::default()
    })
}

#[test]
fn send_from_our_own_window_copies_without_typing() {
    let mut harness = harness();
    harness.state_mut().set_typist(granted_typist());
    type_prompt(&mut harness, "hello");
    harness.get_by_label("Send →").click();
    harness.run_steps(2);
    harness.get_by_label("Prompt copied");
    assert_eq!(harness.state().core().doc().committed(), "");
}

#[test]
fn send_while_another_app_is_focused_pastes_and_submits() {
    let mut harness = harness();
    harness.state_mut().set_typist(granted_typist());
    type_prompt(&mut harness, "hello");
    harness.state_mut().set_window_focused(false);
    harness
        .state_mut()
        .dispatch(promptbox::core::AppAction::SendPrompt);
    harness.run_steps(2);
    harness.get_by_label("Prompt sent");
    assert_eq!(harness.state().core().doc().committed(), "");
}

#[test]
fn send_without_accessibility_keeps_the_prompt() {
    let mut harness = harness();
    harness.state_mut().set_typist(Box::new(FakeTypist {
        granted: false,
        fail_with: Some("Accessibility permission not granted".into()),
        ..Default::default()
    }));
    type_prompt(&mut harness, "hello");
    harness.state_mut().set_window_focused(false);
    harness
        .state_mut()
        .dispatch(promptbox::core::AppAction::SendPrompt);
    harness.run_steps(2);
    assert_eq!(harness.state().core().doc().committed(), "hello");
    harness.get_by_label(
        "Copied, but could not type into the app: Accessibility permission not granted. Prompt kept.",
    );
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
