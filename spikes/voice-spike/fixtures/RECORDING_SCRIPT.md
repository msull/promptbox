# Recording script for real-voice fixtures

The synthesized fixtures come from `scripts/*.txt` via macOS `say`. That audio
is unrealistically clean, so at some point record yourself reading the same
scripts and drop the WAVs here so the same `bench` runs on real speech.

## How to record

Any tool works; the loader needs **16 kHz, mono, 16-bit PCM WAV**. With
QuickTime or Voice Memos, export and convert:

```sh
# afconvert ships with macOS
afconvert -f WAVE -d LEI16@16000 -c 1 input.m4a fixtures/01_pydantic_sully_1.wav
```

Name files `<script>_<speaker>_<take>.wav` (for example
`03_three_sentences_sully_1.wav`) so `run` finds the reference text
automatically and `bench --fixtures sully` selects them.

Read at a natural dictation pace. Leave a real pause where the script says
`[pause]`. Do not read the bracketed cues aloud.

## 01_pydantic

> Add a Pydantic model for the DynamoDB item and use a conditional
> expression so the write isn't overwritten.

## 02_rust_egui

> In the egui app, move the transcript state out of the ui method into an
> AppCore struct, and make the whisper.cpp worker send speech events over a
> bounded channel instead of mutating the document directly.

## 03_three_sentences

> We should update the DynamoDB model so that the conditional write prevents
> two workers from updating the same record at once. `[pause]`
> Then I think we should also move that validation down into the service
> layer. `[pause]`
> Add unit tests for both cases before you refactor anything.

## 04_short

> Run the tests.

## 05_long_dictation

> I want to refactor the audio capture pipeline so the microphone callback
> never blocks on speech recognition. Right now the callback pushes samples
> straight into the whisper worker, and when the model is busy the callback
> stalls and we lose audio without noticing. Instead, the callback should
> write into a preallocated ring buffer and increment a sample counter. A
> separate worker thread drains that buffer in fixed chunks, tags every chunk
> with its starting sample offset, and forwards it over a bounded channel. If
> the channel is full, the worker must count the dropped chunk and emit an
> audio gap event with the missing sample range, so the UI can flip the
> session status from healthy to degraded. Please also add a rolling buffer
> of the last thirty seconds of raw audio so we can retranscribe a segment
> later if the recognizer stalls. Keep the whisper specific types out of the
> core module, and write table driven tests for the gap detection using a
> fake clock so nothing sleeps in the test suite.

## 06_project_names

> In the Acme project, the Univer Sheets import should go through FastHTML
> and validate each row with Pydantic before writing to DynamoDB. Add a
> correction rule so that "you never sheets" becomes Univer Sheets.

## Extra takes worth recording

- One take of `05_long_dictation` with a few false starts and "um"s left in.
- One take of `03_three_sentences` at a distance from the mic, or with a fan
  running, to see how the energy VAD threshold copes with noise.
