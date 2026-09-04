//! Dock badge: a small pill on the app's Dock icon while the microphone
//! is live, so recording status is visible without the window.
//!
//! macOS only; elsewhere the badge is a no-op.

/// Shows or clears the Dock badge. Must be called from the main thread
/// (the eframe frame loop is fine).
pub fn set_recording_badge(on: bool) {
    #[cfg(target_os = "macos")]
    macos::set_badge(if on { Some("REC") } else { None });
    #[cfg(not(target_os = "macos"))]
    let _ = on;
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub fn set_badge(label: Option<&str>) {
        let Some(mtm) = MainThreadMarker::new() else {
            log::warn!("dock badge requested off the main thread; ignoring");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let label = label.map(NSString::from_str);
        app.dockTile().setBadgeLabel(label.as_deref());
    }
}
