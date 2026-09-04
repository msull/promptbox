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
use crate::adapters::openai::{self, OpenAiRewriter};
use crate::adapters::persistence::FileStore;
use crate::adapters::speech::WhisperEngine;
use crate::adapters::typist::SystemTypist;
use crate::core::action::RECENT_LIMIT;
use crate::core::action::TypingPolicy;
use crate::core::{AppAction, AppCore, Clock, Effect};
use crate::ports::ai::{RewriteResponse, Rewriter};
use crate::ports::clipboard::Clipboard;
use crate::ports::engine::{AudioChunk, EngineConfig, PushError, SpeechEngine};
use crate::ports::history::{HistoryStore, Settings, ThemeChoice};
use crate::ports::speech::SpeechEventKind;
use crate::ports::typist::Typist;

/// Compact "docked" window size in points: the compact top bar, the
/// bottom bar, roughly 200 px of prompt, and a little room below it.
pub const DOCK_SIZE: egui::Vec2 = egui::vec2(300.0, 330.0);
/// Gap between a docked window and the screen edge, in points.
const DOCK_MARGIN: f32 = 8.0;

/// Screen corners the dock button cycles through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopRight,
    BottomRight,
    BottomLeft,
    TopLeft,
}

impl Corner {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::TopRight => Self::BottomRight,
            Self::BottomRight => Self::BottomLeft,
            Self::BottomLeft => Self::TopLeft,
            Self::TopLeft => Self::TopRight,
        }
    }

    /// Top-left outer position for a window of `outer` size on a monitor
    /// of `monitor` size, inset by `margin`.
    #[must_use]
    pub fn position(self, monitor: egui::Vec2, outer: egui::Vec2, margin: f32) -> egui::Pos2 {
        let x = match self {
            Self::TopRight | Self::BottomRight => monitor.x - outer.x - margin,
            Self::BottomLeft | Self::TopLeft => margin,
        };
        let y = match self {
            Self::TopRight | Self::TopLeft => margin,
            Self::BottomRight | Self::BottomLeft => monitor.y - outer.y - margin,
        };
        egui::pos2(x.max(0.0), y.max(0.0))
    }
}

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

#[allow(clippy::struct_excessive_bools)] // independent lifecycle flags
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
    settings: Settings,
    /// The window level must be applied once a viewport exists.
    window_level_applied: bool,
    /// Corner the window was last docked to; `None` until first use.
    docked_corner: Option<Corner>,
    /// Whether the voice-command help popup is open.
    pub show_commands: bool,
    /// Whether the settings window is open.
    pub show_settings: bool,
    /// Text in the AI instruction box under the prompt.
    pub ai_instruction: String,
    /// Draft values in the settings window until saved.
    pub settings_draft: Settings,
    rewriter: Option<std::sync::Arc<dyn Rewriter>>,
    ai_rx: Option<Receiver<(u64, Result<RewriteResponse, String>)>>,
    typist: Box<dyn Typist>,
    /// Whether our own window is focused this frame (from egui).
    window_focused: bool,
}

impl PromptBoxApp {
    /// Production wiring: system clipboard and the platform data directory.
    #[must_use]
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::with_services(
            Box::new(SystemClipboard::default()),
            Box::new(FileStore::new(FileStore::default_dir())),
        );
        app.typist = Box::new(SystemTypist::default());
        app
    }

    /// Wires explicit adapters (tests use fakes) and loads persisted state.
    #[must_use]
    pub fn with_services(
        clipboard: Box<dyn Clipboard>,
        mut history: Box<dyn HistoryStore>,
    ) -> Self {
        let recent = history.load_recent(RECENT_LIMIT);
        let draft = history.load_draft();
        let settings = history.load_settings().unwrap_or_else(|e| {
            log::warn!("{e}; using default settings");
            Settings::default()
        });
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
            settings,
            window_level_applied: false,
            docked_corner: None,
            show_commands: false,
            show_settings: false,
            ai_instruction: String::new(),
            settings_draft: Settings::default(),
            rewriter: None,
            ai_rx: None,
            typist: Box::new(crate::adapters::typist::FakeTypist::default()),
            window_focused: true,
        };
        app.settings_draft = app.settings.clone();
        app.rebuild_rewriter();
        app.core.set_trigger(&app.settings.trigger);
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

    #[must_use]
    pub fn always_on_top(&self) -> bool {
        self.settings.always_on_top
    }

    /// Installs a typist directly (tests use a fake).
    pub fn set_typist(&mut self, typist: Box<dyn Typist>) {
        self.typist = typist;
    }

    #[must_use]
    pub fn typing_permission_granted(&self) -> bool {
        self.typist.permission_granted()
    }

    pub fn request_typing_permission(&self) {
        self.typist.request_permission();
    }

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Typing-related settings persist immediately, like the theme.
    pub fn set_type_on_send(&mut self, on: bool) {
        self.settings.type_on_send = on;
        self.settings_draft.type_on_send = on;
        self.persist_settings();
    }

    pub fn set_submit_after_paste(&mut self, on: bool) {
        self.settings.submit_after_paste = on;
        self.settings_draft.submit_after_paste = on;
        self.persist_settings();
    }

    fn persist_settings(&mut self) {
        if let Err(e) = self.history.save_settings(&self.settings) {
            log::warn!("could not save settings: {e}");
        }
    }

    /// Typing only makes sense when another app has focus; from our own
    /// window a Send would paste back into Prompt Box.
    fn typing_policy(&self) -> TypingPolicy {
        TypingPolicy {
            enabled: self.settings.type_on_send && !self.window_focused,
            submit: self.settings.submit_after_paste,
        }
    }

    /// Records whether our window is focused this frame.
    pub fn set_window_focused(&mut self, focused: bool) {
        self.window_focused = focused;
        self.core.set_typing_policy(self.typing_policy());
    }

    /// Installs a rewriter directly (tests use a fake).
    pub fn set_rewriter(&mut self, rewriter: std::sync::Arc<dyn Rewriter>) {
        self.rewriter = Some(rewriter);
    }

    /// Where the `OpenAI` key comes from, for the settings window.
    #[must_use]
    pub fn api_key_source(&self) -> &'static str {
        if !self.settings.openai_api_key.trim().is_empty() {
            "settings"
        } else if std::env::var_os("OPENAI_API_KEY").is_some() {
            "OPENAI_API_KEY environment variable"
        } else if openai::read_dotenv_key(std::path::Path::new(".env"), "OPENAI_API_KEY").is_some()
        {
            ".env file in the working directory"
        } else {
            "none"
        }
    }

    #[must_use]
    pub fn ai_available(&self) -> bool {
        self.rewriter.is_some()
    }

    fn resolve_api_key(&self) -> Option<String> {
        let from_settings = self.settings.openai_api_key.trim();
        if !from_settings.is_empty() {
            return Some(from_settings.to_owned());
        }
        std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| openai::read_dotenv_key(std::path::Path::new(".env"), "OPENAI_API_KEY"))
    }

    fn rebuild_rewriter(&mut self) {
        let model = if self.settings.openai_model.trim().is_empty() {
            openai::DEFAULT_MODEL.to_owned()
        } else {
            self.settings.openai_model.trim().to_owned()
        };
        self.rewriter = self.resolve_api_key().map(|key| {
            std::sync::Arc::new(OpenAiRewriter::new(key, model)) as std::sync::Arc<dyn Rewriter>
        });
    }

    /// Applies a theme to the egui context immediately.
    pub fn apply_theme(ctx: &egui::Context, theme: ThemeChoice) {
        ctx.set_theme(match theme {
            ThemeChoice::Auto => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        });
    }

    #[must_use]
    pub fn theme(&self) -> ThemeChoice {
        self.settings.theme
    }

    /// Applies and persists the appearance immediately; unlike the API key
    /// or model there is nothing to validate, so no Save step is needed.
    pub fn set_theme(&mut self, ctx: &egui::Context, theme: ThemeChoice) {
        self.settings.theme = theme;
        self.settings_draft.theme = theme;
        Self::apply_theme(ctx, theme);
        if let Err(e) = self.history.save_settings(&self.settings) {
            log::warn!("could not save settings: {e}");
        }
    }

    /// Saves the settings-window draft and reapplies anything it affects.
    pub fn save_settings_draft(&mut self) {
        self.settings = self.settings_draft.clone();
        self.core.set_trigger(&self.settings.trigger);
        self.rebuild_rewriter();
        match self.history.save_settings(&self.settings) {
            Ok(()) => log::info!("settings saved"),
            Err(e) => log::warn!("could not save settings: {e}"),
        }
    }

    /// Pins or unpins the window above others and remembers the choice.
    pub fn set_always_on_top(&mut self, ctx: &egui::Context, on: bool) {
        self.settings.always_on_top = on;
        self.window_level_applied = false;
        self.apply_window_level(ctx);
        if let Err(e) = self.history.save_settings(&self.settings) {
            log::warn!("could not save settings: {e}");
        }
    }

    #[must_use]
    pub fn docked_corner(&self) -> Option<Corner> {
        self.docked_corner
    }

    /// Shrinks the window to [`DOCK_SIZE`] and moves it to the next screen
    /// corner (top-right first). Positions come from the current monitor's
    /// size; on a secondary monitor egui does not expose the origin, so the
    /// window lands relative to the primary one.
    pub fn dock_next_corner(&mut self, ctx: &egui::Context) {
        let corner = self.docked_corner.map_or(Corner::TopRight, Corner::next);
        self.docked_corner = Some(corner);
        let (monitor, inner, outer) = ctx.input(|i| {
            let v = i.viewport();
            (v.monitor_size, v.inner_rect, v.outer_rect)
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(DOCK_SIZE));
        let Some(monitor) = monitor else {
            log::warn!("monitor size unknown; resized without moving");
            return;
        };
        // Decorations (title bar) are the difference between outer and inner.
        let chrome = match (inner, outer) {
            (Some(i), Some(o)) => o.size() - i.size(),
            _ => egui::vec2(0.0, 28.0),
        };
        let pos = corner.position(monitor, DOCK_SIZE + chrome, DOCK_MARGIN);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
    }

    fn apply_window_level(&mut self, ctx: &egui::Context) {
        if self.window_level_applied {
            return;
        }
        self.window_level_applied = true;
        // The saved theme also needs a live viewport, so apply it here.
        Self::apply_theme(ctx, self.settings.theme);
        let level = if self.settings.always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
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
                Effect::StopListening => {
                    self.stop_listening();
                    continue;
                }
                Effect::TypeIntoActiveApp { id, submit } => AppAction::TypeFinished {
                    id,
                    result: self.typist.paste_and_submit(submit),
                },
                Effect::AiRewrite(request) => {
                    let Some(rewriter) = self.rewriter.clone() else {
                        self.dispatch(AppAction::AiRewriteFinished {
                            id: request.id,
                            result: Err("no OpenAI API key; set one in Settings".into()),
                        });
                        continue;
                    };
                    let (tx, rx) = channel();
                    self.ai_rx = Some(rx);
                    std::thread::Builder::new()
                        .name("ai-rewrite".into())
                        .spawn(move || {
                            let t = Instant::now();
                            let result = rewriter.rewrite(&request);
                            match &result {
                                Ok(r) => log::info!(
                                    "ai rewrite: {} prompt + {} completion tokens in {:.1} s",
                                    r.prompt_tokens,
                                    r.completion_tokens,
                                    t.elapsed().as_secs_f64()
                                ),
                                Err(e) => log::warn!("ai rewrite failed: {e}"),
                            }
                            let _ = tx.send((request.id, result));
                        })
                        .expect("spawn ai thread");
                    continue;
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
                    hint: Some(hint),
                    ..EngineConfig::default()
                };
                let _ = tx.send(WhisperEngine::load(&path, cfg).map_err(|e| format!("{e:#}")));
            })
            .expect("spawn whisper load thread");
        self.recognizer = Recognizer::Loading(rx);
    }

    /// Project vocabulary plus example command phrases, so whisper is
    /// primed to hear the trigger word and the grammar after it.
    fn vocabulary_hint(&self) -> String {
        use std::fmt::Write as _;
        let p = &self.core.projects()[self.core.selected_project()];
        let mut trigger = self.core.trigger().to_owned();
        if let Some(first) = trigger.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        let mut hint = p.vocabulary.join(", ");
        if !hint.is_empty() {
            hint.push_str(". ");
        }
        let _ = write!(
            hint,
            "{trigger} delete sentence. {trigger} DP. {trigger} new paragraph. {trigger} send. {trigger} copy. {trigger} enhance, confirm."
        );
        hint
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
        self.core.set_typing_policy(self.typing_policy());
        self.pump_recognizer_load();
        self.pump_ai();
        self.pump_download();
        self.pump_live();
        self.pump_demo();
        self.dispatch(AppAction::Tick);
        if self.is_live() || self.demo.is_some() || self.download.is_some() {
            return Some(Duration::from_millis(20));
        }
        if matches!(self.recognizer, Recognizer::Loading(_)) || self.ai_rx.is_some() {
            return Some(Duration::from_millis(100));
        }
        self.core
            .toast()
            .map(|t| t.expires_at.saturating_sub(self.clock().mono) + Duration::from_millis(10))
    }

    fn pump_ai(&mut self) {
        let Some(rx) = &self.ai_rx else { return };
        if let Ok((id, result)) = rx.try_recv() {
            self.ai_rx = None;
            self.dispatch(AppAction::AiRewriteFinished { id, result });
        }
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
        self.apply_window_level(ui.ctx());
        let focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
        self.set_window_focused(focused);
        if let Some(delay) = self.pump() {
            ui.ctx().request_repaint_after(delay);
        }
        crate::ui::draw(self, ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_cycle_clockwise_from_top_right() {
        let mut c = Corner::TopRight;
        let seen: Vec<Corner> = (0..5)
            .map(|_| {
                let cur = c;
                c = c.next();
                cur
            })
            .collect();
        assert_eq!(
            seen,
            [
                Corner::TopRight,
                Corner::BottomRight,
                Corner::BottomLeft,
                Corner::TopLeft,
                Corner::TopRight
            ]
        );
    }

    #[test]
    fn corner_positions_inset_by_margin_and_never_negative() {
        let monitor = egui::vec2(1000.0, 800.0);
        let outer = egui::vec2(300.0, 350.0);
        assert_eq!(
            Corner::TopRight.position(monitor, outer, 8.0),
            egui::pos2(692.0, 8.0)
        );
        assert_eq!(
            Corner::BottomRight.position(monitor, outer, 8.0),
            egui::pos2(692.0, 442.0)
        );
        assert_eq!(
            Corner::BottomLeft.position(monitor, outer, 8.0),
            egui::pos2(8.0, 442.0)
        );
        assert_eq!(
            Corner::TopLeft.position(monitor, outer, 8.0),
            egui::pos2(8.0, 8.0)
        );
        let tiny = egui::vec2(100.0, 100.0);
        assert_eq!(
            Corner::BottomRight.position(tiny, outer, 8.0),
            egui::pos2(0.0, 0.0)
        );
    }
}
