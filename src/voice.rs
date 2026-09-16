//! The voice runtime: the whisper engine, the microphone, the audio
//! backlog, the demo script, and the caption overlay's state. There is
//! one per process because there is one microphone and one loaded
//! model, however many editors a host keeps. It never touches a
//! document: every method hands back the [`AppAction`]s the bound
//! editor should receive, and the host dispatches them.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::adapters::audio::MicCapture;
use crate::adapters::fake_speech::{DEMO_SCRIPT, FakeDictation};
use crate::adapters::model::{self, DEFAULT_MODEL, Download};
use crate::adapters::speech::WhisperEngine;
use crate::core::{AppAction, SessionStatus};
use crate::ports::engine::{AudioChunk, EngineConfig, PushError, SpeechEngine};
use crate::ports::speech::SpeechEventKind;

/// Audio the runtime will hold for a slow engine before giving up (30 s).
const BACKLOG_LIMIT_CHUNKS: usize = 1500;

/// Where the whisper engine is in its lifecycle. Loading happens on a
/// thread because a first Metal shader compile can take seconds.
pub enum Recognizer {
    NotLoaded,
    Loading(Receiver<Result<WhisperEngine, String>>),
    Ready(Box<WhisperEngine>),
    Failed(String),
}

struct LiveSession {
    mic: MicCapture,
    audio_rx: Receiver<AudioChunk>,
}

/// What one frame of pumping produced.
#[derive(Default)]
pub struct Pumped {
    /// Speech events and status changes for the bound editor, in order.
    pub actions: Vec<AppAction>,
    /// How soon the runtime wants another frame, if at all.
    pub repaint: Option<Duration>,
}

#[allow(clippy::struct_excessive_bools)] // independent lifecycle flags
pub struct Voice {
    recognizer: Recognizer,
    /// Set when the user asked to start before the engine finished loading.
    start_when_ready: bool,
    /// The vocabulary hint for the next engine load.
    hint: String,
    live: Option<LiveSession>,
    /// Stop requested; polling the engine until its last events drain.
    stopping: bool,
    model_path: std::path::PathBuf,
    download: Option<Download>,
    live_chunks_seen: u64,
    /// Chunks the engine could not accept yet; retried next frame.
    backlog: VecDeque<AudioChunk>,
    demo: Option<FakeDictation>,
    next_demo_session: u64,
    started: Instant,
    /// Added to the monotonic clock; tests use it to skip ahead.
    time_offset: Duration,
    /// Text and timing of the on-screen caption overlay.
    pub caption: crate::caption::CaptionState,
    captions: bool,
    /// Whether the Dock icon currently shows the recording badge.
    dock_badge_shown: bool,
}

impl Default for Voice {
    fn default() -> Self {
        Self::new(model::model_path(DEFAULT_MODEL))
    }
}

impl Voice {
    #[must_use]
    pub fn new(model_path: std::path::PathBuf) -> Self {
        Self {
            recognizer: Recognizer::NotLoaded,
            start_when_ready: false,
            hint: String::new(),
            live: None,
            stopping: false,
            model_path,
            download: None,
            live_chunks_seen: 0,
            backlog: VecDeque::new(),
            demo: None,
            next_demo_session: 1_000_000,
            started: Instant::now(),
            time_offset: Duration::ZERO,
            caption: crate::caption::CaptionState::default(),
            captions: true,
            dock_badge_shown: false,
        }
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live.is_some() || self.stopping
    }

    #[must_use]
    pub fn is_demo_running(&self) -> bool {
        self.demo.is_some()
    }

    #[must_use]
    pub fn recognizer(&self) -> &Recognizer {
        &self.recognizer
    }

    #[must_use]
    pub fn model_path(&self) -> &std::path::PathBuf {
        &self.model_path
    }

    #[must_use]
    pub fn model_present(&self) -> bool {
        self.model_path.exists()
    }

    #[must_use]
    pub fn download(&self) -> Option<&Download> {
        self.download.as_ref()
    }

    pub fn start_download(&mut self) {
        if self.download.is_none() && !self.model_present() {
            self.download = Some(Download::start(DEFAULT_MODEL));
        }
    }

    #[must_use]
    pub fn captions_enabled(&self) -> bool {
        self.captions
    }

    /// Shows or hides the on-screen caption overlay.
    pub fn set_captions_enabled(&mut self, on: bool) {
        self.captions = on;
    }

    /// Skips the runtime's clock forward (tests only).
    pub fn advance_time(&mut self, by: Duration) {
        self.time_offset += by;
    }

    fn mono(&self) -> Duration {
        self.started.elapsed() + self.time_offset
    }

    // ---- live dictation ---------------------------------------------

    /// Loads the model on a thread if needed, then opens the microphone.
    /// `hint` primes the recognizer with the bound editor's vocabulary
    /// (read once, when the engine loads).
    pub fn start(&mut self, hint: String) -> Vec<AppAction> {
        if self.is_live() || self.demo.is_some() {
            return Vec::new();
        }
        if !self.model_present() {
            return vec![AppAction::EngineUnavailable(format!(
                "No speech model at {}. Download it below.",
                self.model_path.display()
            ))];
        }
        self.hint = hint;
        match &self.recognizer {
            Recognizer::Ready(_) => self.open_microphone(),
            Recognizer::Loading(_) => {
                self.start_when_ready = true;
                Vec::new()
            }
            Recognizer::NotLoaded | Recognizer::Failed(_) => {
                self.start_when_ready = true;
                self.load_engine();
                Vec::new()
            }
        }
    }

    fn load_engine(&mut self) {
        let (tx, rx) = channel();
        let path = self.model_path.clone();
        let hint = self.hint.clone();
        std::thread::Builder::new()
            .name("whisper-load".into())
            .spawn(move || {
                let cfg = EngineConfig {
                    hint: Some(hint),
                    ..EngineConfig::default()
                };
                let _ = tx.send(WhisperEngine::load(&path, cfg).map_err(|e| format!("{e:#}")));
            })
            .expect("spawn whisper load thread");
        self.recognizer = Recognizer::Loading(rx);
    }

    fn open_microphone(&mut self) -> Vec<AppAction> {
        let Recognizer::Ready(engine) = &mut self.recognizer else {
            return Vec::new();
        };
        let capture = match std::env::var_os("PROMPTBOX_FAKE_MIC") {
            Some(path) => MicCapture::from_wav(std::path::Path::new(&path)),
            None => MicCapture::start(),
        };
        match capture {
            Ok((mic, audio_rx)) => match engine.start() {
                Ok(session) => {
                    log::info!("listening on {}", mic.device_name);
                    self.live = Some(LiveSession { mic, audio_rx });
                    vec![AppAction::SessionStarted(session)]
                }
                Err(e) => {
                    log::warn!("engine start failed: {e:#}");
                    vec![AppAction::EngineUnavailable(format!("{e:#}"))]
                }
            },
            Err(e) => {
                log::warn!("microphone open failed: {e}");
                vec![AppAction::EngineUnavailable(format!("Microphone: {e}"))]
            }
        }
    }

    /// Closes the microphone now; the session ends once the engine has
    /// emitted its final events (see [`Self::pump`]).
    pub fn stop(&mut self) -> Vec<AppAction> {
        if self.live.take().is_none() {
            return Vec::new();
        }
        if let Recognizer::Ready(engine) = &mut self.recognizer {
            // Flush what we can before asking the worker to finish.
            while let Some(c) = self.backlog.pop_front() {
                if engine.push_audio(c).is_err() {
                    break;
                }
            }
            self.backlog.clear();
            engine.stop();
        }
        self.stopping = true;
        vec![AppAction::SessionStopping]
    }

    // ---- demo ---------------------------------------------------------

    /// Starts scripted dictation as a new session. `with_gap` injects an
    /// audio gap so the degraded state can be seen.
    pub fn start_demo(&mut self, with_gap: bool) -> Vec<AppAction> {
        if self.is_live() {
            return Vec::new();
        }
        let mut actions = self.stop_demo();
        let session = self.next_demo_session;
        self.next_demo_session += 1;
        self.demo = Some(FakeDictation::new(
            session,
            DEMO_SCRIPT,
            self.mono(),
            with_gap,
        ));
        actions.push(AppAction::SessionStarted(session));
        actions
    }

    pub fn stop_demo(&mut self) -> Vec<AppAction> {
        if self.demo.take().is_some() {
            vec![AppAction::SessionStopped]
        } else {
            Vec::new()
        }
    }

    // ---- per-frame ----------------------------------------------------

    /// Once per frame: finishes an engine load, moves audio into the
    /// engine, drains speech events, advances the demo. The actions go to
    /// the bound editor; `repaint` says how soon to come back.
    pub fn pump(&mut self) -> Pumped {
        let mut actions = Vec::new();
        self.pump_recognizer_load(&mut actions);
        self.pump_download(&mut actions);
        self.pump_live(&mut actions);
        self.pump_demo(&mut actions);
        let repaint = if self.is_live() || self.demo.is_some() || self.download.is_some() {
            Some(Duration::from_millis(20))
        } else if matches!(self.recognizer, Recognizer::Loading(_)) {
            Some(Duration::from_millis(100))
        } else {
            None
        };
        Pumped { actions, repaint }
    }

    fn pump_recognizer_load(&mut self, actions: &mut Vec<AppAction>) {
        let Recognizer::Loading(rx) = &self.recognizer else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(engine)) => {
                self.recognizer = Recognizer::Ready(Box::new(engine));
                if std::mem::take(&mut self.start_when_ready) {
                    actions.extend(self.open_microphone());
                }
            }
            Ok(Err(e)) => {
                log::warn!("model load failed: {e}");
                self.recognizer = Recognizer::Failed(e.clone());
                self.start_when_ready = false;
                actions.push(AppAction::EngineUnavailable(format!(
                    "Model failed to load: {e}"
                )));
            }
            Err(_) => {}
        }
    }

    fn pump_download(&mut self, actions: &mut Vec<AppAction>) {
        let Some(d) = &self.download else { return };
        match d.try_result() {
            Some(Ok(_)) => {
                self.download = None;
                actions.push(AppAction::AcknowledgeStatus);
            }
            Some(Err(e)) => {
                self.download = None;
                actions.push(AppAction::EngineUnavailable(format!(
                    "Download failed: {e}"
                )));
            }
            None => {}
        }
    }

    fn pump_live(&mut self, actions: &mut Vec<AppAction>) {
        let mic_error = self
            .live
            .as_ref()
            .and_then(|l| l.mic.stats().error.lock().expect("mic mutex").take());
        if let Some(e) = mic_error {
            actions.extend(self.stop());
            actions.push(AppAction::EngineUnavailable(format!(
                "Microphone error: {e}"
            )));
            return;
        }
        if let Some(live) = &self.live {
            let level = live.mic.stats().level_db();
            let mut chunks = Vec::new();
            while let Ok(c) = live.audio_rx.try_recv() {
                chunks.push(c);
            }
            let n_chunks = chunks.len() as u64;
            self.backlog.extend(chunks);
            if let Recognizer::Ready(engine) = &mut self.recognizer {
                while let Some(c) = self.backlog.pop_front() {
                    if let Err(PushError::QueueFull) = engine.push_audio(c.clone()) {
                        self.backlog.push_front(c);
                        break;
                    }
                }
                // Only give up if the engine has been stuck for a long time;
                // the resulting offset jump surfaces as AudioGap.
                if self.backlog.len() > BACKLOG_LIMIT_CHUNKS {
                    let drop = self.backlog.len() - BACKLOG_LIMIT_CHUNKS;
                    log::warn!("engine stuck; dropping {drop} chunks of audio");
                    self.backlog.drain(..drop);
                }
            }
            if level > crate::core::action::VOICE_DB {
                log::trace!(
                    "mic level {level:.0} dBFS, {} chunks",
                    self.live_chunks_seen
                );
            }
            self.live_chunks_seen += n_chunks;
            actions.push(AppAction::AudioLevel(level));
        }
        if self.live.is_some() || self.stopping {
            let mut events = Vec::new();
            let mut drained = false;
            if let Recognizer::Ready(engine) = &mut self.recognizer {
                events = engine.poll(64);
                if self.stopping && events.is_empty() {
                    drained = engine.is_drained();
                }
            }
            for ev in events {
                match &ev.kind {
                    SpeechEventKind::Partial { .. } | SpeechEventKind::VoiceEnded { .. } => {
                        log::debug!("speech {}", ev.label());
                    }
                    _ => log::info!("speech {}", ev.label()),
                }
                actions.push(AppAction::SpeechEventReceived(ev));
            }
            if drained {
                self.stopping = false;
                actions.push(AppAction::SessionStopped);
            }
        }
    }

    fn pump_demo(&mut self, actions: &mut Vec<AppAction>) {
        let now = self.mono();
        if let Some(demo) = &mut self.demo {
            let events = demo.poll(now);
            let finished = demo.is_finished();
            for ev in events {
                actions.push(AppAction::SpeechEventReceived(ev));
            }
            if finished {
                actions.extend(self.stop_demo());
            }
        }
    }

    /// Keeps the Dock badge in step with whether audio is being captured,
    /// judged by the bound editor's status. Only touches `AppKit` when
    /// the state actually changes.
    pub fn sync_dock_badge(&mut self, status: &SessionStatus) {
        let recording = matches!(status, SessionStatus::Listening | SessionStatus::Finishing);
        if recording != self.dock_badge_shown {
            crate::adapters::dock::set_recording_badge(recording);
            self.dock_badge_shown = recording;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_without_a_model_reports_it_and_stays_idle() {
        let mut voice = Voice::new(std::path::PathBuf::from("/nowhere/ggml-none.bin"));
        let actions = voice.start("hint".into());
        assert!(
            matches!(&actions[..], [AppAction::EngineUnavailable(m)] if m.contains("No speech model"))
        );
        assert!(!voice.is_live());
        assert!(voice.stop().is_empty(), "nothing to stop");
    }

    #[test]
    fn the_demo_starts_and_stops_as_a_session() {
        let mut voice = Voice::new(std::path::PathBuf::from("/nowhere/ggml-none.bin"));
        let started = voice.start_demo(false);
        assert!(matches!(&started[..], [AppAction::SessionStarted(_)]));
        assert!(voice.is_demo_running());
        assert!(
            voice.start("hint".into()).is_empty(),
            "no mic while the demo runs"
        );
        let stopped = voice.stop_demo();
        assert!(matches!(&stopped[..], [AppAction::SessionStopped]));
        assert!(voice.stop_demo().is_empty());
    }
}
