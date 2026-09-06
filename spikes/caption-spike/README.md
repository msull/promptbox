# Caption overlay spike

Feasibility check for showing the live partial as TV-style closed captions
on the main monitor, outside the Prompt Box window, so you can keep your
eyes on the app you are working in.

Run with `cargo run --release` (add `CAPTION_AUTOPLAY=1` to stream the
canned passage at launch). "Play script" feeds words in like whisper
partials; the text field lets you type your own. Sliders adjust font size
and box opacity.

## Findings (macOS, eframe/egui 0.36, wgpu)

- A second `show_viewport_immediate` window with `with_decorations(false)`,
  `with_transparent(true)`, `with_always_on_top()`, `with_mouse_passthrough(true)`
  and `with_active(false)` gives a borderless, click-through, floating caption
  bar that does not take focus. Positioned with `with_position` from
  `viewport().monitor_size`, bottom-centre with an inset above the Dock.
- Transparency support is decided once from the **root** window's
  `ViewportBuilder`, so the main window must also be created with
  `with_transparent(true)`; `App::clear_color` must return zero alpha. The
  main window's panels then need an opaque fill (Prompt Box already paints
  its own frames).
- Long passages: the bar clips to the last few lines so the tail stays
  visible; text fades out after a short hold once nothing new arrives.
- Repaints are driven by `request_repaint_after` from the root; the caption
  viewport is rebuilt every frame of the root, which is cheap.

## Open questions for integration

- Which monitor: the one the main window is on is what `monitor_size`
  describes; a "main display" setting needs winit's monitor list.
- Show only the provisional span, or committed text of the current
  utterance too? The spike shows one growing string.
- Whether to also show voice-command feedback (toasts) in the bar.
