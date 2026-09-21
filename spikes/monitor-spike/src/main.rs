//! Monitor spike. egui reports only the current monitor's size, and its
//! `with_monitor` makes a window fullscreen, so screen geometry comes from
//! `AppKit`'s `NSScreen`. Questions: does converting Cocoa frames (origin
//! bottom-left of the primary screen, y up) to winit's global logical
//! coordinates (origin top-left of the primary screen, y down) agree with
//! the `outer_rect` egui reports for the root window; can a child viewport
//! be positioned on the secondary monitor with `with_position`; and which
//! screen the root window is on.
//!
//! Run `cargo run --release`. The control window lists screens, shows its
//! own outer rect and which screen contains it, and opens a caption bar at
//! the bottom of every screen (or only the chosen one).

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Vec2, ViewportBuilder, ViewportId};

#[derive(Debug, Clone, PartialEq)]
struct Screen {
    name: String,
    /// Full frame in winit global logical points (top-left origin).
    frame: Rect,
    /// Frame minus menu bar and Dock.
    visible: Rect,
    scale: f32,
}

#[cfg(target_os = "macos")]
fn screens() -> Vec<Screen> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let all = NSScreen::screens(mtm);
    // Index 0 is the primary screen; its frame origin is (0, 0) and its
    // height is the y axis flip reference.
    let Some(primary) = all.iter().next() else {
        return Vec::new();
    };
    let h0 = primary.frame().size.height as f32;
    let convert = |r: objc2_foundation::NSRect| {
        let (x, y, w, h) = (
            r.origin.x as f32,
            r.origin.y as f32,
            r.size.width as f32,
            r.size.height as f32,
        );
        Rect::from_min_size(Pos2::new(x, h0 - (y + h)), Vec2::new(w, h))
    };
    all.iter()
        .map(|s| Screen {
            name: s.localizedName().to_string(),
            frame: convert(s.frame()),
            visible: convert(s.visibleFrame()),
            scale: s.backingScaleFactor() as f32,
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn screens() -> Vec<Screen> {
    Vec::new()
}

const BAR_SIZE: Vec2 = Vec2::new(700.0, 90.0);

struct Spike {
    only: Option<usize>,
    logged: bool,
}

impl eframe::App for Spike {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screens = screens();
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let on = outer.and_then(|o| screens.iter().position(|s| s.frame.contains(o.center())));
        if !self.logged {
            self.logged = true;
            eprintln!("outer_rect {outer:?} on {on:?}");
            for (i, s) in screens.iter().enumerate() {
                eprintln!("[{i}] {s:?}");
            }
        }

        ui.heading("Monitor spike");
        ui.label(format!("root outer_rect (egui): {outer:?}"));
        ui.label(format!("root window is on screen: {on:?}"));
        for (i, s) in screens.iter().enumerate() {
            ui.label(format!(
                "[{i}] {}  frame {:?}  visible {:?}  scale {}",
                s.name, s.frame, s.visible, s.scale
            ));
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.only, None, "captions on every screen");
            for i in 0..screens.len() {
                ui.selectable_value(&mut self.only, Some(i), format!("only [{i}]"));
            }
        });

        for (i, s) in screens.iter().enumerate() {
            if self.only.is_some_and(|o| o != i) {
                continue;
            }
            let pos = Pos2::new(
                s.visible.center().x - BAR_SIZE.x / 2.0,
                s.visible.max.y - BAR_SIZE.y - 40.0,
            );
            let text = format!("caption on [{i}] {}", s.name);
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of(("caption", i)),
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
                    let p = ui.painter();
                    let r = ui.max_rect();
                    p.rect_filled(r, CornerRadius::same(12), Color32::from_black_alpha(180));
                    p.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        &text,
                        FontId::proportional(28.0),
                        Color32::WHITE,
                    );
                },
            );
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([760.0, 300.0])
            .with_transparent(true),
        ..Default::default()
    };
    eframe::run_native(
        "Monitor spike",
        options,
        Box::new(|_cc| Ok(Box::new(Spike { only: None, logged: false }))),
    )
}
