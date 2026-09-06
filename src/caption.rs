//! Closed-caption overlay: the live utterance in a translucent bar at the
//! bottom of the screen, in its own borderless, click-through, always-on-top
//! viewport, so the text can be followed while another app has focus.
//!
//! Transparency is decided once from the root window (`src/main.rs` creates
//! it transparent and `clear_color` is fully transparent), so the caption
//! window can composite over whatever is beneath it. The caption holds for
//! a moment after the text stops changing, then fades out.

use std::collections::VecDeque;

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Vec2, ViewportBuilder, ViewportId};

use crate::app::PromptBoxApp;
use crate::core::document::CaptionParts;

/// Seconds the caption stays fully visible after its text last changed.
const HOLD_SECS: f64 = 2.5;
/// Seconds the fade-out takes after the hold.
const FADE_SECS: f64 = 0.8;
/// How long a finalized sentence stays in the caption while dictation
/// continues, so it can still be read after the next utterance starts.
const LINGER_SECS: f64 = 8.0;
/// At most this many finalized sentences ahead of the live text.
const MAX_RECENT: usize = 2;
/// Caption window size and its inset from the bottom of the monitor.
const BAR_SIZE: Vec2 = Vec2::new(900.0, 120.0);
const BOTTOM_INSET: f32 = 80.0;
const FONT_SIZE: f32 = 28.0;
const PADDING: f32 = 24.0;
const BOX_ALPHA: f32 = 170.0;

/// What the overlay is showing: recently finalized sentences (with when
/// each arrived, egui time), the live text, and when anything was last
/// added or changed. Sentences expiring is not a change: it must not
/// restart the hold.
#[derive(Debug, Default)]
pub struct CaptionState {
    recent: VecDeque<(String, f64)>,
    live: String,
    changed_at: f64,
    /// Whether anything at all has been shown since the last clear.
    showing: bool,
}

impl CaptionState {
    /// Feeds the document's caption pieces for this frame. Returns whether
    /// something new was added or the live text changed.
    fn update(&mut self, parts: &CaptionParts, now: f64) -> bool {
        let mut changed = false;
        if !parts.committed.is_empty()
            && self
                .recent
                .back()
                .is_none_or(|(t, _)| *t != parts.committed)
        {
            self.recent.push_back((parts.committed.clone(), now));
            while self.recent.len() > MAX_RECENT {
                self.recent.pop_front();
            }
            changed = true;
        }
        if self.live != parts.live {
            self.live.clone_from(&parts.live);
            changed = true;
        }
        self.recent.retain(|(_, at)| now - *at < LINGER_SECS);
        if changed {
            self.changed_at = now;
            self.showing = true;
        }
        changed
    }

    /// Forgets everything (captions turned off, or listening stopped).
    fn clear(&mut self) {
        self.recent.clear();
        self.live.clear();
        self.showing = false;
    }

    fn text(&self) -> String {
        let mut parts: Vec<&str> = self.recent.iter().map(|(t, _)| t.as_str()).collect();
        if !self.live.is_empty() {
            parts.push(&self.live);
        }
        parts.join(" ")
    }

    /// Opacity in `0..=1` for `now`; zero once the fade has finished.
    fn alpha(&self, now: f64) -> f32 {
        if !self.showing || self.text().is_empty() {
            return 0.0;
        }
        let idle = now - self.changed_at;
        if idle < HOLD_SECS {
            1.0
        } else {
            (1.0 - (idle - HOLD_SECS) / FADE_SECS).clamp(0.0, 1.0) as f32
        }
    }
}

/// Updates the caption from the document and draws the overlay viewport
/// while there is something to show. Call once per frame from the root.
pub fn draw(app: &mut PromptBoxApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    if app.captions_enabled() && (app.is_live() || app.is_demo_running()) {
        let parts = app.core().doc().caption_parts();
        app.caption.update(&parts, now);
    } else {
        app.caption.clear();
    }
    let alpha = app.caption.alpha(now);
    if alpha <= 0.0 {
        return;
    }
    // Keep animating the fade even when nothing else repaints.
    ctx.request_repaint_after(std::time::Duration::from_millis(50));

    let monitor = ctx
        .input(|i| i.viewport().monitor_size)
        .unwrap_or(Vec2::new(1920.0, 1080.0));
    let pos = Pos2::new(
        (monitor.x - BAR_SIZE.x) / 2.0,
        monitor.y - BAR_SIZE.y - BOTTOM_INSET,
    );
    let text = app.caption.text();

    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("caption-overlay"),
        ViewportBuilder::default()
            .with_title("Captions")
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_active(false)
            .with_taskbar(false)
            .with_resizable(false)
            .with_inner_size(BAR_SIZE)
            .with_position(pos),
        |ui, _class| paint(ui, &text, alpha),
    );
}

/// Paints the text in a rounded translucent box, clipped so the tail of a
/// long utterance stays visible.
fn paint(ui: &mut egui::Ui, text: &str, alpha: f32) {
    let painter = ui.painter();
    let rect = ui.max_rect();
    let galley = painter.layout(
        text.to_owned(),
        FontId::proportional(FONT_SIZE),
        Color32::WHITE.gamma_multiply(alpha),
        rect.width() - 2.0 * PADDING,
    );
    let visible_h = rect.height() - PADDING;
    let overflow = (galley.size().y - visible_h).max(0.0);
    let box_rect = egui::Rect::from_center_size(
        rect.center(),
        Vec2::new(
            galley.size().x + 2.0 * PADDING,
            galley.size().y.min(visible_h) + PADDING,
        ),
    );
    painter.rect_filled(
        box_rect,
        CornerRadius::same(12),
        Color32::from_black_alpha((BOX_ALPHA * alpha) as u8),
    );
    let clip = painter.with_clip_rect(box_rect.shrink(PADDING / 2.0));
    let anchor = Align2::LEFT_TOP.anchor_size(
        box_rect.left_top() + Vec2::new(PADDING, PADDING / 2.0 - overflow),
        galley.size(),
    );
    clip.galley(anchor.min, galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(c: &str, l: &str) -> CaptionParts {
        CaptionParts {
            committed: c.into(),
            live: l.into(),
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact 0 and 1 are the intended clamps
    fn caption_holds_then_fades_and_hides_when_cleared() {
        let mut s = CaptionState::default();
        assert_eq!(s.alpha(0.0), 0.0, "nothing to show");
        assert!(s.update(&parts("", "hello"), 10.0));
        assert!(
            !s.update(&parts("", "hello"), 11.0),
            "unchanged is not a change"
        );
        assert_eq!(s.text(), "hello");
        assert_eq!(s.alpha(10.0 + HOLD_SECS - 0.1), 1.0);
        let mid = s.alpha(10.0 + HOLD_SECS + FADE_SECS / 2.0);
        assert!(mid > 0.4 && mid < 0.6, "{mid}");
        assert_eq!(s.alpha(10.0 + HOLD_SECS + FADE_SECS + 1.0), 0.0);
        assert!(
            s.update(&parts("", "hello world"), 20.0),
            "new text restarts the hold"
        );
        assert_eq!(s.alpha(20.5), 1.0);
        s.clear();
        assert_eq!(s.alpha(30.0), 0.0, "cleared hides immediately");
    }

    #[test]
    #[allow(clippy::float_cmp)] // changed_at is assigned, never computed
    fn finalized_sentences_linger_while_dictation_continues() {
        let mut s = CaptionState::default();
        s.update(&parts("", "first po"), 0.0);
        // Final lands: the sentence is now committed, nothing live.
        assert!(s.update(&parts("First point.", ""), 1.0));
        assert_eq!(s.text(), "First point.");
        // Next utterance starts; the same committed sentence is not re-added.
        assert!(s.update(&parts("First point.", "second"), 2.0));
        assert_eq!(s.text(), "First point. second");
        // It finalizes quickly: both sentences stay visible.
        s.update(&parts("Second point.", ""), 3.0);
        s.update(&parts("Second point.", "third"), 3.5);
        assert_eq!(s.text(), "First point. Second point. third");
        // A third final pushes the oldest out (at most MAX_RECENT).
        s.update(&parts("Third point.", ""), 4.0);
        assert_eq!(s.text(), "Second point. Third point.");
        // Expiry drops old sentences without restarting the hold.
        let changed = s.update(&parts("Third point.", ""), 3.0 + LINGER_SECS + 0.1);
        assert!(!changed);
        assert_eq!(s.text(), "Third point.");
        assert_eq!(s.changed_at, 4.0);
        s.update(&parts("Third point.", ""), 4.0 + LINGER_SECS + 0.1);
        assert_eq!(s.text(), "", "everything expired");
    }
}
