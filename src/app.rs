//! Orchestration: owns [`AppCore`], the port adapters, the recognizer, and
//! the microphone. Maps effects to adapter calls and feeds their results
//! back as actions. Rendering lives in [`crate::ui`].

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use crate::adapters::audio::MicCapture;
use crate::adapters::clipboard::SystemClipboard;
use crate::adapters::fake_speech::{DEMO_SCRIPT, FakeDictation};
use crate::adapters::model::{self, DEFAULT_MODEL, Download};
use crate::adapters::persistence::FileStore;
use crate::adapters::speech::WhisperEngine;
use crate::core::action::RECENT_LIMIT;
use crate::core::{AppAction, AppCore, Clock, Effect};
use crate::ports::clipboard::Clipboard;
use crate::ports::engine::{AudioChunk, EngineConfig, PushError, SpeechEngine};
use crate::ports::history::HistoryStore;
use crate::ports::speech::SpeechEventKind;

/// Audio the app will hold for a slow engine before giving up (30 s).
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

pub struct PromptBoxApp {
    core: AppCore,
    clipboard: Box<dyn Clipboard>,
    history: Box<dyn HistoryStore>,
    started: Instant,
    /// Added to the monotonic clock; tests use it to skip ahead.
    time_offset: Duration,
    demo: Option<FakeDictation>,
    next_demo_session: u64,
    recognizer: Recognizer,
    /// Set when the user asked to start before the engine finished loading.
    start_when_ready: bool,
    live: Option<LiveSession>,
    /// Stop requested; polling the engine until its last events drain.
    stopping: bool,
    model_path: PathBuf,
    download: Option<Download>,
    live_chunks_seen: u64,
    /// Chunks the engine could not accept yet; retried next frame.
    backlog: std::collections::VecDeque<AudioChunk>,
}

impl PromptBoxApp {
    /// Production wiring: system clipboard and the platform data directory.
    #[must_use]
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_services(
            Box::new(SystemClipboard::default()),
            Box::new(FileStore::new(FileStore::default_dir())),
        )
    }

    /// Wires explicit adapters (tests use fakes) and loads persisted state.
    #[must_use]
    pub fn with_services(
        clipboard: Box<dyn Clipboard>,
        mut history: Box<dyn HistoryStore>,
    ) -> Self {
        let recent = history.load_recent(RECENT_LIMIT);
        let draft = history.load_draft();
        let mut app = Self {
            core: AppCore::new(),
            clipboard,
            history,
            started: Instant::now(),
            time_offset: Duration::ZERO,
            demo: None,
            next_demo_session: 1_000_000,
            recognizer: Recognizer::NotLoaded,
            start_when_ready: false,
            live: None,
            stopping: false,
            model_path: model::model_path(DEFAULT_MODEL),
            download: None,
            live_chunks_seen: 0,
            backlog: std::collections::VecDeque::new(),
        };
        app.dispatch(AppAction::RecentLoaded(recent));
        app.dispatch(AppAction::DraftLoaded(draft));
        app
    }

    #[must_use]
    pub fn core(&self) -> &AppCore {
        &self.core
    }

    #[must_use]
    pub fn is_demo_running(&self) -> bool {
        self.demo.is_some()
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live.is_some() || self.stopping
    }

    #[must_use]
    pub fn recognizer(&self) -> &Recognizer {
        &self.recognizer
    }

    #[must_use]
    pub fn model_path(&self) -> &PathBuf {
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

    /// Skips the app clock forward (tests only; the UI never calls this).
    pub fn advance_time(&mut self, by: Duration) {
        self.time_offset += by;
    }

    fn clock(&self) -> Clock {
        Clock {
            mono: self.started.elapsed() + self.time_offset,
            wall: SystemTime::now(),
        }
    }

    /// The single entry point for every user, speech, or timer action.
    pub fn dispatch(&mut self, action: AppAction) {
        let now = self.clock();
        let effects = self.core.dispatch(action, now);
        for effect in effects {
            let result = match effect {
                Effect::WriteClipboard(text) => {
                    AppAction::ClipboardWriteFinished(self.clipboard.write_text(&text))
                }
                Effect::SaveHistory(prompt) => AppAction::HistorySaveFinished {
                    id: prompt.id,
                    result: self.history.save_sent(&prompt),
                },
                Effect::SaveDraft(text) => {
                    AppAction::DraftSaveFinished(self.history.save_draft(&text))
                }
            };
            self.dispatch(result);
        }
    }

    // ---- live dictation ---------------------------------------------

    /// Loads the model on a thread if needed, then opens the microphone.
    pub fn start_listening(&mut self) {
        if self.is_live() || self.demo.is_some() {
            return;
        }
        if !self.model_present() {
            self.dispatch(AppAction::EngineUnavailable(format!(
                "No speech model at {}. Download it below.",
                self.model_path.display()
            )));
            return;
        }
        match &self.recognizer {
            Recognizer::Ready(_) => self.open_microphone(),
            Recognizer::Loading(_) => self.start_when_ready = true,
            Recognizer::NotLoaded | Recognizer::Failed(_) => {
                self.start_when_ready = true;
                self.load_engine();
            }
        }
    }

    fn load_engine(&mut self) {
        let (tx, rx) = channel();
        let path = self.model_path.clone();
        let hint = self.vocabulary_hint();
        std::thread::Builder::new()
            .name("whisper-load".into())
            .spawn(move || {
                let cfg = EngineConfig {
                    hint,
                    ..EngineConfig::default()
                };
                let _ = tx.send(WhisperEngine::load(&path, cfg).map_err(|e| format!("{e:#}")));
            })
            .expect("spawn whisper load thread");
        self.recognizer = Recognizer::Loading(rx);
    }

    fn vocabulary_hint(&self) -> Option<String> {
        let p = &self.core.projects()[self.core.selected_project()];
        (!p.vocabulary.is_empty()).then(|| p.vocabulary.join(", "))
    }

    fn open_microphone(&mut self) {
        let Recognizer::Ready(engine) = &mut self.recognizer else {
            return;
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
                    self.dispatch(AppAction::SessionStarted(session));
                }
                Err(e) => {
                    log::warn!("engine start failed: {e:#}");
                    self.dispatch(AppAction::EngineUnavailable(format!("{e:#}")));
                }
            },
            Err(e) => {
                log::warn!("microphone open failed: {e}");
                self.dispatch(AppAction::EngineUnavailable(format!("Microphone: {e}")));
            }
        }
    }

    /// Closes the microphone now; the session ends once the engine has
    /// emitted its final events (see `pump`).
    pub fn stop_listening(&mut self) {
        if self.live.take().is_some() {
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
            self.dispatch(AppAction::SessionStopping);
        }
    }

    pub fn start_download(&mut self) {
        if self.download.is_none() && !self.model_present() {
            self.download = Some(Download::start(DEFAULT_MODEL));
        }
    }

    // ---- demo ---------------------------------------------------------

    /// Starts scripted dictation as a new session. `with_gap` injects an
    /// audio gap so the degraded state can be seen.
    pub fn start_demo(&mut self, with_gap: bool) {
        if self.is_live() {
            return;
        }
        self.stop_demo();
        let session = self.next_demo_session;
        self.next_demo_session += 1;
        self.demo = Some(FakeDictation::new(
            session,
            DEMO_SCRIPT,
            self.clock().mono,
            with_gap,
        ));
        self.dispatch(AppAction::SessionStarted(session));
    }

    pub fn stop_demo(&mut self) {
        if self.demo.take().is_some() {
            self.dispatch(AppAction::SessionStopped);
        }
    }

    // ---- per-frame ----------------------------------------------------

    /// Called once per frame: moves audio into the engine, drains speech
    /// events, ticks the clock. Returns how soon a repaint is wanted.
    pub fn pump(&mut self) -> Option<Duration> {
        self.pump_recognizer_load();
        self.pump_download();
        self.pump_live();
        self.pump_demo();
        self.dispatch(AppAction::Tick);
        if self.is_live() || self.demo.is_some() || self.download.is_some() {
            return Some(Duration::from_millis(20));
        }
        if matches!(self.recognizer, Recognizer::Loading(_)) {
            return Some(Duration::from_millis(100));
        }
        self.core
            .toast()
            .map(|t| t.expires_at.saturating_sub(self.clock().mono) + Duration::from_millis(10))
    }

    fn pump_recognizer_load(&mut self) {
        let Recognizer::Loading(rx) = &self.recognizer else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(engine)) => {
                self.recognizer = Recognizer::Ready(Box::new(engine));
                if std::mem::take(&mut self.start_when_ready) {
                    self.open_microphone();
                }
            }
            Ok(Err(e)) => {
                log::warn!("model load failed: {e}");
                self.recognizer = Recognizer::Failed(e.clone());
                self.start_when_ready = false;
                self.dispatch(AppAction::EngineUnavailable(format!(
                    "Model failed to load: {e}"
                )));
            }
            Err(_) => {}
        }
    }

    fn pump_download(&mut self) {
        let Some(d) = &self.download else { return };
        match d.try_result() {
            Some(Ok(_)) => {
                self.download = None;
                self.dispatch(AppAction::AcknowledgeStatus);
            }
            Some(Err(e)) => {
                self.download = None;
                self.dispatch(AppAction::EngineUnavailable(format!(
                    "Download failed: {e}"
                )));
            }
            None => {}
        }
    }

    fn pump_live(&mut self) {
        let mic_error = self
            .live
            .as_ref()
            .and_then(|l| l.mic.stats().error.lock().expect("mic mutex").take());
        if let Some(e) = mic_error {
            self.stop_listening();
            self.dispatch(AppAction::EngineUnavailable(format!(
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
            if self.core.doc().is_empty() || level > crate::core::action::VOICE_DB {
                log::trace!(
                    "mic level {level:.0} dBFS, {} chunks",
                    self.live_chunks_seen
                );
            }
            self.live_chunks_seen += n_chunks;
            self.dispatch(AppAction::AudioLevel(level));
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
                self.dispatch(AppAction::SpeechEventReceived(ev));
            }
            if drained {
                self.stopping = false;
                self.dispatch(AppAction::SessionStopped);
            }
        }
    }

    fn pump_demo(&mut self) {
        let now = self.clock().mono;
        if let Some(demo) = &mut self.demo {
            let events = demo.poll(now);
            let finished = demo.is_finished();
            for ev in events {
                self.dispatch(AppAction::SpeechEventReceived(ev));
            }
            if finished {
                self.stop_demo();
            }
        }
    }
}

impl eframe::App for PromptBoxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(delay) = self.pump() {
            ui.ctx().request_repaint_after(delay);
        }
        crate::ui::draw(self, ui);
    }
}
