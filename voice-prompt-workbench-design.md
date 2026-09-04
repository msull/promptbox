# Voice Prompt Workbench

## Status

Initial design draft for a Rust desktop application focused on
near-real-time voice prompt composition for coding agents.

## Product Concept

Voice Prompt Workbench is a desktop application for composing
high-quality prompts by voice.

The application continuously captures speech and converts it into
editable text with very low perceived latency. The transcript is not
immediately sent anywhere. Instead, it lives in a lightweight editing
workspace where the user can:

-   See speech appear as close to real time as practical.
-   Quickly verify that speech is actually being captured.
-   Edit the transcript with keyboard and mouse.
-   Apply project-specific terminology and deterministic corrections.
-   Invoke editing/application actions using voice commands.
-   Optionally use AI to clean up or rewrite the completed prompt.
-   Copy the finished prompt to the clipboard and immediately begin
    another.

The primary initial use case is creating prompts for coding agents. A
longer-term direction may include dedicated hardware, so core
transcription and command-processing logic should remain reasonably
independent from the desktop GUI.

------------------------------------------------------------------------

## Product Priorities

The transcription experience should optimize for these priorities, in
order:

1.  **Never silently lose speech.**
2.  **Show transcription as close to real time as practical.**
3.  **Maximize final transcription accuracy.**

The application should prefer immediate visible feedback, even if the
newest few words remain provisional and are revised as additional audio
is processed.

A core problem this product is intended to solve is the failure mode
where a user speaks for an extended period, assumes the speech was
captured, and only later discovers that a section in the middle was
lost.

------------------------------------------------------------------------

## Initial Platform

-   **Language:** Rust
-   **GUI:** `egui` / `eframe`
-   **Initial target:** Desktop; choose one first supported operating
    system during the feasibility milestone rather than treating macOS,
    Windows, and Linux as interchangeable targets.
-   **Speech recognition:** Local-first, with a pluggable speech engine
    architecture

`egui` is a good fit because this is a highly interactive application
whose UI is primarily a representation of continuously changing
application state.

The GUI should remain a relatively thin layer over the application core
so that future interfaces or dedicated hardware do not require rewriting
the transcription and command pipeline.

------------------------------------------------------------------------

## Primary Interaction

Conceptually:

``` text
Microphone
    ↓
Audio capture / buffering
    ↓
Speech recognition
    ↓
Live provisional transcript
    ↓
Final recognition
    ↓
Deterministic command extraction
    ↓
Project-specific deterministic corrections
    ↓
Committed transcript
    ↓
User review / manual editing / voice actions
    ↓
Optional AI rewrite
    ↓
Send
```

The central UI is a large editable prompt area.

There should not be a strong distinction between "dictation mode" and
"editing mode." The user should be able to speak, pause, type, select
text, delete text, resume speaking, and continue naturally.

------------------------------------------------------------------------

## Live Transcription and Feedback

Live visible transcription is a core reliability feature, not merely a
UI enhancement.

### Partial and Final Results

The speech engine should expose both **partial/provisional** and
**final/committed** transcription results.

While speaking, provisional text should appear immediately in the prompt
editor. The engine may revise this trailing text as it receives more
audio and becomes more confident.

Example:

``` text
We should update the DynamoDB model so that the
conditional write prevents two workers from updating
the same record at once. Then I think we should also

[move that validation down into the service...]
```

The bracketed portion represents provisional text. In the actual UI, the
distinction should be subtle---for example, slightly dimmed text or
another lightweight treatment.

The goal is for the user to be able to glance at the application while
speaking and immediately see:

> Yes, it is hearing me and transcription is progressing.

### Independent Audio Feedback

The UI should also provide an audio activity indicator independent of
transcription:

``` text
● Listening  ▂▃▆█▅▂
```

This provides two independent signals:

``` text
Microphone activity
    → Audio is reaching the application.

Transcript activity
    → The speech recognizer is successfully processing it.
```

This distinction helps diagnose failures. If the audio meter is flat,
capture is failing. If audio activity continues but transcription stops
advancing, recognition may be stalled.

### Stall Detection

The application should detect likely transcription stalls.

For example:

``` text
voice detected + audio arriving
        │
        ├── transcript advancing → normal
        │
        └── no transcript progress for several seconds
                    ↓
          "Transcription delayed"
```

This should be a visible but non-modal warning.

Exact thresholds should be determined experimentally rather than treated
as fixed requirements.

### Audio Buffering

Maintain a rolling buffer of recent raw audio.

This provides a future recovery path if the recognizer stalls, crashes,
or produces a suspicious gap. Buffered audio could potentially be
replayed into the speech engine or used to retranscribe a recent
segment.

The initial implementation does not necessarily need automatic recovery,
but the audio pipeline should avoid making recovery impossible.

The rolling buffer is not, by itself, a guarantee against silent loss.
Every captured chunk should carry a monotonically increasing sample range
or sequence number. The pipeline should count and report chunks dropped by
queue overflow, device interruption, or buffer overwrite. Any detected gap
must immediately change the session status from healthy to degraded and
remain visible until acknowledged or recovered.

The microphone callback must do bounded work and must not block on speech
recognition, disk I/O, or the GUI. It should feed a preallocated or bounded
buffer consumed by a worker. Buffer capacity and overflow policy are part of
the application's observable reliability behavior, not merely implementation
details.

------------------------------------------------------------------------

## Transcript Model

The visible text box should not be the only source of truth.

Internally, preserve transcript segments and their relationship to the
original recognition output. The application must also define how those
segments relate to arbitrary manual edits.

A conceptual model:

``` rust
struct Transcript {
    committed: String,
    provisional: Option<ProvisionalSpan>,
    recognition: Vec<RecognitionRecord>,
    revisions: Vec<PromptRevision>,
}

struct ProvisionalSpan {
    session_id: SessionId,
    utterance_id: UtteranceId,
    revision: u64,
    insertion_anchor: TextPosition,
    text: String,
}

struct RecognitionRecord {
    id: SegmentId,
    utterance_id: UtteranceId,
    original_text: String,
    corrected_text: String,
    audio_range: Range<u64>,
    confidence: Option<f32>,
    committed_revision_id: RevisionId,
}
```

### Editing and Reconciliation Invariants

The first implementation should use these invariants:

1.  Committed document text is authoritative user content.
2.  At most one active provisional span exists for an utterance in V1.
3.  A partial speech event may replace only the provisional span with the
    same session and utterance identity.
4.  A final event commits that provisional span as one logical, undoable edit.
5.  Keyboard, mouse, voice, correction, and AI changes all become explicit
    range-replacement operations rather than independent mutations of a
    second string.
6.  Stale, duplicate, and out-of-order speech events do not change the
    document.

The editor should capture a dictation insertion anchor when an utterance
starts. Moving the cursor later must not allow a partial update to overwrite
unrelated manual edits. If an edit overlaps the provisional span, the
application should first commit or cancel that provisional span according to
an explicit policy.

The rendered text area may be a projection of committed text plus the active
provisional span, but there must be a single authoritative edit history. Raw
recognition text and audio associations remain immutable provenance attached
to the relevant committed operation.

This leaves room for:

-   Undo/restore.
-   Recovering original transcription after corrections or AI rewriting.
-   Retranscribing a portion.
-   Associating transcript sections with audio.
-   Highlighting uncertain recognition.
-   Identifying recently spoken text.
-   Prompt revision history.

The user-facing experience should still feel like one ordinary editable
text area.

------------------------------------------------------------------------

## Projects

A project contains context and configuration associated with the work
the user is currently doing.

Example:

``` text
Project: Acme

Vocabulary
----------
Univer
DynamoDB
FastHTML
Pydantic
Acme

Corrections
-----------
"you never sheets" → "Univer Sheets"
"fast html"        → "FastHTML"
"dynamo DB"        → "DynamoDB"
```

Projects should eventually support:

-   Vocabulary / speech-recognition hints.
-   Deterministic post-transcription replacement rules.
-   Project-specific voice commands.
-   A small project glossary.
-   Freeform project context for AI rewriting.
-   AI rewrite instructions/preferences.
-   Other project-specific application settings.

### Vocabulary vs. Corrections

Vocabulary hints and deterministic replacements should remain separate
concepts.

**Vocabulary hints** are supplied to speech engines that support
contextual prompting or vocabulary biasing.

**Corrections** operate after recognition and therefore work
consistently regardless of the selected speech engine.

Corrections should apply to newly finalized dictation before it becomes a
committed document edit. They should not repeatedly rewrite the whole prompt
or silently alter later manual edits. Rule ordering, Unicode boundaries, case
handling, and overlapping matches must be deterministic and covered by tests.

------------------------------------------------------------------------

## Voice Commands

Voice commands should be separate from ordinary dictation and should
initially use a deterministic command grammar.

A trigger word activates the command channel. For example, using
`Zevro`:

> "We should probably move this validation into the service layer. Zevro
> delete sentence."

The first sentence becomes transcript text. The command phrase does not.

Possible initial commands:

``` text
Zevro delete sentence
Zevro delete paragraph
Zevro undo
Zevro newline
Zevro new paragraph
Zevro clear
Zevro copy
Zevro send
```

Voice-command detection should **not require an LLM in V1**. Core
commands should be fast, local, predictable, and deterministic.

In V1, commands should be extracted from finalized recognition units before
project corrections are applied:

``` text
Raw final recognition
    ↓
Deterministic command extraction
    ├── command action, if present
    └── remaining dictation text
              ↓
       project corrections
              ↓
       committed document edit
```

Each recognized command must have a stable event identity and execute at
most once. Successive provisional hypotheses must not repeatedly trigger the
same command. Destructive actions such as Clear and Send should create an
immediately undoable revision; a later usability test can determine whether
they also need explicit confirmation.

A conceptual action model:

``` rust
enum AppAction {
    ReplaceText {
        range: TextRange,
        text: String,
        source: RevisionSource,
    },
    SpeechEventReceived(SpeechEvent),
    CopyPrompt,
    SendPrompt,
    ClipboardWriteFinished(SendResult),
    HistorySaveFinished(SendResult),
    ClearPrompt,
    Undo,
    DeleteSentence,
    DeleteParagraph,
    Newline,
    NewParagraph,
}
```

------------------------------------------------------------------------

## Send Behavior

For V1, "Send" does not directly integrate with coding agents.

**Send means:**

1.  Snapshot the current prompt.
2.  Write the snapshot to the system clipboard through a clipboard adapter
    that can report success or failure.
3.  Save the snapshot to lightweight recent history.
4.  Clear the prompt editor only after both required operations succeed.
5.  Show a brief non-modal notification such as **"Prompt copied."**

The clipboard and history store are explicit application ports returning
`Result`. A GUI request to copy text is not, by itself, proof that the
platform clipboard accepted it. If either operation fails, retain the prompt,
show a useful error, and make retry safe. Sending the same immutable snapshot
again must not corrupt history.

Clearing after Send should be represented as an undoable action. An autosaved
current draft provides an additional recovery path after a crash or an
accidental send.

`Copy` and `Send` remain separate actions:

-   **Copy:** copy without clearing.
-   **Send:** copy and clear.

The notification should be brief and non-modal, such as a toast that
disappears automatically.

Example flow:

``` text
Speak
  ↓
Live transcript
  ↓
Edit / voice cleanup
  ↓
Send
  ├─ Copy full prompt to clipboard
  ├─ Save to recent history
  ├─ Clear editor
  └─ Toast: "Prompt copied"
```

Clipboard/send should be included in the first genuinely usable build
rather than deferred as a later integration feature.

------------------------------------------------------------------------

## AI Rewrite Layer

AI-assisted rewriting is an optional second-stage cleanup mechanism, not
part of the live transcription path.

The intended flow is:

``` text
Speech
  ↓
Near-real-time transcript
  ↓
Deterministic project corrections
  ↓
User review / editing
  ↓
Optional AI rewrite
  ↓
Send
```

The AI should not silently rewrite the transcript while the user is
speaking. The live transcript serves as feedback about what was actually
captured, while AI rewriting is an explicit transformation requested by
the user.

Potential rewrite actions include:

-   **Clean Up**
-   **Make Concise**
-   **Rewrite for Agent**

Project context can help the model resolve transcription errors and
technical terminology. For example, if a project glossary contains
`Univer Sheets`, an AI rewrite can often correctly infer that a
transcription such as "universe sheets" refers to that project-specific
term.

Project AI context may include:

-   Glossary / important names.
-   Technology names.
-   Repository or architecture terminology.
-   A short freeform description of the project.
-   Conventions the model should preserve.
-   Instructions about what the rewrite should or should not change.

### Revision Safety

AI rewrites should be reversible.

A conceptual model:

``` rust
struct PromptRevision {
    text: String,
    source: RevisionSource,
    created_at: SystemTime,
}

enum RevisionSource {
    Transcription,
    ManualEdit,
    AiRewrite,
}
```

Persist a wall-clock timestamp such as `SystemTime` (or a serialized UTC
timestamp) with history. Use `Instant` only for process-local measurements
such as stall thresholds and latency.

This allows inexpensive undo, restore, and potentially compare behavior
later.

The overall philosophy is:

> Speech recognition gets words onto the screen quickly and visibly.\
> Project rules fix predictable mistakes deterministically.\
> AI fixes meaning-level messiness when explicitly requested.

------------------------------------------------------------------------

## Speech Engine Architecture

Speech recognition should be pluggable rather than coupled directly to
the UI.

A conceptual interface:

``` rust
trait SpeechEngine {
    fn start(&mut self, config: SpeechConfig) -> Result<()>;
    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<()>;
    fn poll(&mut self, limit: usize) -> Result<Vec<SpeechEvent>>;
    fn stop(&mut self) -> Result<()>;
}
```

This trait is called by a recognition worker, never directly by the audio
callback or UI. Bounded channels connect audio capture to recognition and
recognition to application orchestration. The worker reports overflow and
failure explicitly. The UI drains only a bounded number of events per frame
and requests a repaint when background work publishes new state.

Conceptual events:

``` rust
struct SpeechEvent {
    session_id: SessionId,
    sequence: u64,
    audio_range: Range<u64>,
    kind: SpeechEventKind,
}

enum SpeechEventKind {
    VoiceStarted {
        utterance_id: UtteranceId,
    },
    VoiceEnded {
        utterance_id: UtteranceId,
    },

    Partial {
        utterance_id: UtteranceId,
        revision: u64,
        text: String,
    },

    Final {
        utterance_id: UtteranceId,
        text: String,
        confidence: Option<f32>,
    },

    ProcessingDelayed,
    AudioGap {
        missing: Range<u64>,
    },
    Error(SpeechError),
}
```

Audio ranges are monotonic sample offsets within a session; wall-clock time
is supplementary metadata, not the ordering authority. Session identity
prevents late events from a stopped or restarted engine from modifying the
current document. Sequence and revision numbers make duplicates, gaps, and
out-of-order partials detectable.

The exact Rust interface should be refined during implementation. The
important architectural requirement is that the rest of the application
consumes a stream of recognition events rather than depending directly
on a specific model/runtime.

------------------------------------------------------------------------

## Speech Engine Strategy

Before committing Milestone 2 to an engine, build a short feasibility spike
with representative recorded audio. Validate that candidate engines actually
provide useful streaming partials, can be cancelled and restarted, fit the
target hardware, and can be packaged and redistributed. The first product
implementation should then use the strongest practical local baseline while
the repeatable benchmark suite continues to evolve.

A sensible baseline is Whisper.cpp through Rust bindings such as
`whisper-rs`.

Other current local models/runtimes, including Parakeet-family models,
should be evaluated rather than choosing the long-term engine based only
on reputation or generic benchmarks.

The speech engine should ultimately be selected using the application's
actual workload.

### Benchmark Criteria

Do not optimize solely for generic word error rate.

Important criteria include:

-   Time until the first useful partial transcription appears.
-   Frequency of partial updates.
-   Accuracy of final text.
-   Stability of provisional text.
-   Handling of long continuous dictation.
-   Whether speech is ever silently dropped.
-   Recovery behavior after pauses or processing delays.
-   CPU/GPU utilization.
-   Memory use.
-   Model startup/load time.
-   Performance with technical developer vocabulary.
-   Ability to accept vocabulary/context hints.
-   Ease of local distribution with the Rust application.

Representative test speech should contain realistic coding-agent
prompts, for example:

> Add a Pydantic model for the DynamoDB item and use a conditional
> expression so the write isn't overwritten.

Tests should deliberately include project-specific and difficult
technical terminology.

------------------------------------------------------------------------

## Initial UI

The first UI should remain intentionally simple.

``` text
┌─────────────────────────────────────────────────────────┐
│ ● Listening ▂▃▆█   Project: [ Acme ▾ ]       ⚙        │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  We should change the DynamoDB conditional write so     │
│  that the existing model isn't overwritten when...      │
│                                                         │
│                                                         │
│                                                         │
│                                                         │
├─────────────────────────────────────────────────────────┤
│ Undo   Delete sentence   AI Cleanup   Copy   Send →      │
└─────────────────────────────────────────────────────────┘
```

A narrow optional panel can expose project vocabulary, corrections, and
configuration.

The transcript should dominate the interface.

> **The text is the product.**

Avoid surrounding the editor with unnecessary transcription controls or
diagnostics.

------------------------------------------------------------------------

## High-Level Architecture

``` text
┌─────────────────────────────────────────┐
│                egui UI                  │
└──────────────────┬──────────────────────┘
                   │ actions / state
┌──────────────────▼──────────────────────┐
│            Application Core             │
│ transcript / projects / undo / history │
└───────┬──────────┬───────────┬──────────┘
        │          │           │
  ┌─────▼────┐ ┌───▼────────┐ ┌▼───────────┐
  │ Commands │ │ Corrections│ │ AI Rewrite │
  └──────────┘ └────────────┘ └────────────┘
        ▲
        │ identified speech events
┌───────┴─────────────────────────────────┐
│            Speech Engine API            │
└──────────────────┬──────────────────────┘
                   │
          ┌────────▼────────┐
          │ Local STT Engine│
          └────────▲────────┘
                   │ PCM
          ┌────────┴────────┐
          │ Audio Capture   │
          │ + Rolling Buffer│
          └─────────────────┘
```

### Recommended Source Shape

Keep the application in one crate initially. Module boundaries are useful;
separate packages and a workspace are not yet necessary.

``` text
src/
├── core/
│   ├── document.rs      committed/provisional text and edits
│   ├── action.rs        actions, revisions, and undo transactions
│   └── project.rs       vocabulary and correction rules
├── ports/
│   ├── speech.rs        engine-facing types and events
│   ├── clipboard.rs     clipboard capability
│   └── history.rs       draft and sent-prompt persistence
├── adapters/
│   ├── audio/           platform capture and buffering
│   ├── speech/          concrete local recognizer
│   └── persistence/     filesystem-backed storage
├── app.rs                    orchestration, channels, and effects
├── ui.rs                     egui rendering and input mapping
├── lib.rs
└── main.rs                   native startup only
```

`AppCore` should be deterministic and independent of egui. Buttons, keyboard
shortcuts, voice commands, speech events, and completed asynchronous work all
enter through a common action-dispatch path. Actions may request side effects
such as clipboard writes or history saves; adapters perform those effects and
return success or failure as new actions. This keeps the state machine easy to
test without a window, microphone, filesystem, or real model.

`PromptBoxApp` owns `AppCore`, the UI-facing service handles, and channel
receivers. It renders state and maps interaction into actions, but it does not
own transcript semantics. Keep engine-specific model types out of `core`.

------------------------------------------------------------------------

## Suggested Implementation Milestones

### Milestone 0 - Feasibility and Risk Spikes

Before building around irreversible assumptions:

-   Choose the first supported desktop operating system.
-   Feed representative recorded audio through promising local engines.
-   Verify useful partial-result behavior, cancellation, restart, resource
    use, model redistribution, and application packaging.
-   Prototype provisional-span replacement while simulated speech events and
    manual edits occur concurrently.
-   Define event identity, queue bounds, overflow reporting, and the dictation
    insertion-anchor policy.

These are disposable spikes whose conclusions become tests and interfaces in
the product code.

### Milestone 1 - Application Shell

Build the basic `eframe` application:

-   Main editable prompt area.
-   Project selector placeholder.
-   Truthful idle/listening/degraded/error states, initially driven by fakes.
-   `AppCore`, document operations, and fake speech events.
-   Copy action.
-   Send action.
-   Clipboard integration.
-   "Prompt copied" toast.
-   Lightweight sent-prompt history.
-   Autosaved current draft and failure-safe Send behavior.

This establishes the fundamental prompt-workbench interaction before
speech recognition is introduced.

### Milestone 2 - Live Audio and Transcription

Implement:

-   Microphone capture.
-   Audio activity visualization.
-   One local speech engine.
-   Partial transcription.
-   Final transcription.
-   Continuous updates to the prompt editor.
-   Basic stall/error state.
-   Rolling audio buffer.
-   Bounded worker channels, event identity, and visible gap reporting.

At the end of this milestone, the application should already be useful
for real dictation.

### Milestone 3 - Transcript Operations

Add:

-   Undo.
-   Delete last sentence.
-   Delete last paragraph.
-   Clear.
-   Newline / new paragraph.
-   Appropriate keyboard shortcuts.
-   Transcript/revision state needed to make these operations reliable.

### Milestone 4 - Voice Command Channel

Implement:

-   Trigger-word detection.
-   Deterministic command parsing.
-   Mapping voice commands to existing `AppAction`s.
-   Ensure recognized command text is excluded from the prompt.

### Milestone 5 - Projects

Implement persisted projects with:

-   Project selection.
-   Vocabulary.
-   Correction rules.
-   Project glossary.
-   Freeform project context.
-   Settings storage.

### Milestone 6 - AI Rewrite

Implement an explicit AI transformation layer:

-   Clean Up.
-   Make Concise.
-   Rewrite for Agent.
-   Project context supplied to the model.
-   Preserve pre-rewrite revisions.
-   Undo/restore after rewrite.

### Milestone 7 - Speech Engine Evaluation and Default Selection

Grow the feasibility harness from Milestone 0 into a repeatable benchmark
suite using realistic developer dictation.

Compare candidate local engines on:

-   Partial-result latency.
-   Final accuracy.
-   Long-dictation reliability.
-   Technical vocabulary.
-   Resource usage.
-   Packaging complexity.

Use the results to decide whether the baseline speech engine should
remain the default.

------------------------------------------------------------------------

## Testing Strategy

Most behavior should be tested below the GUI through deterministic state
transitions and fake ports.

-   Table-driven document tests should cover partial replacement,
    finalization, cursor movement, overlapping manual edits, corrections, and
    undo transaction boundaries.
-   Speech-event tests should inject duplicates, stale sessions, reordered
    partials, missing sequences, engine restarts, queue overflow, and delayed
    finals.
-   Text-operation tests must include Unicode and grapheme-boundary cases;
    Rust string byte offsets must not be confused with user-visible character
    positions.
-   Clipboard and history fakes should inject every failure ordering and
    verify that Send never clears the only recoverable prompt.
-   Persistence tests should cover atomic replacement, corrupt or partial
    files, schema migration, and recovery of the autosaved draft.
-   Headless egui tests should verify a small number of important user flows
    and accessible labels rather than duplicating every core-state test.
-   Recorded-audio integration tests should exercise an engine adapter with
    known fixtures. Latency and long-session benchmarks should be kept
    separate from fast correctness tests.

Use a fake monotonic clock for stall and toast behavior so tests do not sleep.
The application should expose counters for captured audio, dropped audio,
recognizer progress, and event gaps; reliability claims should be testable
against those counters.

------------------------------------------------------------------------

## Explicit Non-Goals for the First Pass

The initial implementation does **not** need:

-   Direct integration with coding-agent APIs.
-   Automated typing into another application.
-   Accessibility API automation.
-   Virtual keyboard/input-device emulation.
-   Cloud speech recognition.
-   Autonomous AI rewriting while dictating.
-   LLM-based interpretation of basic voice commands.
-   Dedicated hardware support.
-   A sophisticated document editor.

These may be explored later without complicating the first useful
version.

------------------------------------------------------------------------

## Open Design Questions

These should be resolved through implementation/testing rather than
blocking the initial scaffold:

-   Which local STT engine provides the best combination of streaming
    latency, reliability, and accuracy?
-   How should provisional text be represented inside an editable `egui`
    text area without making manual editing awkward?
-   What pause/latency threshold should trigger a transcription-stall
    warning?
-   How large should the rolling audio buffer be?
-   How should deterministic correction rules interact with manual
    edits?
-   Should a later trigger-word/command detector continue using the primary
    STT output or move to a separate recognizer or hybrid?
-   What persistence format should be used for projects and prompt
    history?
-   Which AI provider/model interface should be used for the first
    rewrite implementation?
-   How much recent history should be retained, and should history
    survive application restarts?

------------------------------------------------------------------------

## V1 Success Criteria

The first useful version succeeds if the user can:

1.  Launch the application and select a project.
2.  Start speaking and see text appear continuously with low perceived
    latency.
3.  Easily tell whether audio and transcription are still functioning.
4.  Stop speaking and immediately review the captured prompt.
5.  Make quick keyboard/mouse edits.
6.  Use a small set of voice commands for common editing actions.
7.  Benefit from project-specific terminology/corrections.
8.  Optionally run an AI cleanup/rewrite.
9.  Press Send and have the final prompt copied to the clipboard.
10. Immediately begin dictating the next prompt without losing the
    previous one to an accidental action.

The experience should feel less like "record audio and wait for
transcription" and more like **typing with your voice while retaining
full control over the text.**
