# Prompt Box

A desktop GUI app built in Rust with [egui](https://docs.rs/egui) / [eframe](https://docs.rs/eframe).

Requires Rust 1.95 or newer.

## Commands

```sh
cargo run --locked                                   # launch the app
cargo test --locked                                  # unit tests + headless UI tests
cargo clippy --locked --all-targets -- -D warnings   # lint
cargo fmt --all                                      # format
```

## Layout

- `src/main.rs` – thin launcher, opens the native window.
- `src/lib.rs` – library root, re-exports the app so tests can use it.
- `src/app.rs` – `PromptBoxApp`: state, pure logic, and `eframe::App::ui` (rendering). Unit tests live at the bottom.
- `tests/ui.rs` – UI tests that run the real app headlessly with `egui_kittest` and interact via widget labels.

## Testing approach

Keep logic in plain methods on `PromptBoxApp` and unit test them directly. Keep `ui` limited to drawing and wiring events to those methods. UI tests find widgets by label (`harness.get_by_label("Greet").click()`), call `harness.run()` to advance a frame, then assert on labels or `harness.state()`.

## Pre-commit hook

`.githooks/pre-commit` checks formatting, runs Clippy, and runs the tests before each commit. It never modifies or stages files. Enable it once per clone:

```sh
git config core.hooksPath .githooks
```
