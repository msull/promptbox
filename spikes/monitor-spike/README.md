# Monitor spike

Feasibility check for placing the caption and preview overlays on a chosen
monitor. Run `cargo run --release`; the control window lists screens, its own
`outer_rect`, which screen contains it, and opens a caption bar at the bottom
of every screen (or one chosen screen).

## Findings (macOS, eframe/egui 0.36)

- egui exposes only the current monitor's *size* (`ViewportInfo::monitor_size`),
  no origin and no list; `ViewportBuilder::with_monitor` makes the window
  fullscreen. Positioning from `monitor_size` alone assumes the primary
  screen's origin, which is why overlays landed on the wrong monitor.
- `NSScreen::screens()` (objc2-app-kit, feature `NSScreen`, all safe calls)
  gives every screen's `frame`, `visibleFrame` (minus menu bar and Dock),
  `localizedName` and scale. Index 0 is the primary screen.
- Cocoa frames have their origin at the primary screen's bottom-left with y
  up; winit/egui use the primary screen's top-left with y down. Convert with
  `y' = primary_height - (y + height)`. On this machine the Dell to the left
  came out as `[-3440,-797]..[0,643]` and egui reported the root window at
  `[-2100,-500]..[-1340,-172]` while it sat on the Dell: the systems agree.
- `with_position` in those global coordinates puts a child viewport on the
  secondary monitor correctly; always-on-top and mouse passthrough work there.
- "Which screen is Prompt Box on" = the screen whose frame contains the
  centre of the root viewport's `outer_rect`.
- Screen names are stable across reconnects; indices are not, so a setting
  should store the name.
