//! Whole-prompt preview: "Zevro preview" opens a translucent, click-through
//! panel in the middle of the screen showing the entire prompt (live text
//! dimmed), so it can be skimmed before sending without looking away from
//! the app being worked in. It stays until "preview" is said again, the
//! prompt is sent or cleared, or the prompt has not changed for a while.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontId, Pos2, Vec2, ViewportBuilder, ViewportId};

use crate::app::PromptBoxApp;

/// Seconds without a change to the prompt before the preview closes.
const IDLE_SECS: f64 = 60.0;
/// Fraction of the monitor the panel may use.
const WIDTH_FRACTION: f32 = 0.6;
const HEIGHT_FRACTION: f32 = 0.55;
const MAX_WIDTH: f32 = 1000.0;
const FONT_SIZE: f32 = 22.0;
const PADDING: f32 = 28.0;
const BOX_ALPHA: u8 = 215;
const LIVE_COLOR: Color32 = Color32::from_rgb(255, 200, 110);

/// What the preview last showed and when it last changed (egui time).
#[derive(Debug, Default)]
pub struct PreviewState {
    rendered: String,
    changed_at: f64,
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
}

/// Draws the preview viewport while the core says it is open. Closes it
/// when the prompt is empty or has been idle for [`IDLE_SECS`].
pub fn draw(app: &mut PromptBoxApp, ctx: &egui::Context) {
    if !app.core().preview_open() {
        app.preview.rendered.clear();
        return;
    }
    let now = ctx.input(|i| i.time);
    let doc = app.core().doc();
    let rendered = doc.rendered();
    let live = doc.provisional_range();
    let first_show = app.preview.rendered.is_empty();
    app.preview.update(&rendered, now);
    if rendered.trim().is_empty() || (!first_show && app.preview.idle_for(now) > IDLE_SECS) {
        app.set_preview_open(false);
        return;
    }
    // Wake up to close on idle even if nothing else repaints.
    ctx.request_repaint_after(std::time::Duration::from_secs(1));

    let monitor = ctx
        .input(|i| i.viewport().monitor_size)
        .unwrap_or(Vec2::new(1920.0, 1080.0));
    let size = Vec2::new(
        (monitor.x * WIDTH_FRACTION).min(MAX_WIDTH),
        monitor.y * HEIGHT_FRACTION,
    );
    let pos = Pos2::new(
        (monitor.x - size.x) / 2.0,
        (monitor.y - size.y) / 2.0 - 40.0,
    );

    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("prompt-preview"),
        ViewportBuilder::default()
            .with_title("Prompt preview")
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_active(false)
            .with_taskbar(false)
            .with_resizable(false)
            .with_inner_size(size)
            .with_position(pos),
        |ui, _class| paint(ui, &rendered, live.clone()),
    );
}

/// Paints the prompt in a rounded translucent panel, sized to the text up
/// to the panel's limits and clipped to show the tail when it overflows.
fn paint(ui: &mut egui::Ui, rendered: &str, live: Option<std::ops::Range<usize>>) {
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
        None => job.append(rendered, 0.0, format(Color32::WHITE)),
    }
    let galley = painter.layout_job(job);
    let visible_h = rect.height() - 2.0 * PADDING;
    let text_h = galley.size().y.min(visible_h);
    let overflow = galley.size().y - text_h;
    let box_rect = egui::Rect::from_center_size(
        rect.center(),
        Vec2::new(rect.width(), text_h + 2.0 * PADDING),
    );
    painter.rect_filled(
        box_rect,
        CornerRadius::same(16),
        Color32::from_black_alpha(BOX_ALPHA),
    );
    let clip = painter.with_clip_rect(box_rect.shrink(PADDING));
    clip.galley(
        box_rect.left_top() + Vec2::new(PADDING, PADDING - overflow),
        galley,
        Color32::WHITE,
    );
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
}
