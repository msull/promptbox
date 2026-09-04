//! `SpeechEngine` implementation backed by whisper.cpp. One worker thread per
//! session; bounded channels in both directions; stopped sessions keep
//! draining so late events remain observable with their old session id.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::time::Instant;

use anyhow::Context;
use whisper_rs::{WhisperContext, WhisperContextParameters};

use crate::adapters::speech::worker::{Msg, Worker};
use crate::ports::engine::{AudioChunk, Counters, EngineConfig, PushError, SpeechEngine};
use crate::ports::speech::{SessionId, SpeechEvent};

struct Session {
    tx: SyncSender<Msg>,
    rx: Receiver<SpeechEvent>,
}

pub struct WhisperEngine {
    ctx: Arc<WhisperContext>,
    cfg: EngineConfig,
    counters: Arc<Counters>,
    next_session: SessionId,
    current: Option<Session>,
    draining: Vec<Receiver<SpeechEvent>>,
    /// Events pulled while checking drain state; returned by the next `poll`.
    pending: std::collections::VecDeque<SpeechEvent>,
    pub load_ms: f64,
}

impl WhisperEngine {
    pub fn load(model: &Path, cfg: EngineConfig) -> anyhow::Result<Self> {
        let t = Instant::now();
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);
        let path = model
            .to_str()
            .with_context(|| format!("non-utf8 model path {}", model.display()))?;
        let ctx = WhisperContext::new_with_params(path, params)
            .with_context(|| format!("load model {}", model.display()))?;
        // Warm up: the first inference compiles Metal pipelines and can
        // take seconds; do it here rather than on the user's first words.
        {
            let mut state = ctx.create_state().context("create warm-up state")?;
            let mut p =
                whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
            p.set_language(Some("en"));
            p.set_n_threads(cfg.threads);
            p.set_print_special(false);
            p.set_print_progress(false);
            p.set_print_realtime(false);
            p.set_print_timestamps(false);
            let silence = vec![0.0f32; 16_000 * 3];
            let t = Instant::now();
            state.full(p, &silence).context("warm-up inference")?;
            log::info!(
                "whisper warm-up took {:.0} ms",
                t.elapsed().as_secs_f64() * 1000.0
            );
        }
        Ok(Self {
            ctx: Arc::new(ctx),
            cfg,
            counters: Arc::new(Counters::default()),
            next_session: 1,
            current: None,
            draining: Vec::new(),
            pending: std::collections::VecDeque::new(),
            load_ms: t.elapsed().as_secs_f64() * 1000.0,
        })
    }

    #[must_use]
    pub fn counters(&self) -> Arc<Counters> {
        Arc::clone(&self.counters)
    }

    fn drain_one(rx: &Receiver<SpeechEvent>, out: &mut Vec<SpeechEvent>, limit: usize) -> bool {
        while out.len() < limit {
            match rx.try_recv() {
                Ok(ev) => out.push(ev),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
        false
    }
}

impl SpeechEngine for WhisperEngine {
    fn start(&mut self) -> anyhow::Result<SessionId> {
        self.stop();
        let id = self.next_session;
        self.next_session += 1;
        let (audio_tx, audio_rx) = sync_channel::<Msg>(self.cfg.audio_queue);
        let (event_tx, event_rx) = sync_channel::<SpeechEvent>(self.cfg.event_queue);
        Worker::spawn(
            &self.ctx,
            self.cfg.clone(),
            id,
            audio_rx,
            event_tx,
            Arc::clone(&self.counters),
        )?;
        self.current = Some(Session {
            tx: audio_tx,
            rx: event_rx,
        });
        Ok(id)
    }

    fn push_audio(&mut self, chunk: AudioChunk) -> Result<(), PushError> {
        let Some(s) = &self.current else {
            return Err(PushError::NotRunning);
        };
        let n = chunk.samples.len() as u64;
        match s.tx.try_send(Msg::Audio(chunk)) {
            Ok(()) => {
                self.counters.add(&self.counters.pushed_chunks, 1);
                self.counters.add(&self.counters.pushed_samples, n);
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(PushError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(PushError::NotRunning),
        }
    }

    fn poll(&mut self, limit: usize) -> Vec<SpeechEvent> {
        let mut out = Vec::new();
        while out.len() < limit {
            match self.pending.pop_front() {
                Some(ev) => out.push(ev),
                None => break,
            }
        }
        if out.len() >= limit {
            return out;
        }
        // Oldest sessions first so late events keep their causal order.
        self.draining
            .retain(|rx| !Self::drain_one(rx, &mut out, limit));
        if let Some(s) = &self.current {
            Self::drain_one(&s.rx, &mut out, limit);
        }
        out
    }

    fn stop(&mut self) {
        if let Some(s) = self.current.take() {
            // Prompt stop if the queue has room; otherwise dropping the
            // sender disconnects the worker after it drains queued audio.
            let _ = s.tx.try_send(Msg::Stop);
            drop(s.tx);
            self.draining.push(s.rx);
        }
    }

    fn is_drained(&mut self) -> bool {
        let mut sink = Vec::new();
        self.draining
            .retain(|rx| !Self::drain_one(rx, &mut sink, usize::MAX));
        let quiet = sink.is_empty();
        self.pending.extend(sink);
        quiet && self.current.is_none() && self.draining.is_empty()
    }
}
