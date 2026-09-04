//! Recognition worker: owns a `WhisperState`, runs the energy VAD over the
//! session's audio, and emulates streaming partials by re-running whisper
//! over a window anchored at the utterance start every `step_ms`.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::Instant;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperState};

use crate::adapters::speech::vad::{EnergyVad, FRAME_LEN, VadTransition};
use crate::ports::engine::{AudioChunk, Counters, EngineConfig};
use crate::ports::speech::{SessionId, SpeechError, SpeechEvent, SpeechEventKind, UtteranceId};

pub enum Msg {
    Audio(AudioChunk),
    Stop,
}

const DENY_LIST: &[&str] = &[
    "[BLANK_AUDIO]",
    "[ Silence ]",
    "(silence)",
    "[Music]",
    "you",
    "Thank you.",
    "Thanks for watching!",
];

struct Utterance {
    id: UtteranceId,
    start: u64,
    last_run_end: u64,
    revision: u64,
    last_text: Option<String>,
}

pub struct Worker {
    state: WhisperState,
    cfg: EngineConfig,
    session: SessionId,
    tx: SyncSender<SpeechEvent>,
    counters: Arc<Counters>,
    seq: u64,
    overflow_pending: u64,
    /// Whole-session audio; index == sample offset (gaps are zero-filled).
    audio: Vec<f32>,
    expected_next: u64,
    vad: EnergyVad,
    frames_done: usize,
    utt: Option<Utterance>,
    next_utt: UtteranceId,
    last_utt_end: u64,
    delayed_flagged: bool,
    step: u64,
    max_window: u64,
    keep: u64,
    min_infer: usize,
    final_pad: u64,
}

impl Worker {
    pub fn spawn(
        ctx: &Arc<WhisperContext>,
        cfg: EngineConfig,
        session: SessionId,
        rx: Receiver<Msg>,
        tx: SyncSender<SpeechEvent>,
        counters: Arc<Counters>,
    ) -> anyhow::Result<std::thread::JoinHandle<()>> {
        let t = Instant::now();
        let state = ctx.create_state()?;
        counters.add(&counters.state_create_us, t.elapsed().as_micros() as u64);
        let ms = EngineConfig::ms_to_samples;
        let worker = Self {
            state,
            vad: EnergyVad::new(cfg.vad_db, cfg.onset_ms, cfg.hangover_ms, cfg.preroll_ms),
            step: ms(cfg.step_ms),
            max_window: ms(cfg.max_window_ms),
            keep: ms(cfg.keep_ms),
            min_infer: ms(cfg.min_infer_ms) as usize,
            final_pad: ms(300),
            cfg,
            session,
            tx,
            counters,
            seq: 0,
            overflow_pending: 0,
            audio: Vec::new(),
            expected_next: 0,
            frames_done: 0,
            utt: None,
            next_utt: 1,
            last_utt_end: 0,
            delayed_flagged: false,
        };
        let handle = std::thread::Builder::new()
            .name(format!("whisper-session-{session}"))
            .spawn(move || worker.run(&rx))?;
        Ok(handle)
    }

    fn run(mut self, rx: &Receiver<Msg>) {
        loop {
            let mut batch = Vec::new();
            let mut stop = false;
            match rx.recv() {
                Ok(Msg::Audio(c)) => batch.push(c),
                Ok(Msg::Stop) | Err(_) => stop = true,
            }
            if !stop {
                // Drain everything queued so a slow inference collapses lag
                // instead of accumulating it.
                loop {
                    match rx.try_recv() {
                        Ok(Msg::Audio(c)) => batch.push(c),
                        Ok(Msg::Stop) => {
                            stop = true;
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            let mut newly = 0u64;
            for c in batch {
                newly += c.samples.len() as u64;
                self.ingest(&c);
            }
            if self.utt.is_some() && newly > 2 * self.step && !self.delayed_flagged {
                self.delayed_flagged = true;
                self.counters.add(&self.counters.delayed_count, 1);
                let end = self.audio.len() as u64;
                self.emit(SpeechEventKind::ProcessingDelayed, end - newly..end);
            }
            self.process_frames();
            self.maybe_partial();
            if stop {
                self.on_stop();
                return;
            }
        }
    }

    fn ingest(&mut self, chunk: &AudioChunk) {
        let start = chunk.start_sample;
        if start > self.expected_next {
            let missing = self.expected_next..start;
            self.counters
                .add(&self.counters.gap_samples, missing.end - missing.start);
            self.audio.resize(
                self.audio.len() + (missing.end - missing.start) as usize,
                0.0,
            );
            self.emit(
                SpeechEventKind::AudioGap {
                    missing: missing.clone(),
                },
                missing,
            );
        }
        let skip = self.expected_next.saturating_sub(start) as usize;
        if skip < chunk.samples.len() {
            self.audio.extend_from_slice(&chunk.samples[skip..]);
        }
        self.expected_next = self.expected_next.max(chunk.end_sample());
    }

    fn process_frames(&mut self) {
        while (self.frames_done + 1) * FRAME_LEN <= self.audio.len() {
            let f = self.frames_done;
            let frame_start = (f * FRAME_LEN) as u64;
            let transition = self
                .vad
                .push_frame(&self.audio[f * FRAME_LEN..(f + 1) * FRAME_LEN], frame_start);
            self.frames_done += 1;
            match transition {
                Some(VadTransition::Started { sample }) => self.start_utterance(sample),
                Some(VadTransition::Ended { sample }) => self.end_utterance(sample),
                None => {}
            }
        }
    }

    fn start_utterance(&mut self, sample: u64) {
        let start = sample.max(self.last_utt_end);
        let id = self.next_utt;
        self.next_utt += 1;
        self.delayed_flagged = false;
        self.utt = Some(Utterance {
            id,
            start,
            last_run_end: start,
            revision: 0,
            last_text: None,
        });
        self.emit(
            SpeechEventKind::VoiceStarted { utterance: id },
            start..start,
        );
    }

    fn end_utterance(&mut self, silence_start: u64) {
        let Some(utt) = self.utt.take() else { return };
        self.emit(
            SpeechEventKind::VoiceEnded { utterance: utt.id },
            silence_start..silence_start,
        );
        let end = (silence_start + self.final_pad).min(self.audio.len() as u64);
        let text = self.infer(utt.start, end).unwrap_or_default();
        self.last_utt_end = silence_start;
        self.emit(
            SpeechEventKind::Final {
                utterance: utt.id,
                text,
                confidence: None,
            },
            utt.start..end,
        );
    }

    fn maybe_partial(&mut self) {
        loop {
            let now = self.audio.len() as u64;
            let Some(utt) = &self.utt else { return };
            let (id, start) = (utt.id, utt.start);
            if now - start >= self.max_window {
                self.forced_split(start + self.max_window);
                continue;
            }
            if now - utt.last_run_end < self.step {
                return;
            }
            let text = self.infer(start, now);
            let utt = self.utt.as_mut().expect("utterance present");
            utt.last_run_end = now;
            if let Some(text) = text
                && utt.last_text.as_deref() != Some(text.as_str())
            {
                utt.revision += 1;
                let revision = utt.revision;
                utt.last_text = Some(text.clone());
                self.emit(
                    SpeechEventKind::Partial {
                        utterance: id,
                        revision,
                        text,
                    },
                    start..now,
                );
            }
            return;
        }
    }

    /// Utterance reached the window limit: finalize it over its own audio,
    /// cutting at the quietest frame in the last two seconds before `limit`
    /// so the split lands in a pause when one exists, then start a new
    /// utterance there. Only a hard (non-quiet) cut keeps `keep` overlap.
    fn forced_split(&mut self, limit: u64) {
        let Some(utt) = self.utt.take() else { return };
        self.counters.add(&self.counters.forced_splits, 1);
        let search_from = limit
            .saturating_sub(EngineConfig::ms_to_samples(2000))
            .max(utt.start + 1);
        let mut best = (f32::MAX, limit);
        let mut pos = search_from;
        while pos + FRAME_LEN as u64 <= limit {
            let db = EnergyVad::frame_db(&self.audio[pos as usize..pos as usize + FRAME_LEN]);
            if db < best.0 {
                best = (db, pos);
            }
            pos += FRAME_LEN as u64;
        }
        let quiet = best.0 < self.cfg.vad_db;
        let split = if quiet { best.1 } else { limit };
        let text = self.infer(utt.start, split).unwrap_or_default();
        self.emit(
            SpeechEventKind::VoiceEnded { utterance: utt.id },
            split..split,
        );
        self.emit(
            SpeechEventKind::Final {
                utterance: utt.id,
                text,
                confidence: None,
            },
            utt.start..split,
        );
        let next_start = if quiet { split } else { split - self.keep };
        self.last_utt_end = next_start;
        self.start_utterance(next_start);
    }

    fn on_stop(&mut self) {
        let now = self.audio.len() as u64;
        if let Some(utt) = self.utt.take() {
            self.emit(SpeechEventKind::VoiceEnded { utterance: utt.id }, now..now);
            if self.cfg.finalize_on_stop {
                // Run a real final pass over everything we have rather than
                // reusing the last partial, which may be up to `step` stale.
                let text = self.infer(utt.start, now).unwrap_or_default();
                self.emit(
                    SpeechEventKind::Final {
                        utterance: utt.id,
                        text,
                        confidence: None,
                    },
                    utt.start..now,
                );
            }
        }
    }

    /// Runs whisper over `audio[start..end]`. Returns `None` when the result
    /// was filtered as a hallucination or inference failed.
    fn infer(&mut self, start: u64, end: u64) -> Option<String> {
        let mut window = self.audio[start as usize..end as usize].to_vec();
        if window.len() < self.min_infer {
            window.resize(self.min_infer, 0.0);
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_n_threads(self.cfg.threads);
        params.set_no_context(self.cfg.no_context);
        params.set_single_segment(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_temperature(0.0);
        if let Some(hint) = self.cfg.hint.as_deref() {
            params.set_initial_prompt(hint);
        }

        let t = Instant::now();
        let result = self.state.full(params, &window);
        let took = t.elapsed();
        self.counters.add(&self.counters.full_calls, 1);
        self.counters
            .add(&self.counters.full_time_us, took.as_micros() as u64);
        log::debug!(
            "whisper full: {:.1} s window in {:.0} ms",
            window.len() as f64 / 16_000.0,
            took.as_secs_f64() * 1000.0
        );
        if let Err(e) = result {
            self.emit(
                SpeechEventKind::Error(SpeechError::Engine(format!("{e:?}"))),
                start..end,
            );
            return None;
        }

        let mut text = String::new();
        let mut no_speech = 0.0f32;
        for (i, seg) in self.state.as_iter().enumerate() {
            if i == 0 {
                no_speech = seg.no_speech_probability();
            }
            if let Ok(s) = seg.to_str_lossy() {
                text.push_str(&s);
            }
        }
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if no_speech > 0.6 || text.is_empty() || DENY_LIST.contains(&text.as_str()) {
            self.counters.add(&self.counters.hallucinations_dropped, 1);
            return None;
        }
        Some(text)
    }

    fn emit(&mut self, kind: SpeechEventKind, audio_range: std::ops::Range<u64>) {
        if self.overflow_pending > 0 {
            let ev = self.make(
                SpeechEventKind::Error(SpeechError::EventOverflow {
                    dropped: self.overflow_pending,
                }),
                audio_range.clone(),
            );
            if self.tx.try_send(ev).is_ok() {
                self.overflow_pending = 0;
            }
        }
        let ev = self.make(kind, audio_range);
        if let Err(TrySendError::Full(_)) = self.tx.try_send(ev) {
            self.overflow_pending += 1;
            self.counters.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn make(&mut self, kind: SpeechEventKind, audio_range: std::ops::Range<u64>) -> SpeechEvent {
        self.seq += 1;
        SpeechEvent {
            session: self.session,
            sequence: self.seq,
            audio_range,
            kind,
        }
    }
}
