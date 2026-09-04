//! egui rendering and input mapping. Reads core state, draws, and turns
//! interaction into [`AppAction`]s. No transcript semantics live here.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Key, KeyboardShortcut, Modifiers, RichText, TextEdit, TextStyle, Ui};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{PromptBoxApp, Recognizer};
use crate::core::{AppAction, SessionStatus};
use crate::ports::history::ThemeChoice;

const SEND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
const COPY_ALL: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C);
const TOGGLE_LISTEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::L);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const DELETE_SENTENCE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Backspace);
const DELETE_PARAGRAPH: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Backspace);
const NEW_PARAGRAPH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter);
const CLEAR: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::K);

pub fn draw(app: &mut PromptBoxApp, ui: &mut Ui) {
    handle_shortcuts(app, ui);
    egui::Panel::top("top").show(ui, |ui| top_bar(app, ui));
    // Outermost first: the notification strip sits below the buttons and
    // keeps its height whether or not a toast is showing, so nothing above
    // it reflows when one appears.
    egui::Panel::bottom("notifications")
        .exact_size(NOTIFICATION_STRIP_HEIGHT)
        .resizable(false)
        .show(ui, |ui| notification_strip(app, ui));
    egui::Panel::bottom("bottom").show(ui, |ui| bottom_bar(app, ui));
    egui::Panel::bottom("ai-row").show(ui, |ui| ai_row(app, ui));
    egui::CentralPanel::default().show(ui, |ui| editor(app, ui));
    commands_popup(app, ui);
    settings_window(app, ui);
    projects_window(app, ui);
}

/// Editor for the project list. Everything is plain text, one entry per
/// line, and nothing is applied until Save.
fn projects_window(app: &mut PromptBoxApp, ui: &mut Ui) {
    if app.project_editor.is_none() {
        return;
    }
    let mut open = true;
    let mut action = None;
    egui::Window::new("Projects")
        .open(&mut open)
        .collapsible(false)
        .default_width(520.0)
        .show(ui.ctx(), |ui| {
            let Some(editor) = app.project_editor.as_mut() else {
                return;
            };
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(140.0);
                    for i in 0..editor.drafts.len() {
                        let name = editor.drafts[i].name.trim();
                        let label = if name.is_empty() { "(unnamed)" } else { name };
                        if ui.selectable_label(editor.selected == i, label).clicked() {
                            editor.selected = i;
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("New").clicked() {
                            editor.add();
                        }
                        if ui
                            .add_enabled(editor.drafts.len() > 1, egui::Button::new("Delete"))
                            .clicked()
                        {
                            editor.remove_selected();
                        }
                    });
                });
                ui.separator();
                project_form(ui, &mut editor.drafts[editor.selected]);
            });
            if let Some(err) = &editor.error {
                ui.colored_label(egui::Color32::from_rgb(0xd0, 0x40, 0x40), err);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    action = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(false);
                }
            });
        });
    match action {
        Some(true) => {
            app.save_project_editor();
        }
        Some(false) => app.project_editor = None,
        None => {}
    }
    if !open {
        app.project_editor = None;
    }
}

/// The fields of one project in the editor.
fn project_form(ui: &mut Ui, draft: &mut crate::app::ProjectDraft) {
    ui.vertical(|ui| {
        ui.set_width(340.0);
        ui.horizontal(|ui| {
            let label = ui.label("Name");
            ui.add(
                TextEdit::singleline(&mut draft.name)
                    .id(egui::Id::new("project-name"))
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(label.id);
        });
        field(
            ui,
            "Vocabulary",
            "Names the recognizer should expect, one per line.",
            &mut draft.vocabulary,
            "project-vocabulary",
        );
        field(
            ui,
            "Corrections",
            "heard words => Written Form, one per line. \
             Applied to new dictation, whole words, any case.",
            &mut draft.corrections,
            "project-corrections",
        );
        field(
            ui,
            "Glossary",
            "Term: what it means, one per line. Given to the AI.",
            &mut draft.glossary,
            "project-glossary",
        );
        field(
            ui,
            "AI context",
            "What the project is, conventions, what rewrites must keep.",
            &mut draft.context,
            "project-context",
        );
    });
}

/// A labelled multi-line box in the project editor.
fn field(ui: &mut Ui, label: &str, help: &str, text: &mut String, id: &str) {
    ui.add_space(4.0);
    let heading = ui.label(label).on_hover_text(help);
    ui.add(
        TextEdit::multiline(text)
            .id(egui::Id::new(id))
            .hint_text(help)
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    )
    .labelled_by(heading.id);
}

/// The bundled egui fonts lack most of the symbols the UI uses (status
/// dots, level-meter bars, ⇧ and ⌫ in shortcut hints). Append a system font
/// that has them as the last fallback; when none is found the glyphs render
/// as boxes but everything still works.
pub fn install_symbol_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Apple Symbols.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
        log::warn!("no system symbol font found; some icons will show as boxes");
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "symbols".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.push("symbols".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// Instruction box under the prompt: whatever is typed here is sent to the
/// model together with the whole prompt, and the reply replaces the prompt.
fn ai_row(app: &mut PromptBoxApp, ui: &mut Ui) {
    if let Some(cap) = app.core().instruction_capture().cloned() {
        capture_row(&cap, ui);
        return;
    }
    ui.horizontal(|ui| {
        let busy = app.core().ai_busy();
        let available = app.ai_available();
        let hint = if available {
            "Ask the AI to change the prompt… (Enter to send)"
        } else {
            "Set an OpenAI key in Settings to use AI"
        };
        let label = ui.label(RichText::new("AI").small().weak());
        let width = ui.available_width() - 60.0;
        let response = ui
            .add_enabled(
                !busy && available,
                TextEdit::singleline(&mut app.ai_instruction)
                    .id(egui::Id::new("ai-instruction"))
                    .hint_text(hint)
                    .desired_width(width),
            )
            .labelled_by(label.id);
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !busy;
        let clicked = ui
            .add_enabled(!busy && available, egui::Button::new("Ask"))
            .clicked();
        if (submitted || clicked) && !app.ai_instruction.trim().is_empty() {
            let instruction = std::mem::take(&mut app.ai_instruction);
            app.dispatch(AppAction::AiRewrite { instruction });
        }
    });
}

/// The instruction box while "Zevro enhance" is dictating into it: the
/// spoken words in blue (the current hypothesis dimmed) where the typed
/// instruction would go, until "confirm" sends it or "abort" drops it.
fn capture_row(cap: &crate::core::InstructionCapture, ui: &mut Ui) {
    let blue = egui::Color32::from_rgb(0x3a, 0x8d, 0xde);
    ui.horizontal(|ui| {
        ui.label(RichText::new("AI").small().color(blue));
        egui::Frame::new()
            .fill(blue.gamma_multiply(0.15))
            .stroke(egui::Stroke::new(1.0, blue))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if cap.text().is_empty() {
                        ui.label(
                            RichText::new("Say what the AI should do, then \"confirm\"…")
                                .color(blue)
                                .italics(),
                        );
                    } else {
                        if !cap.committed.is_empty() {
                            ui.label(RichText::new(&cap.committed).color(blue));
                        }
                        if !cap.partial.is_empty() {
                            let sep = if cap.committed.is_empty() { "" } else { " " };
                            ui.label(
                                RichText::new(format!("{sep}{}", cap.partial))
                                    .color(blue.gamma_multiply(0.6)),
                            );
                        }
                    }
                });
            });
    });
}

/// The paste-into-app options for Send, with the Accessibility status.
fn send_settings(app: &mut PromptBoxApp, ui: &mut Ui) {
    ui.vertical(|ui| {
        let mut on = app.settings().type_on_send;
        if ui
            .checkbox(&mut on, "Paste into the focused app (⌘V)")
            .on_hover_text(
                "Only when another app is in front, e.g. after \"Zevro send\". \
                 From this window Send just copies.",
            )
            .changed()
        {
            app.set_type_on_send(on);
        }
        let mut submit = app.settings().submit_after_paste;
        if ui
            .add_enabled(
                on,
                egui::Checkbox::new(&mut submit, "Press Return afterwards"),
            )
            .changed()
        {
            app.set_submit_after_paste(submit);
        }
        ui.horizontal(|ui| {
            if app.typing_permission_granted() {
                ui.label(RichText::new("Accessibility: granted").weak().small());
            } else {
                ui.label(
                    RichText::new("Accessibility: not granted")
                        .color(ui.visuals().error_fg_color)
                        .small(),
                );
                if ui.small_button("Request…").clicked() {
                    app.request_typing_permission();
                }
            }
        });
    });
}

fn settings_window(app: &mut PromptBoxApp, ui: &mut Ui) {
    if !app.show_settings {
        return;
    }
    let mut open = true;
    let mut save = false;
    let mut open_projects = false;
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ui.ctx(), |ui| {
            egui::Grid::new("settings-grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("OpenAI API key");
                    ui.add(
                        TextEdit::singleline(&mut app.settings_draft.openai_api_key)
                            .password(true)
                            .hint_text("sk-…")
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label("Model");
                    ui.add(
                        TextEdit::singleline(&mut app.settings_draft.openai_model)
                            .hint_text(crate::adapters::openai::DEFAULT_MODEL)
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label("Trigger word");
                    ui.add(
                        TextEdit::singleline(&mut app.settings_draft.trigger)
                            .hint_text(crate::core::commands::DEFAULT_TRIGGER)
                            .desired_width(240.0),
                    );
                    ui.end_row();
                    ui.label("Send");
                    send_settings(app, ui);
                    ui.end_row();
                    ui.label("Appearance");
                    ui.horizontal(|ui| {
                        let mut choice = app.theme();
                        for (value, label) in [
                            (ThemeChoice::Auto, "Auto"),
                            (ThemeChoice::Light, "Light"),
                            (ThemeChoice::Dark, "Dark"),
                        ] {
                            ui.selectable_value(&mut choice, value, label);
                        }
                        if choice != app.theme() {
                            app.set_theme(ui.ctx(), choice);
                        }
                    });
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Key in use: {}", app.api_key_source()))
                    .weak()
                    .small(),
            );
            let (p, c) = app.core().ai_tokens();
            ui.label(
                RichText::new(format!(
                    "AI tokens this session: {p} prompt, {c} completion"
                ))
                .weak()
                .small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Projects…").clicked() {
                    open_projects = true;
                }
            });
        });
    if save {
        app.save_settings_draft();
    }
    if open_projects {
        app.open_project_editor();
    }
    app.show_settings = open;
}

/// Floating list of voice commands, built from the parser's grammar.
fn commands_popup(app: &mut PromptBoxApp, ui: &mut Ui) {
    if !app.show_commands {
        return;
    }
    let mut trigger = app.core().trigger().to_owned();
    if let Some(first) = trigger.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    let mut open = true;
    egui::Window::new("Voice commands")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .show(ui.ctx(), |ui| {
            ui.label(format!(
                "Say \"{trigger}\" then a command, at the end of a sentence or on its own. \
                 Whole-command utterances ignore any garbled tail. \
                 Say \"abort\" after the trigger to cancel a command before it runs."
            ));
            ui.add_space(6.0);
            egui::Grid::new("commands-grid")
                .num_columns(2)
                .spacing([18.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for entry in crate::core::commands::help_entries() {
                        let phrases = entry
                            .phrases
                            .iter()
                            .map(|p| format!("{trigger} {p}"))
                            .collect::<Vec<_>>()
                            .join("  /  ");
                        ui.label(RichText::new(phrases).monospace());
                        ui.label(entry.command.description());
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Small slips are tolerated (\"sand\" for send, \"Zebro\" for the trigger).",
                )
                .weak()
                .small(),
            );
        });
    app.show_commands = open;
}

/// Shortcuts are consumed before the text box sees them, so the document's
/// single history owns undo/redo rather than egui's internal one.
fn handle_shortcuts(app: &mut PromptBoxApp, ui: &mut Ui) {
    // Order matters: more modifiers first so ⌘⇧Z is not eaten by ⌘Z.
    type Binding = (&'static KeyboardShortcut, fn() -> AppAction);
    const BINDINGS: &[Binding] = &[
        (&REDO, || AppAction::Redo),
        (&UNDO, || AppAction::Undo),
        (&DELETE_PARAGRAPH, || AppAction::DeleteParagraph),
        (&DELETE_SENTENCE, || AppAction::DeleteSentence),
        (&CLEAR, || AppAction::ClearPrompt),
        (&COPY_ALL, || AppAction::CopyPrompt),
        (&SEND, || AppAction::SendPrompt),
        (&NEW_PARAGRAPH, || AppAction::NewParagraph),
    ];
    let mut actions = Vec::new();
    let toggle = ui.input_mut(|i| {
        for (shortcut, make) in BINDINGS {
            if i.consume_shortcut(shortcut) {
                actions.push(make());
            }
        }
        i.consume_shortcut(&TOGGLE_LISTEN)
    });
    for action in actions {
        app.dispatch(action);
    }
    if toggle {
        if app.is_live() {
            app.stop_listening();
        } else {
            app.start_listening();
        }
    }
}

/// Height of the always-present toast strip at the bottom of the window.
const NOTIFICATION_STRIP_HEIGHT: f32 = 22.0;

fn notification_strip(app: &PromptBoxApp, ui: &mut Ui) {
    ui.horizontal_centered(|ui| {
        if let Some(toast) = app.core().toast() {
            let mut text = RichText::new(&toast.text).small();
            if toast.is_error {
                text = text.color(ui.visuals().error_fg_color);
            }
            ui.label(text).on_hover_text(&toast.text);
        }
    });
}

/// Below this width the top bar drops the project picker and Debug menu
/// so the window can shrink to a corner-sized note.
const COMPACT_WIDTH: f32 = 460.0;

fn top_bar(app: &mut PromptBoxApp, ui: &mut Ui) {
    let compact = ui.available_width() < COMPACT_WIDTH;
    // Right-side controls are laid out first so they always fit; the
    // status and project picker get whatever width is left and truncate.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            window_controls(app, ui, compact);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                status_indicator(app, ui);
                if !compact {
                    project_picker(app, ui);
                }
            });
        });
    });
    if !app.model_present() {
        model_download_row(app, ui);
    }
}

fn project_picker(app: &mut PromptBoxApp, ui: &mut Ui) {
    ui.separator();
    ui.label("Project");
    let selected = app.core().selected_project();
    let names: Vec<String> = app
        .core()
        .projects()
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let mut choice = selected;
    egui::ComboBox::from_id_salt("project")
        .selected_text(&names[selected])
        .show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                ui.selectable_value(&mut choice, i, name);
            }
        });
    if choice != selected {
        app.dispatch(AppAction::SelectProject(choice));
    }
    if ui
        .selectable_label(app.project_editor.is_some(), "Edit")
        .on_hover_text("Edit projects: vocabulary, corrections, glossary, AI context")
        .clicked()
    {
        if app.project_editor.is_some() {
            app.project_editor = None;
        } else {
            app.open_project_editor();
        }
    }
}

/// Settings, Pin, Dock, Debug, and Listen/Stop, right-aligned.
fn window_controls(app: &mut PromptBoxApp, ui: &mut Ui, compact: bool) {
    if ui
        .selectable_label(app.show_settings, "⚙")
        .on_hover_text("Settings: OpenAI key, model, trigger word")
        .clicked()
    {
        app.show_settings = !app.show_settings;
    }
    let mut pinned = app.always_on_top();
    if ui
        .toggle_value(&mut pinned, "Pin")
        .on_hover_text("Pin: keep this window above others")
        .changed()
    {
        app.set_always_on_top(ui.ctx(), pinned);
    }
    if ui
        .button("Dock")
        .on_hover_text("Dock: shrink and move to the next screen corner")
        .clicked()
    {
        app.dock_next_corner(ui.ctx());
    }
    if compact {
        listen_controls(app, ui, true);
        return;
    }
    ui.menu_button("Debug", |ui| {
        if app.is_demo_running() {
            if ui.button("Stop demo").clicked() {
                app.stop_demo();
            }
        } else {
            if ui.button("Demo dictation").clicked() {
                app.start_demo(false);
            }
            if ui.button("Demo with gap").clicked() {
                app.start_demo(true);
            }
        }
    });
    listen_controls(app, ui, false);
}

fn listen_controls(app: &mut PromptBoxApp, ui: &mut Ui, compact: bool) {
    let loading = matches!(app.recognizer(), Recognizer::Loading(_));
    let finishing = *app.core().status() == SessionStatus::Finishing;
    if app.is_live() {
        if ui
            .add_enabled(!finishing, egui::Button::new("Stop"))
            .on_hover_text("Stop listening (⌘L)")
            .clicked()
        {
            app.stop_listening();
        }
    } else {
        let label = match (loading, compact) {
            (true, true) => "Loading…",
            (true, false) => "Loading model…",
            (false, true) => "Listen",
            (false, false) => "Start listening",
        };
        if ui
            .add_enabled(!loading && !app.is_demo_running(), egui::Button::new(label))
            .on_hover_text("Start listening (⌘L)")
            .clicked()
        {
            app.start_listening();
        }
    }
}

fn model_download_row(app: &mut PromptBoxApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if let Some(d) = app.download() {
            let (done, total) = d.progress();
            let mb = |b: u64| b as f32 / 1_048_576.0;
            let frac = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_width(240.0)
                    .text(format!(
                        "Downloading base.en… {:.0} / {:.0} MB",
                        mb(done),
                        mb(total)
                    )),
            );
        } else {
            ui.label(RichText::new("No speech model yet.").weak());
            if ui.button("Download base.en (148 MB)").clicked() {
                app.start_download();
            }
            ui.label(
                RichText::new(format!("→ {}", app.model_path().display()))
                    .weak()
                    .small(),
            );
        }
    });
}

/// Five-bar level meter from the latest microphone level.
fn level_meter(ui: &mut Ui, level_db: f32, active: bool) {
    let bars = 5;
    let lit = if active {
        (((level_db + 60.0) / 50.0).clamp(0.0, 1.0) * bars as f32).round() as usize
    } else {
        0
    };
    let s: String = (0..bars).map(|i| if i < lit { '▮' } else { '▯' }).collect();
    let color = if active {
        egui::Color32::from_rgb(0x2e, 0xb8, 0x5c)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.label(RichText::new(s).color(color).monospace());
}

fn status_indicator(app: &mut PromptBoxApp, ui: &mut Ui) {
    let (icon, text, color, ack) = match app.core().status() {
        SessionStatus::Idle => (
            "○",
            "Idle".to_owned(),
            ui.visuals().weak_text_color(),
            false,
        ),
        SessionStatus::Listening => (
            "●",
            "Listening".to_owned(),
            egui::Color32::from_rgb(0x2e, 0xb8, 0x5c),
            false,
        ),
        SessionStatus::Finishing => (
            "◐",
            "Finishing…".to_owned(),
            egui::Color32::from_rgb(0x2e, 0xb8, 0x5c),
            false,
        ),
        SessionStatus::Degraded(why) => (
            "▲",
            format!("Degraded: {why}"),
            egui::Color32::from_rgb(0xe0, 0xa0, 0x20),
            true,
        ),
        SessionStatus::Error(why) => (
            "×",
            format!("Error: {why}"),
            ui.visuals().error_fg_color,
            true,
        ),
    };
    // A long error must not push the rest of the bar off screen: cap the
    // label, show the full message on hover, and keep Dismiss next to it.
    let reserved = if ack { 110.0 } else { 60.0 };
    let max = (ui.available_width() - reserved).clamp(60.0, 260.0);
    ui.scope(|ui| {
        ui.set_max_width(max);
        ui.add(egui::Label::new(RichText::new(format!("{icon} {text}")).color(color)).truncate())
            .on_hover_text(&text);
    });
    if ack && ui.small_button("Dismiss").clicked() {
        app.dispatch(AppAction::AcknowledgeStatus);
    }
    level_meter(ui, app.core().audio_level_db(), app.is_live());
}

fn bottom_bar(app: &mut PromptBoxApp, ui: &mut Ui) {
    // Wrapped so a narrow docked window keeps every button reachable.
    ui.horizontal_wrapped(|ui| {
        if ui.button("Undo").on_hover_text("⌘Z").clicked() {
            app.dispatch(AppAction::Undo);
        }
        if ui
            .add_enabled(app.core().doc().can_redo(), egui::Button::new("Redo"))
            .on_hover_text("⌘⇧Z")
            .clicked()
        {
            app.dispatch(AppAction::Redo);
        }
        if ui
            .button("Delete sentence")
            .on_hover_text("Remove the last sentence (⌘⌫). ⌘⇧⌫ removes the paragraph.")
            .clicked()
        {
            app.dispatch(AppAction::DeleteSentence);
        }
        if ui.button("Clear").on_hover_text("⌘⇧K").clicked() {
            app.dispatch(AppAction::ClearPrompt);
        }
        let busy = app.core().is_busy();
        let ai_busy = app.core().ai_busy();
        if ui
            .add_enabled(
                !ai_busy && app.ai_available(),
                egui::Button::new("Clean up"),
            )
            .on_hover_text("AI: fix recognition errors and punctuation, remove filler (undoable)")
            .clicked()
        {
            app.dispatch(AppAction::AiCleanUp);
        }
        if ai_busy {
            ui.spinner();
            ui.label(RichText::new("AI is rewriting…").weak());
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Copy"))
            .on_hover_text("Copy without clearing (⌘⇧C)")
            .clicked()
        {
            app.dispatch(AppAction::CopyPrompt);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Send →"))
            .on_hover_text("Copy to clipboard and clear (⌘Return)")
            .clicked()
        {
            app.dispatch(AppAction::SendPrompt);
        }
        if ui
            .selectable_label(app.show_commands, "Commands")
            .on_hover_text("Show the voice commands")
            .clicked()
        {
            app.show_commands = !app.show_commands;
        }
    });
}

fn editor(app: &mut PromptBoxApp, ui: &mut Ui) {
    let label = ui.label(RichText::new("Prompt").small().weak());
    let rendered = app.core().doc().rendered();
    let provisional = app.core().doc().provisional_range();
    let pending_command = app.core().pending_command_range();
    let mut text = rendered.clone();

    // The document owns the cursor. When it moved for a non-typing reason
    // (a dictated sentence committed, undo, draft restore), push it into
    // egui's text state; otherwise egui's stale click position would be
    // read back below and every later utterance would anchor there.
    let editor_id = egui::Id::new("prompt-editor");
    let synced_key = egui::Id::new("prompt-editor-synced-cursor");
    let doc_cursor = app.core().doc().cursor();
    let last_synced: Option<usize> = ui.data(|d| d.get_temp(synced_key));
    if last_synced != Some(doc_cursor) {
        let mut state = TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
        let ci = char_index(&rendered, doc_cursor);
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        TextEdit::store_state(ui.ctx(), editor_id, state);
        ui.data_mut(|d| d.insert_temp(synced_key, doc_cursor));
    }

    let normal = ui.visuals().text_color();
    let dim = ui.visuals().weak_text_color();
    let accent = egui::Color32::from_rgb(0xe0, 0xa0, 0x20);
    let font = TextStyle::Body.resolve(ui.style());
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let s = buf.as_str();
        let mut job = LayoutJob::default();
        let fmt = |color| TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        };
        let valid = |r: &Range<usize>| {
            r.start <= r.end
                && r.end <= s.len()
                && s.is_char_boundary(r.start)
                && s.is_char_boundary(r.end)
        };
        match &provisional {
            Some(r) if valid(r) => {
                job.append(&s[..r.start], 0.0, fmt(normal));
                match &pending_command {
                    Some(c) if valid(c) && c.start >= r.start && c.end <= r.end => {
                        job.append(&s[r.start..c.start], 0.0, fmt(dim));
                        job.append(&s[c.start..c.end], 0.0, fmt(accent));
                        job.append(&s[c.end..r.end], 0.0, fmt(dim));
                    }
                    _ => job.append(&s[r.start..r.end], 0.0, fmt(dim)),
                }
                job.append(&s[r.end..], 0.0, fmt(normal));
            }
            _ => job.append(s, 0.0, fmt(normal)),
        }
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };

    let output = TextEdit::multiline(&mut text)
        .id(editor_id)
        .hint_text("Speak or type your prompt…")
        .desired_width(f32::INFINITY)
        .desired_rows(12)
        .layouter(&mut layouter)
        .show(ui);
    let response = output.response.response.clone().labelled_by(label.id);

    if text != rendered {
        let (range, replacement) = diff_edit(&rendered, &text);
        app.dispatch(AppAction::ReplaceText {
            range,
            text: replacement,
        });
    } else if response.has_focus()
        && let Some(cursor) = output.cursor_range.and_then(|r| r.single())
    {
        let byte = byte_offset(&text, cursor.index.0);
        if byte != doc_cursor {
            app.dispatch(AppAction::CursorMoved(byte));
            ui.data_mut(|d| d.insert_temp(synced_key, byte));
        }
    }
}

/// Number of chars before byte offset `byte` (clamped to the end).
fn char_index(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().count()
}

/// Byte offset of the `char_index`-th char (clamped to the end).
fn byte_offset(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map_or(s.len(), |(b, _)| b)
}

/// Turns a before/after pair into one range replacement on `old`, with the
/// range snapped outward to grapheme boundaries of `old`.
fn diff_edit(old: &str, new: &str) -> (Range<usize>, String) {
    let prefix = old
        .char_indices()
        .zip(new.char_indices())
        .take_while(|((_, a), (_, b))| a == b)
        .last()
        .map_or(0, |((i, c), _)| i + c.len_utf8());
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old[old.len() - max_suffix..]
        .char_indices()
        .rev()
        .zip(new[new.len() - max_suffix..].char_indices().rev())
        .take_while(|((_, a), (_, b))| a == b)
        .count();
    let suffix_bytes = old[old.len() - max_suffix..]
        .chars()
        .rev()
        .take(suffix)
        .map(char::len_utf8)
        .sum::<usize>();

    // The prefix/suffix bytes are identical in both strings, so a boundary
    // snap computed on either string is valid for both. Take the wider one:
    // a combining mark typed after "e" is mid-grapheme only in `new`.
    let head_extra = (prefix - snap_down(old, prefix)).max(prefix - snap_down(new, prefix));
    let old_tail = old.len() - suffix_bytes;
    let new_tail = new.len() - suffix_bytes;
    let tail_extra = (snap_up(old, old_tail) - old_tail).max(snap_up(new, new_tail) - new_tail);
    let start = prefix - head_extra;
    let end = old_tail + tail_extra;
    let replacement = new[start..new_tail + tail_extra].to_owned();
    (start..end, replacement)
}

/// Largest grapheme boundary of `s` at or below `pos`.
fn snap_down(s: &str, pos: usize) -> usize {
    s.grapheme_indices(true)
        .map(|(i, _)| i)
        .chain([s.len()])
        .take_while(|&i| i <= pos)
        .last()
        .unwrap_or(0)
}

/// Smallest grapheme boundary of `s` at or above `pos`.
fn snap_up(s: &str, pos: usize) -> usize {
    s.grapheme_indices(true)
        .map(|(i, _)| i)
        .chain([s.len()])
        .find(|&i| i >= pos)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_detects_insert_delete_and_replace() {
        assert_eq!(diff_edit("", "abc"), (0..0, "abc".into()));
        assert_eq!(diff_edit("abc", "abXc"), (2..2, "X".into()));
        assert_eq!(diff_edit("abc", "ac"), (1..2, String::new()));
        assert_eq!(
            diff_edit("hello world", "hello there"),
            (6..11, "there".into())
        );
        assert_eq!(diff_edit("aaa", "aa"), (2..3, String::new()));
        assert_eq!(diff_edit("abc", "abc"), (3..3, String::new()));
    }

    #[test]
    fn diff_snaps_to_grapheme_boundaries() {
        // Adding a combining accent after "e": the change lands mid-grapheme
        // in the new string but must cover the whole "e" in the old one.
        let (range, rep) = diff_edit("cafe!", "cafe\u{301}!");
        assert_eq!(range, 3..4);
        assert_eq!(rep, "e\u{301}");
        let (range, rep) = diff_edit("a👨‍👩‍👧b", "ab");
        assert_eq!(range, 1..19);
        assert_eq!(rep, "");
    }

    #[test]
    fn byte_offset_handles_multibyte() {
        assert_eq!(byte_offset("héllo", 2), 3);
        assert_eq!(byte_offset("hi", 10), 2);
        assert_eq!(char_index("héllo", 3), 2);
        assert_eq!(char_index("hi", 10), 2);
    }
}
