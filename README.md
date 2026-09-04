# Prompt Box

A desktop workbench for composing coding-agent prompts by voice. Speak, watch
the words appear, tidy them by hand, by voice command, or with an AI pass,
then send the prompt to the clipboard and start the next one.

Built in Rust with [egui](https://docs.rs/egui) / [eframe](https://docs.rs/eframe)
and local speech recognition via whisper.cpp. macOS on Apple Silicon is the
first supported platform. The design is in `voice-prompt-workbench-design.md`;
the feasibility spike and its measurements are in `spikes/voice-spike/`.

## First run

1. `brew install cmake` (whisper.cpp builds via CMake). Rust 1.95 or newer.
2. `cargo run --release` (debug builds run whisper far too slowly).
3. Click **Download base.en (148 MB)** once. The model lands in the data
   directory listed under [Settings and data](#settings-and-data).
4. Click **Start listening** or press ⌘L. macOS asks for microphone access the
   first time; the prompt is attributed to the terminal you launched from.
5. Speak. Text appears dimmed while provisional and firms up at each pause.
   ⌘Return copies the prompt to the clipboard and clears the editor.

## Using it

### Editor

The prompt area is an ordinary text box. Type, click, select, and dictate in
any order; dictation inserts at the cursor and the cursor follows what you
say. Provisional text (the utterance still being recognized) is dimmed; the
recognizer may revise earlier words in it until the utterance finalizes.

Copy puts the prompt on the clipboard and keeps it. Send copies and clears,
but only after both the clipboard write and the history save succeed; if
either fails the prompt stays and a toast says why. The clear after Send is
undoable.

### Sending into another app

When another app is in front, Send also pastes the prompt into it (⌘V) and
presses Return, so "Zevro send" delivers a finished prompt straight into a
chat box without touching the keyboard. It pastes rather than types: a
multi-line prompt typed key by key would submit at every newline. From
Prompt Box's own window, Send only copies, since the keystrokes would land
back in Prompt Box.

This needs the Accessibility permission (System Settings → Privacy &
Security → Accessibility) for the process that launched Prompt Box, the
same as the microphone. Settings shows whether it is granted and can
request it. If pasting fails, the prompt is kept and a toast says why; the
clipboard still holds it. Both the paste and the trailing Return can be
switched off in Settings.

### Shortcuts

| Keys | Action |
|---|---|
| ⌘L | Start / stop listening |
| ⌘Return | Send (copy and clear) |
| ⌘⇧C | Copy without clearing |
| ⌘Z / ⌘⇧Z | Undo / redo |
| ⌘⌫ | Delete last sentence |
| ⌘⇧⌫ | Delete last paragraph |
| ⇧Return | New paragraph |
| ⌘⇧K | Clear (undoable) |

Undo is one history covering typing, dictation, voice commands, AI rewrites,
and Send. "Last sentence" means the sentence ending at or containing the
cursor, which right after dictation is what you just said.

### Status line

○ Idle · ● Listening · ◐ Finishing (stop requested, last words still
arriving) · ▲ Degraded · × Error.

Degraded means an audio gap was detected or the recognizer made no progress
during four seconds of continuous voice; it stays until dismissed or until
transcription resumes. The bar meter next to the status is raw microphone
level, independent of recognition: a flat meter means capture is the
problem, a moving meter with no text means recognition is.

### Window

**Pin** keeps the window above others. **Dock** shrinks it to 300×330 and moves
it to the next screen corner on each click (top-right, bottom-right,
bottom-left, top-left). Below about 460 px wide the top bar hides the
project picker and Debug menu so the window can sit small in a corner.

## Voice commands

Say the trigger word **Zevro** followed by a command, at the end of a
sentence or on its own: "…move it into the service layer. Zevro delete
sentence." The **Commands** button shows this list in the app.

| Say | Does |
|---|---|
| Zevro delete sentence / scratch that | Delete last sentence |
| Zevro delete paragraph / DP | Delete last paragraph |
| Zevro undo / redo | Undo / redo |
| Zevro new line / new paragraph | Line break / paragraph break at the cursor |
| Zevro new line last / new paragraph last | Move the last sentence to its own line / paragraph |
| Zevro clear | Clear (undoable) |
| Zevro copy | Copy without clearing |
| Zevro send | Copy and clear |
| Zevro stop | Stop listening |
| Zevro clean up | AI clean-up of the whole prompt (undoable) |
| Zevro enhance … confirm | Dictate an AI instruction (see below) |
| Zevro tool … confirm | Dictate a request for a registered tool (see Tools) |
| Zevro … abort | Cancel the command you started saying |

Commands are extracted only from finalized utterances, so each runs exactly
once. While you are still speaking, the command words show in amber inside
the provisional text and disappear when the utterance finalizes.

Recognition of the trigger is deliberately loose. Real microphones render it
as "Zebro", "Zebra", "Zev Bro", or "zebbro", so it is matched on a consonant
skeleton (b/v merged, vowels dropped), optionally across two words; "zero"
never matches. Command words tolerate a one-letter slip ("sand" counts as
"send"). An utterance that starts with the trigger is a command only, so a
garbled tail after the command is ignored. Anything after the trigger that
matches nothing is dropped and reported in a toast, never typed into the
prompt. Example command phrases are added to whisper's prompt so the trigger
and grammar are recognized reliably. The trigger word can be changed in
Settings.

## Projects

The **Project** picker in the top bar chooses the context for what you are
dictating; **Edit** next to it (or **Projects…** in Settings when the window is
narrow) opens the editor. Each project has, one entry per line:

- **Vocabulary**: names and jargon the recognizer is primed with.
- **Corrections**: `heard words => Written Form`. Applied in order to every
  newly finalized utterance, matching whole words in any case with any
  separators between them ("you never sheets", "You Never, Sheets" →
  "Univer Sheets"). Text already in the editor is never touched again, so
  a rule cannot fight your manual edits.
- **Glossary**: `Term: what it means`, given to the AI with every rewrite.
- **AI context**: freeform notes on the project and what rewrites must keep.

Glossary terms and correction targets are also fed to the recognizer.
Projects are saved to `projects.json`; the selected project is remembered
across restarts and recorded with each sent prompt.

## AI rewrite

Two explicit, user-requested transformations of the whole prompt. The AI
never touches text while you are dictating.

- **Clean up** (bottom bar) fixes recognition errors, punctuation, and
  capitalization and removes filler words and false starts, keeping your
  wording and order.
- The **AI box** under the prompt takes an instruction ("make it concise",
  "turn this into a bulleted list"). Press Enter or Ask; the instruction and the
  full prompt go to the model and the reply replaces the prompt.

- **Zevro enhance** dictates an instruction instead of typing it. After
  "enhance", everything you say goes into the AI box, shown in blue, not
  into the prompt. Say "confirm" to send it or "abort" to drop it. It works
  in one breath ("Zevro enhance make it terse, confirm") or across several
  pauses.

Both are one undoable edit. Requests run on a worker thread with a spinner
in the bottom bar; a failure leaves the prompt untouched and shows the error.

The current project's glossary, context, and vocabulary go to the model
with every request, so it can resolve misheard project names.

The model is `gpt-5.6-luna` via OpenAI chat completions. The API key comes
from, in order: the key saved in Settings, the `OPENAI_API_KEY` environment
variable, or a `.env` file in the working directory. Token usage is logged
per call and totalled in Settings.

## Tools (plugins)

A tool is a folder with a `tool.json` manifest and something to run. The
model picks the tool for a spoken or typed request and fills in its
arguments; the app runs the script and shows what it said. No Rust needed:
`examples/tools/save_quote` is a ten-line Python script that appends a
dictated quotation to a SQLite file.

Ask for one with **Zevro tool** (captured like enhance: say the request,
then "confirm" or "abort") or by typing `/tool …` in the AI box. Read a
quote into the prompt, then "Zevro tool save that quote, confirm".

```json
{
  "name": "save_quote",
  "description": "Save a quotation to the local quotes database.",
  "parameters": { "type": "object", "properties": { "quote": { "type": "string" } }, "required": ["quote"] },
  "command": ["python3", "save_quote.py"],
  "review": false
}
```

- `parameters` is a JSON Schema and goes to the model verbatim. The prompt
  text is never an argument; the script always gets it separately.
- `command` runs in the tool's folder. A bare program name is looked up on
  `PATH`, or in the folder if a file of that name is there.
- `review: true` shows the chosen call in the notification strip with
  **Run** and **Cancel** instead of running it at once.

The script receives JSON on stdin, `{"arguments": {...}, "prompt": "..."}`,
and the prompt again in `PROMPTBOX_PROMPT`. It replies on stdout with
`{"message": "...", "replace_prompt": "..."}` (both optional; plain text is
taken as the message). A non-zero exit reports stderr as the error. Scripts
are killed after 30 s. The model's answer, the call being run, and the
result all appear in the notification strip.

Tools live in `tools/` under the data directory, one folder each. Settings
lists what loaded and has a Reload button. Scripts run as you, with your
permissions: the folder is the trust boundary.

## Settings and data

⚙ in the top bar opens Settings: OpenAI API key (stored masked), model,
and voice trigger word, which Save persists; and the Send paste options
and appearance (Auto follows the system, or Light / Dark), which persist
as soon as they are clicked.

Everything lives in the platform data directory, `~/Library/Application
Support/promptbox` on macOS:

| File | Contents |
|---|---|
| `settings.json` | the Settings window, plus pin state |
| `draft.txt` | autosaved current prompt, 500 ms after each change |
| `history.json` | last 50 sent prompts |
| `projects.json` | projects: vocabulary, corrections, glossary, AI context |
| `tools/<name>/tool.json` | tool plugins (see Tools) |
| `models/ggml-base.en.bin` | the speech model |

## Development

```sh
cargo run --locked --release                         # launch the app
cargo test --locked                                  # unit tests + headless UI tests
cargo clippy --locked --all-targets -- -D warnings   # lint
cargo fmt --all                                      # format
```

`scripts/bundle.sh` builds a release binary, wraps it as `Prompt Box.app` in
`~/Applications` (pass another directory to override), and ad-hoc signs it
so Spotlight and Launchpad find it and the microphone / Accessibility
grants stick to the app. A bundled app starts with `/` as its working
directory, so it reads the API key from Settings rather than a `.env`.

Dev aids: `PROMPTBOX_AUTOSTART=1` starts listening at launch;
`PROMPTBOX_FAKE_MIC=/path/to/16k-mono.wav` feeds a WAV through the real
capture path instead of the microphone (the spike's fixtures work); the
**Debug** menu runs scripted dictation without any model.

### Layout

```
src/main.rs              thin launcher
src/lib.rs               module tree
src/core/                deterministic, egui-free
  action.rs              AppAction, Effect, AppCore::dispatch (the state machine)
  document.rs            committed text + one provisional span + edit history
  commands.rs            voice-command grammar and extraction
  text.rs                sentence / paragraph ranges for delete operations
  project.rs             projects: correction rules, recognizer terms, AI context
src/ports/               traits the core needs
  speech.rs, engine.rs   speech events and the speech-engine boundary
  clipboard.rs           clipboard that reports failure
  history.rs             sent prompts, draft, settings, projects
  ai.rs                  prompt rewriter and tool chooser
  tools.rs               tool manifests, runner, script I/O contract
  typist.rs              keyboard injection into the focused app
src/adapters/
  audio.rs               cpal capture -> ring buffer -> resample -> 20 ms chunks
  speech/                whisper.cpp engine: VAD, worker per session, partials
  model.rs               model path and background download
  openai.rs              chat-completions rewriter and tool calling, .env reader
  tools.rs               tool.json discovery, child-process runner
  typist.rs              enigo paste + Return, Accessibility check
  clipboard.rs, persistence.rs, fake_speech.rs
src/app.rs               PromptBoxApp: owns core + adapters + recognizer + mic; runs effects
src/ui.rs                egui drawing and input -> actions (edit diffing, shortcuts)
tests/ui.rs              headless flows via egui_kittest with fake adapters
tests/whisper.rs         real engine over a spike fixture (ignored by default)
tests/openai_live.rs     real OpenAI call (ignored by default)
```

### Testing approach

Everything enters `AppCore::dispatch(action, clock)` and leaves as effects,
so core tests are plain state-transition tests with an explicit clock and
no sleeping. Adapters have their own tests; the file store uses a temp
directory. UI tests run the real `eframe::App` headlessly with fake
clipboard, history, and rewriter, find widgets by label, and advance frames
with `harness.run_steps(2)`: one frame to process the click, one to render
its result.

Ignored tests hit real services:

```sh
cargo test --release --test whisper -- --ignored       # needs the model and spike fixtures
cargo test --test openai_live -- --ignored --nocapture # spends a few hundred tokens
```

### Pre-commit hook

`.githooks/pre-commit` checks formatting, runs Clippy, and runs the tests
before each commit. It never modifies or stages files. Enable it once per
clone:

```sh
git config core.hooksPath .githooks
```

## License

MIT, see `LICENSE`.
