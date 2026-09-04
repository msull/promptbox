//! Feeds WAV audio into an engine either paced in real time or as fast as
//! the engine accepts it, recording a timeline of events with latencies.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::audio::WavAudio;
use crate::engine::{AudioChunk, PushError, SAMPLE_RATE, SpeechEngine};
use crate::events::{SessionId, SpeechEvent};

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    /// Wall time since the feed started.
    pub wall_ms: f64,
    /// Total audio pushed so far (all sessions), in ms.
    pub audio_ms: f64,
    /// Wall time between pushing the last sample the event refers to and
    /// observing the event. `None` for events with no audio anchor.
    pub latency_ms: Option<f64>,
    pub event: SpeechEvent,
}

#[derive(Debug, Clone)]
pub struct FeedOptions {
    pub chunk_ms: u64,
    pub realtime: bool,
    /// Stop the session once this much audio (ms) has been pushed.
    pub stop_after_ms: Option<u64>,
    /// After stopping, start a new session and keep feeding.
    pub restart: bool,
    pub drain_timeout: Duration,
}

#[derive(Debug)]
pub struct FeedResult {
    pub timeline: Vec<TimelineEntry>,
    pub wall_secs: f64,
    pub sessions: Vec<SessionId>,
    pub dropped_chunks: u64,
    pub drain_timed_out: bool,
}

struct Recorder {
    t0: Instant,
    push_times: HashMap<SessionId, Vec<(u64, Instant)>>,
    pushed_samples: u64,
    timeline: Vec<TimelineEntry>,
}

impl Recorder {
    fn record(&mut self, ev: SpeechEvent) {
        let now = Instant::now();
        let latency_ms = self.push_times.get(&ev.session).and_then(|times| {
            let end = ev.audio_range.end;
            if ev.audio_range.is_empty() && end == 0 {
                return None;
            }
            times
                .iter()
                .find(|(e, _)| *e >= end)
                .map(|(_, t)| now.duration_since(*t).as_secs_f64() * 1000.0)
        });
        self.timeline.push(TimelineEntry {
            wall_ms: now.duration_since(self.t0).as_secs_f64() * 1000.0,
            audio_ms: self.pushed_samples as f64 * 1000.0 / f64::from(SAMPLE_RATE),
            latency_ms,
            event: ev,
        });
    }

    fn poll(&mut self, engine: &mut dyn SpeechEngine) {
        for ev in engine.poll(64) {
            self.record(ev);
        }
    }
}

pub fn feed(
    engine: &mut dyn SpeechEngine,
    audio: &WavAudio,
    opts: &FeedOptions,
) -> anyhow::Result<FeedResult> {
    let t0 = Instant::now();
    let mut rec = Recorder {
        t0,
        push_times: HashMap::new(),
        pushed_samples: 0,
        timeline: Vec::new(),
    };
    let mut sessions = Vec::new();
    let mut session = engine.start()?;
    sessions.push(session);
    let mut base = 0u64;
    let mut dropped = 0u64;
    let stop_at = opts
        .stop_after_ms
        .map(|ms| ms * u64::from(SAMPLE_RATE) / 1000);
    let mut stopped_once = false;

    for chunk in audio.chunks(opts.chunk_ms) {
        if let Some(stop_at) = stop_at
            && !stopped_once
            && chunk.start_sample >= stop_at
        {
            stopped_once = true;
            engine.stop();
            rec.poll(engine);
            if !opts.restart {
                break;
            }
            session = engine.start()?;
            sessions.push(session);
            base = chunk.start_sample;
        }
        if opts.realtime {
            let due =
                t0 + Duration::from_secs_f64(chunk.start_sample as f64 / f64::from(SAMPLE_RATE));
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        let local = AudioChunk {
            start_sample: chunk.start_sample - base,
            samples: chunk.samples,
        };
        let n = local.samples.len() as u64;
        let end = local.end_sample();
        loop {
            match engine.push_audio(local.clone()) {
                Ok(()) => {
                    rec.pushed_samples += n;
                    rec.push_times
                        .entry(session)
                        .or_default()
                        .push((end, Instant::now()));
                    break;
                }
                Err(PushError::QueueFull) if opts.realtime => {
                    dropped += 1;
                    break;
                }
                Err(PushError::QueueFull) => {
                    rec.poll(engine);
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(PushError::NotRunning) => anyhow::bail!("engine not running"),
            }
        }
        rec.poll(engine);
    }

    engine.stop();
    let deadline = Instant::now() + opts.drain_timeout;
    let mut timed_out = false;
    loop {
        rec.poll(engine);
        if engine.is_drained() {
            rec.poll(engine);
            break;
        }
        if Instant::now() > deadline {
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    Ok(FeedResult {
        timeline: rec.timeline,
        wall_secs: t0.elapsed().as_secs_f64(),
        sessions,
        dropped_chunks: dropped,
        drain_timed_out: timed_out,
    })
}
