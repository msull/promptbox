//! Screen geometry for the on-screen overlays. egui reports only the
//! current monitor's size, so on macOS the list of screens comes from
//! `AppKit`'s `NSScreen`, converted to the global logical coordinates that
//! winit and egui use (origin at the primary screen's top-left, y down;
//! Cocoa's origin is the primary screen's bottom-left, y up). Elsewhere the
//! list is empty and overlays fall back to the current monitor.

use egui::Rect;

/// One attached display in global logical points.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// Display name as the system shows it ("DELL U3415W"); stable across
    /// reconnects, unlike the index.
    pub name: String,
    /// Full frame.
    pub frame: Rect,
    /// Frame minus the menu bar and Dock.
    pub visible: Rect,
}

/// Every attached screen, primary first. Empty off macOS or off the main
/// thread.
#[must_use]
pub fn screens() -> Vec<Screen> {
    #[cfg(target_os = "macos")]
    {
        macos::screens()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// The area overlays should use: the screen named `chosen` if attached,
/// else the screen containing the centre of `root` (the main window),
/// else the primary screen, else `fallback` (the current monitor at the
/// origin, from egui's `monitor_size`).
#[must_use]
pub fn overlay_area(screens: &[Screen], chosen: &str, root: Option<Rect>, fallback: Rect) -> Rect {
    let chosen = chosen.trim();
    if !chosen.is_empty()
        && let Some(s) = screens.iter().find(|s| s.name == chosen)
    {
        return s.visible;
    }
    if let Some(root) = root
        && let Some(s) = screens.iter().find(|s| s.frame.contains(root.center()))
    {
        return s.visible;
    }
    screens.first().map_or(fallback, |s| s.visible)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Screen;
    use egui::{Pos2, Rect, Vec2};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    use objc2_foundation::NSRect;

    pub fn screens() -> Vec<Screen> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        let all = NSScreen::screens(mtm);
        // Index 0 is the primary screen; its frame origin is (0, 0) and its
        // height is the reference for flipping the y axis.
        let Some(primary) = all.iter().next() else {
            return Vec::new();
        };
        let h0 = primary.frame().size.height as f32;
        let convert = |r: NSRect| {
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
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Vec2};

    fn screen(name: &str, x: f32, y: f32, w: f32, h: f32) -> Screen {
        let frame = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        Screen {
            name: name.into(),
            frame,
            visible: frame.shrink2(Vec2::new(0.0, 20.0)),
        }
    }

    #[test]
    fn overlay_area_prefers_chosen_then_root_screen_then_primary() {
        let laptop = screen("Built-in", 0.0, 0.0, 1496.0, 967.0);
        let dell = screen("DELL", -3440.0, -797.0, 3440.0, 1440.0);
        let screens = vec![laptop.clone(), dell.clone()];
        let fallback = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0));
        let on_dell = Some(Rect::from_min_size(
            Pos2::new(-2100.0, -500.0),
            Vec2::new(760.0, 328.0),
        ));
        let on_laptop = Some(Rect::from_min_size(
            Pos2::new(100.0, 100.0),
            Vec2::new(760.0, 328.0),
        ));
        assert_eq!(
            overlay_area(&screens, "DELL", on_laptop, fallback),
            dell.visible
        );
        assert_eq!(overlay_area(&screens, "", on_dell, fallback), dell.visible);
        assert_eq!(
            overlay_area(&screens, "", on_laptop, fallback),
            laptop.visible
        );
        assert_eq!(
            overlay_area(&screens, "Unplugged", on_dell, fallback),
            dell.visible,
            "an absent chosen screen follows the window"
        );
        assert_eq!(overlay_area(&screens, "", None, fallback), laptop.visible);
        assert_eq!(overlay_area(&[], "DELL", on_dell, fallback), fallback);
    }
}
