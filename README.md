# Prompt Box

A desktop workbench for composing coding-agent prompts by voice, built in Rust
with [egui](https://docs.rs/egui) / [eframe](https://docs.rs/eframe). Design:
`voice-prompt-workbench-design.md`. Milestone 0 spike and its findings:
`spikes/voice-spike/`.

Current state (Milestone 4, voice commands): microphone capture through a
lock-free ring buffer, whisper.cpp (Metal on macOS) with emulated streaming
partials, dimmed provisional text in the editor, an input level meter,
stall and audio-gap warnings, Copy and Send through a clipboard port that
reports failure, sent-prompt history, autosaved draft, undo/redo, delete
sentence/paragraph, paragraph breaks, voice commands, and toasts.

## Voice commands

Say the trigger word **Zevro** followed by a command, in the same breath or
its own: "…move it into the service layer. Zevro delete sentence."

| Say | Does |
|---|---|
| Zevro delete sentence / scratch that | Delete last sentence |
| Zevro delete paragraph | Delete last paragraph |
| Zevro undo / redo | Undo / redo |
| Zevro new line / new paragraph | Line break / paragraph break |
| Zevro clear | Clear (undoable) |
| Zevro copy | Copy without clearing |
| Zevro send | Copy and clear |
| Zevro stop | Stop listening |

Commands are extracted only from finalized utterances, so each runs exactly
once. While you are still speaking, the command words show in amber inside
the dimmed provisional text and disappear when the utterance finalizes.
Real microphones render the trigger many ways ("Zebro", "Zebra", "Zev
Bro", "zebbro"), so it is matched on a consonant skeleton (b/v merged,
vowels dropped), optionally across two words; "zero" never matches.
Command words tolerate a one-letter slip ("sand" counts as "send"). An
utterance that starts with the trigger is treated as a command only, so a
garbled tail after the command is ignored. Anything after the trigger
that matches nothing is dropped and reported in a toast, never typed into
the prompt. Example command phrases are added to whisper's prompt so the
trigger and grammar are recognized reliably.

To use a different trigger word, set `"trigger": "yourword"` in
`settings.json` (see data directory below) and restart.

## First run

1. `brew install cmake` (whisper.cpp builds via CMake).
2. `cargo run --release` (debug builds run whisper far too slowly).
3. Click **Download base.en (148 MB)** once; it lands in the data dir below.
4. Click **Start listening** (or ⌘L). macOS asks for microphone access the
   first time; the prompt is attributed to the terminal you launched from.
5. Speak. Provisional text appears dimmed and firms up at each pause.
   ⌘↩ copies the prompt and clears the editor.

Dev aids: `PROMPTBOX_AUTOSTART=1` starts listening at launch, and
`PROMPTBOX_FAKE_MIC=/path/to/16k-mono.wav` feeds a WAV through the real
capture path instead of the microphone (the spike's fixtures work). The
**Debug** menu runs scripted dictation without any model.

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
src/ports/             traits the core needs: speech events/engine, clipboard, history
src/adapters/
  audio.rs             cpal capture -> ring buffer -> resample -> 20 ms chunks
  speech/              whisper.cpp engine: VAD, worker per session, partials
  model.rs             model path and background download
  clipboard.rs, persistence.rs, fake_speech.rs
src/app.rs             PromptBoxApp: owns core + adapters + recognizer + mic; runs effects
src/ui.rs              egui drawing and input -> actions (edit diffing, shortcuts)
tests/ui.rs            headless flows via egui_kittest with fake adapters
```

Data lives in the platform data dir (`~/Library/Application Support/promptbox`
on macOS): `history.json` (last 50 sent prompts), `draft.txt` (autosaved
every 500 ms after a change), and `models/ggml-base.en.bin`.

Shortcuts: ⌘L start/stop listening, ⌘↩ Send (copy and clear), ⌘⇧C Copy,
⌘Z / ⌘⇧Z undo and redo (one history covering typing, dictation, and
Send), ⌘⌫ delete last sentence, ⌘⇧⌫ delete last paragraph, ⇧↩ new
paragraph, ⌘⇧K clear. "Last" means the unit ending at or containing the
cursor, which after dictation is what was just said.
The 📌 button pins the window above others (remembered in `settings.json`).
**Dock** shrinks the window to 300×330 and moves it to the next screen
corner on each click (top-right, bottom-right, bottom-left, top-left).
Below about 460 px wide the top bar hides the project picker and Debug menu
so the window can sit small in a corner.

Status line: ○ Idle, ● Listening, ◐ Finishing (stop requested, last words
still arriving), ▲ Degraded (audio gap or no transcript progress for 4 s of
continuous voice; sticky until dismissed or recovered), ✖ Error. The bar
meter next to it is raw microphone level, independent of recognition, so a
flat meter means capture is the problem and a moving meter with no text
means recognition is.

## Testing approach

Everything enters `AppCore::dispatch(action, clock)` and leaves as effects, so
core tests are plain state-transition tests with an explicit clock (no
sleeping). Adapters have their own tests (file store uses a temp dir). UI
tests run the real `eframe::App` headlessly with fake clipboard/history,
find widgets by label, and advance frames with `harness.run_steps(2)` (one
frame to process the click, one to render its result).

`cargo test --release --test whisper -- --ignored` runs the real engine over
a spike fixture (needs the model and `spikes/voice-spike` fixtures).

## Pre-commit hook

`.githooks/pre-commit` checks formatting, runs Clippy, and runs the tests before each commit. It never modifies or stages files. Enable it once per clone:

```sh
git config core.hooksPath .githooks
```
