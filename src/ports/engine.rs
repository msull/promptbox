//! The pluggable speech-engine boundary: audio chunks in, speech events out.
//! Called by the app orchestrator, never by an audio callback or the UI.
//! Ported from the Milestone 0 spike, which established the bounds below.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::ports::speech::{SessionId, SpeechEvent};

pub const SAMPLE_RATE: u32 = 16_000;

/// One chunk of mono 16 kHz f32 audio tagged with its first sample offset.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

impl AudioChunk {
    #[must_use]
    pub fn end_sample(&self) -> u64 {
        self.start_sample + self.samples.len() as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    QueueFull,
    NotRunning,
}

/// Tunables for the streaming emulation. All durations in milliseconds.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Re-run recognition after this much new audio arrives in an utterance.
    pub step_ms: u64,
    /// Longest window fed to the model; longer utterances are force-split.
    pub max_window_ms: u64,
    /// Overlap carried into the next utterance on a forced split.
    pub keep_ms: u64,
    /// Whisper needs at least one second; windows are zero-padded to this length.
    pub min_infer_ms: u64,
    /// Silence needed after speech before an utterance is finalized.
    pub hangover_ms: u64,
    /// Speech needed before `VoiceStarted` fires.
    pub onset_ms: u64,
    /// Audio included before the detected onset.
    pub preroll_ms: u64,
    /// Energy threshold in dBFS.
    pub vad_db: f32,
    pub threads: i32,
    pub no_context: bool,
    pub hint: Option<String>,
    pub audio_queue: usize,
    pub event_queue: usize,
    /// On stop mid-utterance, emit a Final carrying the last partial text.
    pub finalize_on_stop: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            step_ms: 500,
            max_window_ms: 10_000,
            keep_ms: 200,
            min_infer_ms: 1200,
            hangover_ms: 600,
            onset_ms: 60,
            preroll_ms: 200,
            vad_db: -40.0,
            threads: 4,
            no_context: true,
            hint: None,
            audio_queue: 500, // 10 s of 20 ms chunks
            event_queue: 256,
            finalize_on_stop: true,
        }
    }
}

impl EngineConfig {
    #[must_use]
    pub fn ms_to_samples(ms: u64) -> u64 {
        ms * u64::from(SAMPLE_RATE) / 1000
    }
}

/// Reliability counters, shared between feeder side and worker side.
#[derive(Debug, Default)]
pub struct Counters {
    pub pushed_chunks: AtomicU64,
    pub pushed_samples: AtomicU64,
    pub dropped_audio_chunks: AtomicU64,
    pub dropped_events: AtomicU64,
    pub gap_samples: AtomicU64,
    pub delayed_count: AtomicU64,
    pub hallucinations_dropped: AtomicU64,
    pub forced_splits: AtomicU64,
    pub full_calls: AtomicU64,
    pub full_time_us: AtomicU64,
    pub state_create_us: AtomicU64,
}

impl Counters {
    pub fn add(&self, field: &AtomicU64, n: u64) {
        field.fetch_add(n, Ordering::Relaxed);
    }

    #[must_use]
    pub fn get(field: &AtomicU64) -> u64 {
        field.load(Ordering::Relaxed)
    }
}

pub trait SpeechEngine {
    /// Starts a new session and returns its id. A previous session, if any,
    /// is stopped first; its late events remain observable via `poll`.
    fn start(&mut self) -> anyhow::Result<SessionId>;
    /// Never blocks. Returns `QueueFull` when the bounded audio queue is full;
    /// the caller decides whether to drop or retry.
    fn push_audio(&mut self, chunk: AudioChunk) -> Result<(), PushError>;
    /// Drains up to `limit` events, oldest sessions first.
    fn poll(&mut self, limit: usize) -> Vec<SpeechEvent>;
    /// Requests the current session to stop. Does not wait.
    fn stop(&mut self);
    /// True once every worker has exited and all events were polled.
    fn is_drained(&mut self) -> bool;
}
