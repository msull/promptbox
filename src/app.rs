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
use crate::core::{AppAction, AppCore, Clock, Effect, Project, SessionStatus};
use crate::ports::ai::Rewriter;
use crate::ports::clipboard::Clipboard;
use crate::ports::engine::{AudioChunk, EngineConfig, PushError, SpeechEngine};
use crate::ports::history::{HistoryStore, Settings, ThemeChoice};
use crate::ports::speech::SpeechEventKind;
use crate::ports::tools::ToolRunner;
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
    /// Whether the Dock icon currently shows the recording badge.
    dock_badge_shown: bool,
    /// Whether the voice-command help popup is open.
    pub show_commands: bool,
    /// Whether the settings window is open.
    pub show_settings: bool,
    /// The project editor, while open.
    pub project_editor: Option<ProjectEditor>,
    /// Text and timing of the on-screen caption overlay.
    pub caption: crate::caption::CaptionState,
    /// Text in the AI instruction box under the prompt.
    pub ai_instruction: String,
    /// Draft values in the settings window until saved.
    pub settings_draft: Settings,
    rewriter: Option<std::sync::Arc<dyn Rewriter>>,
    /// Result of the AI or tool worker in flight, as the action to dispatch.
    ai_rx: Option<Receiver<AppAction>>,
    tool_runner: std::sync::Arc<dyn ToolRunner>,
    /// Where tool folders live; `None` in tests without a data directory.
    tools_dir: Option<PathBuf>,
    /// Manifests that failed to load, for Settings.
    pub tool_problems: Vec<String>,
    typist: Box<dyn Typist>,
    /// Whether our own window is focused this frame (from egui).
    window_focused: bool,
}

impl PromptBoxApp {
    /// Production wiring: system clipboard and the platform data directory.
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::ui::install_symbol_font(&cc.egui_ctx);
        let mut app = Self::with_services(
            Box::new(SystemClipboard::default()),
            Box::new(FileStore::new(FileStore::default_dir())),
        );
        app.typist = Box::new(SystemTypist::default());
        app.tools_dir = Some(FileStore::default_dir().join("tools"));
        app.reload_tools();
        app
    }

    /// Re-reads every tool manifest from the tools folder.
    pub fn reload_tools(&mut self) {
        let Some(dir) = &self.tools_dir else {
            return;
        };
        let (tools, problems) = crate::adapters::tools::load_manifests(dir);
        for p in &problems {
            log::warn!("tool manifest: {p}");
        }
        log::info!("{} tool(s) loaded from {}", tools.len(), dir.display());
        self.tool_problems = problems;
        self.core.set_tools(tools);
    }

    /// The folder tool plugins are read from.
    #[must_use]
    pub fn tools_dir(&self) -> Option<&PathBuf> {
        self.tools_dir.as_ref()
    }

    /// Installs a tool runner directly (tests use a fake).
    pub fn set_tool_runner(&mut self, runner: std::sync::Arc<dyn ToolRunner>) {
        self.tool_runner = runner;
    }

    /// Registers tools without a folder (tests).
    pub fn set_tools(&mut self, tools: Vec<crate::ports::tools::ToolManifest>) {
        self.core.set_tools(tools);
    }

    /// Runs `job` on a named worker thread; its result is dispatched from
    /// the next frame's pump.
    fn spawn_worker(&mut self, name: &str, job: impl FnOnce() -> AppAction + Send + 'static) {
        let (tx, rx) = channel();
        self.ai_rx = Some(rx);
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let _ = tx.send(job());
            })
            .expect("spawn worker thread");
    }

    /// Wires explicit adapters (tests use fakes) and loads persisted state.
    #[must_use]
    pub fn with_services(
        clipboard: Box<dyn Clipboard>,
        mut history: Box<dyn HistoryStore>,
    ) -> Self {
        let recent = history.load_recent(RECENT_LIMIT);
        let draft = history.load_draft();
        let projects = history.load_projects();
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
            dock_badge_shown: false,
            show_commands: false,
            show_settings: false,
            project_editor: None,
            caption: crate::caption::CaptionState::default(),
            ai_instruction: String::new(),
            settings_draft: Settings::default(),
            rewriter: None,
            ai_rx: None,
            tool_runner: std::sync::Arc::new(crate::adapters::tools::ProcessToolRunner),
            tools_dir: None,
            tool_problems: Vec::new(),
            typist: Box::new(crate::adapters::typist::FakeTypist::default()),
            window_focused: true,
        };
        app.settings_draft = app.settings.clone();
        app.rebuild_rewriter();
        app.core.set_trigger(&app.settings.trigger);
        let wanted = app.settings.project.clone();
        app.dispatch(AppAction::ProjectsLoaded(projects));
        app.core.select_project_named(&wanted);
        app.remember_selected_project();
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
    pub fn captions_enabled(&self) -> bool {
        self.settings.captions
    }

    /// Shows or hides the on-screen caption overlay and remembers the choice.
    pub fn set_captions_enabled(&mut self, on: bool) {
        self.settings.captions = on;
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

    /// Keeps `settings.project` in step with the core's selection so the
    /// same project is active after a restart.
    fn remember_selected_project(&mut self) {
        let name = self.core.project().name.as_str();
        if self.settings.project != name {
            self.settings.project = name.to_owned();
            self.settings_draft.project = name.to_owned();
            if let Err(e) = self.history.save_settings(&self.settings) {
                log::warn!("could not save settings: {e}");
            }
        }
    }

    /// The single entry point for every user, speech, or timer action.
    pub fn dispatch(&mut self, action: AppAction) {
        let now = self.clock();
        let effects = self.core.dispatch(action, now);
        self.remember_selected_project();
        for effect in effects {
            if let Some(result) = self.run_effect(effect) {
                self.dispatch(result);
            }
        }
    }

    /// Performs one effect; returns the action that reports its result,
    /// or `None` when the result arrives later through a worker.
    fn run_effect(&mut self, effect: Effect) -> Option<AppAction> {
        let result = match effect {
            Effect::WriteClipboard(text) => {
                AppAction::ClipboardWriteFinished(self.clipboard.write_text(&text))
            }
            Effect::SaveHistory(prompt) => AppAction::HistorySaveFinished {
                id: prompt.id,
                result: self.history.save_sent(&prompt),
            },
            Effect::SaveDraft(text) => AppAction::DraftSaveFinished(self.history.save_draft(&text)),
            Effect::SaveProjects(list) => {
                AppAction::ProjectsSaveFinished(self.history.save_projects(&list))
            }
            Effect::StopListening => {
                self.stop_listening();
                return None;
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
                    return None;
                };
                self.spawn_worker("ai-rewrite", move || {
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
                    AppAction::AiRewriteFinished {
                        id: request.id,
                        result,
                    }
                });
                return None;
            }
            Effect::ChooseTool(request) => {
                let Some(rewriter) = self.rewriter.clone() else {
                    self.dispatch(AppAction::ToolChosen {
                        id: request.id,
                        result: Err("no OpenAI API key; set one in Settings".into()),
                    });
                    return None;
                };
                self.spawn_worker("ai-tool-choice", move || {
                    let result = rewriter.choose_tool(&request);
                    match &result {
                        Ok(c) => log::info!(
                            "tool choice: {:?} ({} prompt + {} completion tokens)",
                            c.call.as_ref().map(|c| &c.name),
                            c.prompt_tokens,
                            c.completion_tokens
                        ),
                        Err(e) => log::warn!("tool choice failed: {e}"),
                    }
                    AppAction::ToolChosen {
                        id: request.id,
                        result,
                    }
                });
                return None;
            }
            Effect::RunTool { id, tool, input } => {
                let runner = self.tool_runner.clone();
                self.spawn_worker("tool-run", move || {
                    let t = Instant::now();
                    let result = runner.run(&tool, &input);
                    match &result {
                        Ok(_) => log::info!(
                            "tool {} finished in {:.1} s",
                            tool.name,
                            t.elapsed().as_secs_f64()
                        ),
                        Err(e) => log::warn!("tool {} failed: {e}", tool.name),
                    }
                    AppAction::ToolFinished {
                        id,
                        name: tool.name,
                        result,
                    }
                });
                return None;
            }
        };
        Some(result)
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

    /// Opens the project editor on a copy of the current list.
    pub fn open_project_editor(&mut self) {
        self.project_editor = Some(ProjectEditor::from_projects(
            self.core.projects(),
            self.core.selected_project(),
        ));
    }

    /// Applies the editor's contents, persists them, and closes it. Returns
    /// false (and keeps the editor open) when a name is empty or repeated.
    pub fn save_project_editor(&mut self) -> bool {
        let Some(editor) = &self.project_editor else {
            return true;
        };
        let list = match editor.to_projects() {
            Ok(list) => list,
            Err(msg) => {
                if let Some(editor) = &mut self.project_editor {
                    editor.error = Some(msg);
                }
                return false;
            }
        };
        let chosen = editor.drafts[editor.selected].name.trim().to_owned();
        self.project_editor = None;
        self.dispatch(AppAction::ReplaceProjects(list));
        self.core.select_project_named(&chosen);
        self.remember_selected_project();
        true
    }

    /// Project vocabulary plus the command grammar's words, so whisper
    /// spells them reliably. The trigger word is deliberately absent:
    /// whisper echoes its prompt on silence and noise, and a prompt full of
    /// "Zevro ..." made it hallucinate trigger phrases. Trigger renderings
    /// are tolerated by the phonetic matching instead. (The hint is read
    /// once, when the engine loads.)
    fn vocabulary_hint(&self) -> String {
        let mut hint = self.core.project().recognition_terms().join(", ");
        if !hint.is_empty() {
            hint.push_str(". ");
        }
        hint.push_str(
            "delete sentence, delete paragraph, new paragraph, send, copy, enhance, confirm.",
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
        if let Ok(action) = rx.try_recv() {
            self.ai_rx = None;
            self.dispatch(action);
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

impl PromptBoxApp {
    /// Keeps the Dock badge in step with whether audio is being captured.
    /// Only touches `AppKit` when the state actually changes.
    fn sync_dock_badge(&mut self) {
        let recording = matches!(
            self.core.status(),
            SessionStatus::Listening | SessionStatus::Finishing
        );
        if recording != self.dock_badge_shown {
            crate::adapters::dock::set_recording_badge(recording);
            self.dock_badge_shown = recording;
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
        self.sync_dock_badge();
        crate::ui::draw(self, ui);
        crate::caption::draw(self, &ui.ctx().clone());
    }

    /// Fully transparent: the window's panels paint their own opaque
    /// backgrounds, and the caption overlay needs a transparent backbuffer
    /// (which eframe enables for every viewport from the root's settings).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
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

/// Editable text form of one project: lists are one entry per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDraft {
    pub name: String,
    pub vocabulary: String,
    pub corrections: String,
    pub glossary: String,
    pub context: String,
}

impl ProjectDraft {
    fn from_project(p: &Project) -> Self {
        use crate::core::project::lines;
        Self {
            name: p.name.clone(),
            vocabulary: lines::vocabulary_to_text(&p.vocabulary),
            corrections: lines::corrections_to_text(&p.corrections),
            glossary: lines::glossary_to_text(&p.glossary),
            context: p.context.clone(),
        }
    }

    fn to_project(&self) -> Project {
        use crate::core::project::lines;
        Project {
            name: self.name.trim().to_owned(),
            vocabulary: lines::vocabulary_from_text(&self.vocabulary),
            corrections: lines::corrections_from_text(&self.corrections),
            glossary: lines::glossary_from_text(&self.glossary),
            context: self.context.trim().to_owned(),
        }
    }
}

/// State of the Projects window: every project as editable text, plus
/// which one is being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEditor {
    pub drafts: Vec<ProjectDraft>,
    pub selected: usize,
    pub error: Option<String>,
}

impl ProjectEditor {
    fn from_projects(projects: &[Project], selected: usize) -> Self {
        Self {
            drafts: projects.iter().map(ProjectDraft::from_project).collect(),
            selected: selected.min(projects.len().saturating_sub(1)),
            error: None,
        }
    }

    /// Adds an unnamed project and selects it; Save insists on a name.
    pub fn add(&mut self) {
        self.drafts
            .push(ProjectDraft::from_project(&Project::new("")));
        self.selected = self.drafts.len() - 1;
        self.error = None;
    }

    /// Removes the selected project; the last one cannot be removed.
    pub fn remove_selected(&mut self) {
        if self.drafts.len() > 1 {
            self.drafts.remove(self.selected);
            self.selected = self.selected.min(self.drafts.len() - 1);
        }
        self.error = None;
    }

    fn to_projects(&self) -> Result<Vec<Project>, String> {
        let list: Vec<Project> = self.drafts.iter().map(ProjectDraft::to_project).collect();
        for (i, p) in list.iter().enumerate() {
            if p.name.is_empty() {
                return Err("Every project needs a name".to_owned());
            }
            if list[..i]
                .iter()
                .any(|q| q.name.eq_ignore_ascii_case(&p.name))
            {
                return Err(format!("Two projects are called \"{}\"", p.name));
            }
        }
        Ok(list)
    }
}
