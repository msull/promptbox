//! Caption overlay spike. A small control window drives a second,
//! click-through, always-on-top, transparent viewport pinned to the
//! bottom of the primary monitor, styled like TV closed captions.
//!
//! Questions this answers: can eframe 0.36 open such a window on macOS
//! without stealing focus from the app you are working in; does mouse
//! passthrough work; can we position it on the monitor; how does the
//! text look; does it fade cleanly when nothing is coming in.
//!
//! Run: `cargo run` in this directory. "Play script" streams a canned
//! passage word by word like whisper partials; the text field lets you
//! type your own. Click into another app and watch the caption update
//! without your focus moving.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Vec2, ViewportBuilder, ViewportId};

const SCRIPT: &str = "So the change I want is that the trigger word is only detected at the \
start of an utterance. I'll never inject a command in the middle of a phrase, \
so a mis-heard zebra should just be dictation. Zevro send.";

/// Seconds of silence before the caption starts to fade.
const HOLD: f32 = 2.5;
/// Fade-out duration.
const FADE: f32 = 0.8;
/// Caption bar size and inset from the bottom of the monitor.
const BAR_SIZE: Vec2 = Vec2::new(900.0, 120.0);
const BOTTOM_INSET: f32 = 80.0;

struct Spike {
    text: String,
    typed: String,
    last_change: Instant,
    playing: Option<(usize, Instant)>,
    show: bool,
    font_size: f32,
    opacity: u8,
}

impl Spike {
    fn new() -> Self {
        // `CAPTION_AUTOPLAY=1` starts the script at launch (handy for screenshots).
        let autoplay = std::env::var_os("CAPTION_AUTOPLAY").is_some();
        Self {
            text: String::new(),
            typed: String::new(),
            last_change: Instant::now(),
            playing: autoplay.then(|| (0, Instant::now() + Duration::from_secs(2))),
            show: true,
            font_size: 26.0,
            opacity: 110,
        }
    }

    fn set_text(&mut self, t: String) {
        if t != self.text {
            self.text = t;
            self.last_change = Instant::now();
        }
    }

    fn advance_script(&mut self) {
        let Some((n, started)) = self.playing else {
            return;
        };
        let words: Vec<&str> = SCRIPT.split_whitespace().collect();
        let due = (started.elapsed().as_secs_f32() / 0.18) as usize;
        let n = due.min(words.len()).max(n);
        self.set_text(words[..n].join(" "));
        if n >= words.len() {
            self.playing = None;
        } else {
            self.playing = Some((n, started));
        }
    }

    /// 0..=1 caption alpha for the current moment.
    fn alpha(&self) -> f32 {
        let idle = self.last_change.elapsed().as_secs_f32();
        if self.text.is_empty() {
            0.0
        } else if idle < HOLD {
            1.0
        } else {
            (1.0 - (idle - HOLD) / FADE).clamp(0.0, 1.0)
        }
    }
}

impl eframe::App for Spike {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.advance_script();
        ctx.request_repaint_after(Duration::from_millis(50));

        {
            ui.heading("Caption overlay spike");
            ui.checkbox(&mut self.show, "Show caption overlay");
            ui.add(egui::Slider::new(&mut self.font_size, 18.0..=48.0).text("font size"));
            ui.add(egui::Slider::new(&mut self.opacity, 0..=255).text("box opacity"));
            ui.horizontal(|ui| {
                if ui.button("Play script").clicked() {
                    self.playing = Some((0, Instant::now()));
                }
                if ui.button("Clear").clicked() {
                    self.playing = None;
                    self.set_text(String::new());
                }
            });
            ui.label("Or type a live partial:");
            if ui.text_edit_singleline(&mut self.typed).changed() {
                let t = self.typed.clone();
                self.set_text(t);
            }
            ui.separator();
            ui.label(format!("caption: {:?}", self.text));
            ui.label(format!("alpha: {:.2}", self.alpha()));
            ui.label(
                "Now click into another app; the caption keeps updating without taking focus.",
            );
        }

        if !self.show {
            return;
        }

        // Bottom-centre of the monitor the control window is on.
        let monitor = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(Vec2::new(1920.0, 1080.0));
        let pos = Pos2::new(
            (monitor.x - BAR_SIZE.x) / 2.0,
            monitor.y - BAR_SIZE.y - BOTTOM_INSET,
        );

        let alpha = self.alpha();
        let text = self.text.clone();
        let font_size = self.font_size;
        let opacity = self.opacity;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("caption-overlay"),
            ViewportBuilder::default()
                .with_title("Caption")
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
            |ui, _class| {
                if alpha <= 0.0 {
                    return;
                }
                let painter = ui.painter();
                let rect = ui.max_rect();
                let font = FontId::proportional(font_size);
                let wrap = rect.width() - 48.0;
                let galley = painter.layout(
                    text.clone(),
                    font,
                    Color32::WHITE.gamma_multiply(alpha),
                    wrap,
                );
                // Show the tail if the passage is longer than the bar.
                let visible_h = rect.height() - 24.0;
                let overflow = (galley.size().y - visible_h).max(0.0);
                let box_rect = egui::Rect::from_center_size(
                    rect.center(),
                    Vec2::new(
                        galley.size().x + 48.0,
                        galley.size().y.min(visible_h) + 24.0,
                    ),
                );
                painter.rect_filled(
                    box_rect,
                    CornerRadius::same(12),
                    Color32::from_black_alpha((f32::from(opacity) * alpha) as u8),
                );
                let clip = painter.with_clip_rect(box_rect.shrink(12.0));
                let anchor = Align2::LEFT_TOP.anchor_size(
                    box_rect.left_top() + Vec2::new(24.0, 12.0 - overflow),
                    galley.size(),
                );
                clip.galley(anchor.min, galley, Color32::WHITE);
            },
        );
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        // Transparency support is chosen once from the root window, so the
        // root must be transparent for the caption viewport to be.
        viewport: ViewportBuilder::default()
            .with_inner_size([480.0, 320.0])
            .with_transparent(true),
        ..Default::default()
    };
    eframe::run_native(
        "Caption spike",
        options,
        Box::new(|_cc| Ok(Box::new(Spike::new()))),
    )
}
