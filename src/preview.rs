//! Whole-prompt preview: a translucent panel showing the entire prompt
//! (live text in amber) with a status line underneath, so dictation can be
//! followed and reviewed while Prompt Box itself is out of sight.
//!
//! Opened by "Zevro preview", or automatically while listening when the
//! setting is on. It closes when "preview" is said again, the prompt is
//! sent or cleared, or (outside auto mode) the prompt has not changed for
//! a while. The panel is draggable, so it is not click-through; its
//! position is remembered for the session.

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId,
};

use crate::app::Editor;

/// Seconds without a change to the prompt before the preview closes
/// (only when it was opened by hand).
const IDLE_SECS: f64 = 60.0;
/// Fraction of the monitor the panel may use.
const WIDTH_FRACTION: f32 = 0.6;
const HEIGHT_FRACTION: f32 = 0.55;
const MAX_WIDTH: f32 = 1000.0;
const FONT_SIZE: f32 = 22.0;
const STATUS_FONT_SIZE: f32 = 15.0;
const STATUS_HEIGHT: f32 = 26.0;
const PADDING: f32 = 28.0;
const BOX_ALPHA: u8 = 215;
const LIVE_COLOR: Color32 = Color32::from_rgb(255, 200, 110);
const STATUS_COLOR: Color32 = Color32::from_rgb(170, 200, 255);
const ERROR_COLOR: Color32 = Color32::from_rgb(255, 120, 110);

/// What the preview last showed, when it last changed (egui time), where
/// it was dragged to, and the last status message.
#[derive(Debug, Default)]
pub struct PreviewState {
    rendered: String,
    changed_at: f64,
    /// Top-left of the panel in global points, once dragged (kept for the
    /// session). The builder is given this only when the panel opens.
    remembered_pos: Option<Pos2>,
    /// Position the open panel was created at; fixed while it stays open so
    /// the OS drag is not fought every frame.
    anchor: Option<Pos2>,
    /// Whether listening was on last frame, for auto open/close.
    listening_was: bool,
    /// Whether the current showing was opened by auto mode.
    auto_opened: bool,
    /// Last toast text and whether it was an error; shown until replaced.
    status: Option<(String, bool)>,
}

impl PreviewState {
    /// Records the current prompt; returns whether it changed.
    fn update(&mut self, rendered: &str, now: f64) -> bool {
        if rendered == self.rendered {
            return false;
        }
        self.rendered.clear();
        self.rendered.push_str(rendered);
        self.changed_at = now;
        true
    }

    fn idle_for(&self, now: f64) -> f64 {
        now - self.changed_at
    }

    /// Auto mode: what the listening state's change means for the panel.
    /// `Some(true)` opens it, `Some(false)` closes it, `None` leaves it.
    fn auto_transition(&mut self, auto: bool, listening: bool) -> Option<bool> {
        let was = std::mem::replace(&mut self.listening_was, listening);
        if !auto || was == listening {
            return None;
        }
        if listening {
            self.auto_opened = true;
            Some(true)
        } else if self.auto_opened {
            self.auto_opened = false;
            Some(false)
        } else {
            None
        }
    }

    /// Remembers a toast so the panel keeps showing the last outcome.
    fn note_status(&mut self, toast: Option<(&str, bool)>) {
        if let Some((text, is_error)) = toast
            && self.status.as_ref().is_none_or(|(t, _)| t != text)
        {
            self.status = Some((text.to_owned(), is_error));
        }
    }
}

/// Draws the preview viewport while the core says it is open, at the
/// remembered position or centred in `area`. `listening` and `auto` drive
/// the automatic open/close.
pub fn draw(app: &mut Editor, ctx: &egui::Context, area: Rect, listening: bool, auto: bool) {
    if let Some(open) = app.preview.auto_transition(auto, listening) {
        app.set_preview_open(open);
    }
    if !app.core().preview_open() {
        app.preview.rendered.clear();
        app.preview.anchor = None;
        app.preview.auto_opened = false;
        return;
    }
    let now = ctx.input(|i| i.time);
    let doc = app.core().doc();
    let rendered = doc.rendered();
    let live = doc.provisional_range();
    let first_show = app.preview.anchor.is_none();
    app.preview.update(&rendered, now);
    let toast = app.core().toast().map(|t| (t.text.clone(), t.is_error));
    app.preview
        .note_status(toast.as_ref().map(|(t, e)| (t.as_str(), *e)));
    let hold = auto && listening;
    if !hold
        && (rendered.trim().is_empty() || (!first_show && app.preview.idle_for(now) > IDLE_SECS))
    {
        app.set_preview_open(false);
        return;
    }
    let busy = app.core().ai_busy();
    // Wake up to close on idle (or spin) even if nothing else repaints.
    ctx.request_repaint_after(std::time::Duration::from_millis(if busy {
        50
    } else {
        1000
    }));

    let size = Vec2::new(
        (area.width() * WIDTH_FRACTION).min(MAX_WIDTH),
        area.height() * HEIGHT_FRACTION,
    );
    let default_pos = area.center() - size / 2.0 - Vec2::new(0.0, 40.0);
    let anchor = *app
        .preview
        .anchor
        .get_or_insert_with(|| app.preview.remembered_pos.unwrap_or(default_pos));
    let status = app.preview.status.clone();
    let placeholder = if listening { "Listening…" } else { "" };

    let moved = ctx.show_viewport_immediate(
        ViewportId::from_hash_of("prompt-preview"),
        ViewportBuilder::default()
            .with_title("Prompt preview")
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_mouse_passthrough(false)
            .with_active(false)
            .with_taskbar(false)
            .with_resizable(false)
            .with_inner_size(size)
            .with_position(anchor),
        |ui, _class| {
            paint(
                ui,
                &rendered,
                live.clone(),
                placeholder,
                status.as_ref(),
                busy,
            );
            // Drag anywhere on the panel moves the window; the OS reports
            // where it ended up.
            let response = ui.interact(ui.max_rect(), ui.id().with("drag"), Sense::drag());
            if response.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            ui.ctx().input(|i| i.viewport().outer_rect.map(|r| r.min))
        },
    );
    if let Some(pos) = moved {
        app.preview.remembered_pos = Some(pos);
    }
}

/// Paints the prompt in a rounded translucent panel with a status line
/// (last voice outcome, spinner while the AI works), sized to the text up
/// to the panel's limits and clipped to show the tail when it overflows.
fn paint(
    ui: &mut egui::Ui,
    rendered: &str,
    live: Option<std::ops::Range<usize>>,
    placeholder: &str,
    status: Option<&(String, bool)>,
    busy: bool,
) {
    let painter = ui.painter();
    let rect = ui.max_rect();
    let font = FontId::proportional(FONT_SIZE);
    let mut job = LayoutJob::default();
    job.wrap.max_width = rect.width() - 2.0 * PADDING;
    let format = |color: Color32| TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    match live {
        Some(r) => {
            job.append(&rendered[..r.start], 0.0, format(Color32::WHITE));
            job.append(&rendered[r.clone()], 0.0, format(LIVE_COLOR));
            job.append(&rendered[r.end..], 0.0, format(Color32::WHITE));
        }
        None if rendered.trim().is_empty() => {
            job.append(placeholder, 0.0, format(Color32::from_gray(160)));
        }
        None => job.append(rendered, 0.0, format(Color32::WHITE)),
    }
    let galley = painter.layout_job(job);
    let visible_h = rect.height() - 2.0 * PADDING - STATUS_HEIGHT;
    let text_h = galley.size().y.min(visible_h);
    let overflow = galley.size().y - text_h;
    let box_rect = Rect::from_center_size(
        rect.center(),
        Vec2::new(rect.width(), text_h + 2.0 * PADDING + STATUS_HEIGHT),
    );
    painter.rect_filled(
        box_rect,
        CornerRadius::same(16),
        Color32::from_black_alpha(BOX_ALPHA),
    );
    let text_rect = Rect::from_min_size(
        box_rect.left_top() + Vec2::new(PADDING, PADDING),
        Vec2::new(box_rect.width() - 2.0 * PADDING, text_h),
    );
    let clip = painter.with_clip_rect(text_rect);
    clip.galley(
        text_rect.left_top() - Vec2::new(0.0, overflow),
        galley,
        Color32::WHITE,
    );
    // Status line along the bottom edge.
    let status_y = box_rect.max.y - STATUS_HEIGHT / 2.0 - 6.0;
    if let Some((text, is_error)) = status {
        painter.text(
            Pos2::new(box_rect.min.x + PADDING, status_y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(STATUS_FONT_SIZE),
            if *is_error { ERROR_COLOR } else { STATUS_COLOR },
        );
    }
    if busy {
        let spinner = Rect::from_center_size(
            Pos2::new(box_rect.max.x - PADDING - 8.0, status_y),
            Vec2::splat(16.0),
        );
        ui.put(spinner, egui::Spinner::new().size(16.0).color(STATUS_COLOR));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_tracks_changes_and_idle_time() {
        let mut p = PreviewState::default();
        assert!(p.update("Ship it.", 10.0));
        assert!(!p.update("Ship it.", 20.0));
        assert!((p.idle_for(20.0) - 10.0).abs() < f64::EPSILON);
        assert!(p.update("Ship it. Now.", 25.0));
        assert!(p.idle_for(25.5) < 1.0);
    }

    #[test]
    fn auto_mode_opens_on_listen_and_closes_on_stop() {
        let mut p = PreviewState::default();
        assert_eq!(p.auto_transition(false, true), None, "off: never acts");
        assert_eq!(p.auto_transition(false, false), None);
        assert_eq!(
            p.auto_transition(true, true),
            Some(true),
            "listening starts"
        );
        assert_eq!(p.auto_transition(true, true), None, "still listening");
        assert_eq!(
            p.auto_transition(true, false),
            Some(false),
            "listening stops"
        );
        assert_eq!(p.auto_transition(true, false), None);
        // A panel the user opened by hand is not closed by listening ending.
        p.auto_opened = false;
        p.listening_was = true;
        assert_eq!(p.auto_transition(true, false), None);
    }

    #[test]
    fn status_keeps_the_last_toast_until_replaced() {
        let mut p = PreviewState::default();
        p.note_status(None);
        assert!(p.status.is_none());
        p.note_status(Some(("Voice: send", false)));
        p.note_status(None);
        assert_eq!(
            p.status,
            Some(("Voice: send".into(), false)),
            "toast expiry keeps it"
        );
        p.note_status(Some(("Unknown voice command \"banana\"", true)));
        assert!(p.status.as_ref().is_some_and(|(_, e)| *e));
    }
}
