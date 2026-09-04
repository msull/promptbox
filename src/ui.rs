//! egui rendering and input mapping. Reads core state, draws, and turns
//! interaction into [`AppAction`]s. No transcript semantics live here.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{Key, KeyboardShortcut, Modifiers, RichText, TextEdit, TextStyle, Ui};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::PromptBoxApp;
use crate::core::{AppAction, SessionStatus};

const SEND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
const COPY_ALL: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C);

pub fn draw(app: &mut PromptBoxApp, ui: &mut Ui) {
    handle_shortcuts(app, ui);
    egui::Panel::top("top").show(ui, |ui| top_bar(app, ui));
    egui::Panel::bottom("bottom").show(ui, |ui| bottom_bar(app, ui));
    egui::CentralPanel::default().show(ui, |ui| editor(app, ui));
}

fn handle_shortcuts(app: &mut PromptBoxApp, ui: &mut Ui) {
    let (send, copy) = ui.input_mut(|i| (i.consume_shortcut(&SEND), i.consume_shortcut(&COPY_ALL)));
    if send {
        app.dispatch(AppAction::SendPrompt);
    }
    if copy {
        app.dispatch(AppAction::CopyPrompt);
    }
}

fn top_bar(app: &mut PromptBoxApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        status_indicator(app, ui);
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
    });
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
        SessionStatus::Degraded(why) => (
            "▲",
            format!("Degraded: {why}"),
            egui::Color32::from_rgb(0xe0, 0xa0, 0x20),
            true,
        ),
        SessionStatus::Error(why) => (
            "✖",
            format!("Error: {why}"),
            ui.visuals().error_fg_color,
            true,
        ),
    };
    ui.label(RichText::new(format!("{icon} {text}")).color(color));
    if ack && ui.small_button("Dismiss").clicked() {
        app.dispatch(AppAction::AcknowledgeStatus);
    }
}

fn bottom_bar(app: &mut PromptBoxApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("Undo").clicked() {
            app.dispatch(AppAction::Undo);
        }
        if ui.button("Clear").clicked() {
            app.dispatch(AppAction::ClearPrompt);
        }
        if let Some(toast) = app.core().toast() {
            let mut text = RichText::new(&toast.text);
            if toast.is_error {
                text = text.color(ui.visuals().error_fg_color);
            }
            ui.label(text);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.core().is_busy();
            if ui
                .add_enabled(!busy, egui::Button::new("Send →"))
                .on_hover_text("Copy to clipboard and clear (⌘↩)")
                .clicked()
            {
                app.dispatch(AppAction::SendPrompt);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Copy"))
                .on_hover_text("Copy without clearing (⌘⇧C)")
                .clicked()
            {
                app.dispatch(AppAction::CopyPrompt);
            }
        });
    });
}

fn editor(app: &mut PromptBoxApp, ui: &mut Ui) {
    let label = ui.label(RichText::new("Prompt").small().weak());
    let rendered = app.core().doc().rendered();
    let provisional = app.core().doc().provisional_range();
    let mut text = rendered.clone();

    let normal = ui.visuals().text_color();
    let dim = ui.visuals().weak_text_color();
    let font = TextStyle::Body.resolve(ui.style());
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let s = buf.as_str();
        let mut job = LayoutJob::default();
        let fmt = |color| TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        };
        match &provisional {
            Some(r)
                if r.end <= s.len() && s.is_char_boundary(r.start) && s.is_char_boundary(r.end) =>
            {
                job.append(&s[..r.start], 0.0, fmt(normal));
                job.append(&s[r.start..r.end], 0.0, fmt(dim));
                job.append(&s[r.end..], 0.0, fmt(normal));
            }
            _ => job.append(s, 0.0, fmt(normal)),
        }
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };

    let output = TextEdit::multiline(&mut text)
        .id_salt("prompt")
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
        if byte != app.core().doc().cursor() {
            app.dispatch(AppAction::CursorMoved(byte));
        }
    }
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
    }
}
