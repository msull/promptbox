# Voice spike (Milestone 0)

Disposable feasibility spike for the voice prompt workbench. It answers two
questions before Milestone 2 commits to an engine and a document model:

1. Does whisper.cpp on Apple Silicon give useful, stable, low-latency
   *emulated* streaming partials, and can it be cancelled and restarted
   cleanly with bounded queues and visible overflow?
2. Does the "committed text + one provisional span + single edit history"
   document model survive concurrent speech events and manual edits?

This crate is standalone (own `Cargo.lock`, not a workspace member) so the
main `promptbox` crate keeps building without cmake or a model.

## Setup (macOS)

```sh
brew install cmake                     # whisper.cpp builds via CMake
cd spikes/voice-spike
mkdir -p models && cd models
B=https://huggingface.co/ggerganov/whisper.cpp/resolve/main
curl -L -o ggml-base.en.bin  $B/ggml-base.en.bin                      # 148 MB
curl -L -o ggml-small.en.bin $B/ggml-small.en.bin                     # 488 MB
curl -L -o ggml-large-v3-turbo-q5_0.bin $B/ggml-large-v3-turbo-q5_0.bin  # 574 MB, optional
cd ..
cargo build --release                  # first build compiles whisper.cpp (~1 min)
```

Always use `--release`; whisper in a debug build is unusably slow.

## Commands

```sh
cargo run --release -- info                                   # confirm METAL = 1
cargo run --release -- fixtures --profile                     # synthesize WAVs with `say`
cargo run --release -- run --model base.en --wav fixtures/03_three_sentences_Samantha_180.wav
cargo run --release -- run --model base.en --wav fixtures/01_pydantic_Samantha_180.wav \
    --fast --stop-after-ms 2500 --restart                     # cancel/restart mid-utterance
cargo run --release -- run --model small.en --wav fixtures/02_rust_egui_Samantha_180.wav \
    --chunk-ms 10 --audio-queue 5                             # provoke overflow -> AudioGap
cargo run --release -- bench --models base.en,small.en        # table over all Samantha_180 fixtures
cargo test --release                                          # doc/VAD/metrics unit tests
cargo test --release -- --ignored                             # engine tests (need model + fixtures)
```

`run` prints one line per event (wall ms, audio pushed, latency from the
last sample the event covers, session, sequence) then a summary. `bench`
runs each fixture x model in a subprocess (`run --json`) so peak RSS is per
model, and prints a table plus a per-model aggregate.

`fixtures/RECORDING_SCRIPT.md` is the same script set, formatted for reading
aloud, for later real-voice fixtures.

## What is in here

```
src/events.rs          SpeechEvent {session, sequence, audio_range, kind}
src/engine.rs          SpeechEngine trait, AudioChunk, EngineConfig, Counters
src/vad.rs             20 ms energy VAD with onset/hangover/pre-roll
src/worker.rs          recognition thread: VAD state machine + sliding-window partials
src/whisper_engine.rs  SpeechEngine impl: bounded std mpsc channels, restart/draining
src/feeder.rs          real-time or fast WAV feed, timeline with latencies
src/metrics.rs         WER, stability, TTFP, RTF, peak RSS
src/doc/               provisional-span document prototype + table-driven tests
tests/restart.rs       ignored engine tests: restart identity, overflow -> AudioGap
```

### Streaming emulation

whisper.cpp is not a streaming model. While the VAD says an utterance is in
progress, the worker re-runs `whisper_full` every `step_ms` (default 1 s)
over the audio from the utterance start to now, and emits a `Partial` when
the text changes. When the VAD sees `hangover_ms` of silence it runs one
last pass over the utterance and emits `Final`. Utterances longer than
`max_window_ms` (10 s) are split at the quietest 20 ms frame in the last two
seconds before the limit. Whisper is never run outside an utterance, and
results with high no-speech probability or deny-listed text are dropped, so
silence does not hallucinate.

### Identity and bounds

- `session_id` increments on every `start()`; `sequence` is per session and
  strictly increasing over every emitted event; `revision` is per utterance
  and only bumps when partial text changes.
- Audio chunks carry their first sample offset. The worker detects
  `chunk.start != expected` and emits `AudioGap{missing}` (zero-filling its
  buffer so offsets stay aligned). The feeder drops on `QueueFull` in
  real-time mode and counts it.
- `stop()` never blocks: it sends a Stop message if the queue has room,
  drops the sender, and moves the event receiver to a draining list so late
  events still surface with their old session id.

## Findings (2026-09-03, Apple M4 Pro, macOS, whisper-rs 0.16 + Metal)

All numbers are from `bench` on synthesized `say` fixtures fed in real time,
chunk 100 ms, 4 threads, no vocabulary hint unless stated. "lat" is wall
time from pushing the last sample a partial covers to seeing the partial.
Synthesized speech is cleaner than a microphone, so treat WER as relative,
not absolute; the VAD threshold in particular will need re-tuning on real
audio (`fixtures/RECORDING_SCRIPT.md`).

### Model comparison (6 fixtures, 94 s, Samantha voice, step 1 s)

| model                | load ms | ttfp med | lat med | lat p95 | stability | WER pooled | RTF   | RSS MB |
|----------------------|--------:|---------:|--------:|--------:|----------:|-----------:|------:|-------:|
| base.en              |     103 |   761 ms |  119 ms |  202 ms |      0.44 |       3.9% | 0.094 |    333 |
| small.en             |     173 |   839 ms |  259 ms |  402 ms |      0.42 |       6.1% | 0.219 |    748 |
| large-v3-turbo-q5_0  |     198 |  1199 ms |  602 ms | 1157 ms |      0.57 |       6.8% | 0.642 |    793 |

Same fixtures with the Daniel (en-GB) voice: base.en 7.4%, small.en 6.5%.
Faster speech (220 wpm, small.en): 4.9%. Threads 4 vs 8: no difference;
inference is Metal-bound.

Almost all errors are project/technical names ("egui" -> "EGGAD", "Acme" ->
"Olive", "whisper.cpp" -> "whisper CPP", "FastHTML" -> "fast HTML"). Plain
English sentences were 0% on every model. Those are exactly the errors the
corrections layer and vocabulary hints are for.

### Vocabulary hint (`--hint "Acme, Univer Sheets, FastHTML, Pydantic, DynamoDB"`)

| model    | no hint | with hint |
|----------|--------:|----------:|
| base.en  |   14.7% |      8.8% |
| small.en |   20.6% |      8.8% |

`initial_prompt` works and is cheap. It should be wired to project
vocabulary in Milestone 2/5.

### Step size (base.en / small.en)

| step   | ttfp med (base/small) | lat med       | RTF (base/small) | WER |
|--------|----------------------:|--------------:|-----------------:|----:|
| 1000ms | 761 / 839 ms          | 119 / 259 ms  | 0.09 / 0.22      | same |
|  500ms | 349 / 416 ms          | 133 / 250 ms  | 0.21 / 0.43      | same |

Halving the step halves time-to-first-partial with no accuracy cost; RTF
stays well under 1 for base and small. Turbo at 500 ms would exceed real
time on 10 s windows.

### Hangover

700 ms vs 400 ms on the three-sentence fixture (base.en): both segment into
3 utterances with 0% WER; final latency drops from ~500 ms to ~200 ms. Real
speech has longer mid-sentence pauses than `say`, so start at 500-700 ms and
tune on recorded audio.

### Reliability behaviour verified

- **No hallucinations**: 0 filtered windows across 48 real-time runs, because
  whisper is only invoked inside a VAD-detected utterance.
- **No processing delays** in real time for any model; forced splits on the
  51 s dictation land in pauses (WER 1.2% base.en).
- **Overflow is visible**: 10 ms chunks with a 5-slot queue produce
  `AudioGap` events with exact sample ranges and feeder drop counts
  (`tests/restart.rs::tiny_queue_reports_audio_gaps`).
- **Restart identity**: stopping mid-utterance and starting a new session
  yields late session-1 events interleaved after session-2 events, each with
  its own strictly increasing sequence
  (`tests/restart.rs::late_events_keep_old_session_id_after_restart`).
- **Stop is prompt and non-blocking**; drain of a stopped worker completes
  within one inference.
- First Metal shader compile on a machine costs ~5.5 s once (cached after).

### What this changes in the design

1. **Provisional text is the whole utterance, not a trailing few words.**
   Every step re-decodes from the utterance start, and greedy whisper
   changes earlier words as context grows: median stability 0.4-0.6, 1-4
   retracted words per update, and in 23 of 48 runs the Final differed from
   the last Partial. The document model already treats the provisional span
   as one replaceable unit, which is the right shape. The UI treatment
   should dim the entire in-progress utterance, not just the tail.
2. **Late finals after restart are real content.** With `finalize_on_stop`,
   the old session's Final carries the words spoken before the restart. The
   spike's `Document` rejects them as `StaleSession`. The app should either
   keep the old session accepted until its receiver drains, or make
   stop/restart commit the old utterance before switching the active
   session. Decide this in Milestone 2; the test exists.
3. **Queue bounds**: 50 x 100 ms audio chunks (5 s) and 256 events never
   overflowed in real time on any model; a single inference is < 700 ms.
   The rolling raw-audio buffer can simply be the session buffer for now.
4. **Anchor policy** (implemented and tested in `doc/`): capture the
   insertion anchor at `VoiceStarted`; edits before the anchor shift it,
   edits after do not, overlapping edits commit or cancel the span first;
   a new utterance commits any live span; a Final is one history entry with
   the provisional text kept as provenance.
5. **Recommended Milestone 2 baseline**: whisper.cpp `base.en` or
   `small.en` with Metal, step 500 ms, hangover 500-700 ms, `no_context`,
   `single_segment`, project vocabulary as `initial_prompt`. base.en is the
   surprising winner on latency and (synthetic) accuracy; confirm on real
   voice before choosing, and keep turbo out of the live path.

### Open

- Energy VAD threshold (-40 dBFS) is tuned to `say` output. Real mic audio
  needs re-tuning or a model VAD (Silero via whisper.cpp's VAD support).
- Stability could improve with beam search or by only re-decoding the last
  N seconds and stitching; both trade latency. Measure on recorded audio.
- Peak RSS via `getrusage` includes unified-memory Metal buffers; treat as
  an upper bound.
