# Prompt Box

A desktop workbench for composing coding-agent prompts by voice, built in Rust
with [egui](https://docs.rs/egui) / [eframe](https://docs.rs/eframe). Design:
`voice-prompt-workbench-design.md`. Milestone 0 spike and its findings:
`spikes/voice-spike/`.

Current state (Milestone 1, application shell): editable prompt area with
dimmed provisional text, idle/listening/degraded status driven by scripted
demo dictation, Copy and Send through a clipboard port that reports failure,
sent-prompt history, autosaved draft, undo, and toasts. No microphone yet.

Requires Rust 1.95 or newer.

## Commands

```sh
cargo run --locked                                   # launch the app
cargo test --locked                                  # unit tests + headless UI tests
cargo clippy --locked --all-targets -- -D warnings   # lint
cargo fmt --all                                      # format
```

## Layout

```
src/main.rs            thin launcher
src/lib.rs             module tree
src/core/              deterministic, egui-free
  action.rs            AppAction, Effect, AppCore::dispatch (the state machine)
  document.rs          committed text + one provisional span + edit history
  project.rs           placeholder projects
src/ports/             traits the core needs: speech events, clipboard, history
src/adapters/          arboard clipboard, JSON file store, scripted fake dictation
src/app.rs             PromptBoxApp: owns core + adapters, runs effects, pumps demo
src/ui.rs              egui drawing and input -> actions (edit diffing, shortcuts)
tests/ui.rs            headless flows via egui_kittest with fake adapters
```

Data lives in the platform data dir (`~/Library/Application Support/promptbox`
on macOS): `history.json` (last 50 sent prompts) and `draft.txt` (autosaved
every 500 ms after a change).

Shortcuts: ⌘↩ Send (copy and clear), ⌘⇧C Copy (keep text). "Demo dictation"
streams scripted speech events; "Demo with gap" also injects an audio gap to
show the sticky degraded state.

## Testing approach

Everything enters `AppCore::dispatch(action, clock)` and leaves as effects, so
core tests are plain state-transition tests with an explicit clock (no
sleeping). Adapters have their own tests (file store uses a temp dir). UI
tests run the real `eframe::App` headlessly with fake clipboard/history,
find widgets by label, and advance frames with `harness.run_steps(2)` (one
frame to process the click, one to render its result).

## Pre-commit hook

`.githooks/pre-commit` checks formatting, runs Clippy, and runs the tests before each commit. It never modifies or stages files. Enable it once per clone:

```sh
git config core.hooksPath .githooks
```
