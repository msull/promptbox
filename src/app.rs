//! Orchestration: owns [`AppCore`], the port adapters, and the fake speech
//! source. Maps effects to adapter calls and feeds their results back as
//! actions. Rendering lives in [`crate::ui`].

use std::time::{Duration, Instant, SystemTime};

use crate::adapters::clipboard::SystemClipboard;
use crate::adapters::fake_speech::{DEMO_SCRIPT, FakeDictation};
use crate::adapters::persistence::FileStore;
use crate::core::action::RECENT_LIMIT;
use crate::core::{AppAction, AppCore, Clock, Effect};
use crate::ports::clipboard::Clipboard;
use crate::ports::history::HistoryStore;

pub struct PromptBoxApp {
    core: AppCore,
    clipboard: Box<dyn Clipboard>,
    history: Box<dyn HistoryStore>,
    started: Instant,
    /// Added to the monotonic clock; tests use it to skip ahead.
    time_offset: Duration,
    demo: Option<FakeDictation>,
    next_session: u64,
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
            next_session: 1,
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

    /// Starts scripted dictation as a new session. `with_gap` injects an
    /// audio gap so the degraded state can be seen.
    pub fn start_demo(&mut self, with_gap: bool) {
        self.stop_demo();
        let session = self.next_session;
        self.next_session += 1;
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

    /// Called once per frame: drains due speech events and ticks the clock.
    /// Returns how soon a repaint is wanted, if anything is animating.
    pub fn pump(&mut self) -> Option<Duration> {
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
        self.dispatch(AppAction::Tick);
        if self.demo.is_some() {
            return Some(Duration::from_millis(50));
        }
        self.core
            .toast()
            .map(|t| t.expires_at.saturating_sub(self.clock().mono) + Duration::from_millis(10))
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
