//! Actions, effects, and the [`AppCore`] state machine.
//!
//! Buttons, shortcuts, speech events, and completed side effects all arrive
//! as [`AppAction`]s. Side effects the core wants (clipboard, persistence)
//! are returned as [`Effect`]s; the adapter runs them and reports back with
//! another action. Time is passed in, never read, so tests never sleep.

use std::ops::Range;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::document::{Document, OverlapPolicy};
use crate::core::project::{Project, placeholder_projects};
use crate::ports::history::SentPrompt;
use crate::ports::speech::{SessionId, SpeechEvent, SpeechEventKind};

const TOAST_DURATION: Duration = Duration::from_millis(2500);
const DRAFT_DEBOUNCE: Duration = Duration::from_millis(500);
/// Input level above which we consider the user to be speaking.
pub const VOICE_DB: f32 = -40.0;
/// Continuous voice without transcript progress before we warn.
const STALL_AFTER: Duration = Duration::from_secs(4);
pub const STALL_MESSAGE: &str = "Transcription delayed";
pub const RECENT_LIMIT: usize = 50;

/// Monotonic time since app start plus wall time, supplied by the caller.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub mono: Duration,
    pub wall: SystemTime,
}

impl Clock {
    #[must_use]
    pub fn at(mono_ms: u64) -> Self {
        Self {
            mono: Duration::from_millis(mono_ms),
            wall: UNIX_EPOCH + Duration::from_secs(1_700_000_000 + mono_ms / 1000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Listening,
    /// Stop requested; waiting for the recognizer to finish the last words.
    Finishing,
    /// Something was lost or delayed; sticky until acknowledged.
    Degraded(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
    pub is_error: bool,
    pub expires_at: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    /// Manual edit in rendered coordinates.
    ReplaceText {
        range: Range<usize>,
        text: String,
    },
    CursorMoved(usize),
    SpeechEventReceived(SpeechEvent),
    SessionStarted(SessionId),
    /// Stop requested; late events for the session are still accepted.
    SessionStopping,
    SessionStopped,
    /// Latest microphone level in dBFS, independent of recognition.
    AudioLevel(f32),
    /// The recognizer could not start (missing model, no microphone, ...).
    EngineUnavailable(String),
    AcknowledgeStatus,
    CopyPrompt,
    SendPrompt,
    ClipboardWriteFinished(Result<(), String>),
    HistorySaveFinished {
        id: u64,
        result: Result<(), String>,
    },
    DraftSaveFinished(Result<(), String>),
    DraftLoaded(Result<Option<String>, String>),
    RecentLoaded(Result<Vec<SentPrompt>, String>),
    ClearPrompt,
    Undo,
    SelectProject(usize),
    Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    WriteClipboard(String),
    SaveHistory(SentPrompt),
    SaveDraft(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Copy,
    Send,
}

#[derive(Debug)]
struct Pending {
    kind: PendingKind,
    snapshot: SentPrompt,
    clipboard: Option<Result<(), String>>,
    history: Option<Result<(), String>>,
}

#[derive(Debug)]
pub struct AppCore {
    doc: Document,
    status: SessionStatus,
    active_session: Option<SessionId>,
    toast: Option<Toast>,
    pending: Option<Pending>,
    projects: Vec<Project>,
    selected_project: usize,
    recent: Vec<SentPrompt>,
    draft_dirty_since: Option<Duration>,
    last_saved_draft: Option<String>,
    next_send_id: u64,
    audio_level_db: f32,
    voice_since: Option<Duration>,
    last_progress: Option<Duration>,
    stall_flagged: bool,
}

impl Default for AppCore {
    fn default() -> Self {
        Self::new()
    }
}

impl AppCore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            doc: Document::new(),
            status: SessionStatus::Idle,
            active_session: None,
            toast: None,
            pending: None,
            projects: placeholder_projects(),
            selected_project: 0,
            recent: Vec::new(),
            draft_dirty_since: None,
            last_saved_draft: None,
            next_send_id: 0,
            audio_level_db: -120.0,
            voice_since: None,
            last_progress: None,
            stall_flagged: false,
        }
    }

    // ---- read side ---------------------------------------------------

    #[must_use]
    pub fn doc(&self) -> &Document {
        &self.doc
    }

    #[must_use]
    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    #[must_use]
    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    #[must_use]
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    #[must_use]
    pub fn selected_project(&self) -> usize {
        self.selected_project
    }

    #[must_use]
    pub fn recent(&self) -> &[SentPrompt] {
        &self.recent
    }

    /// True while a Copy or Send is waiting for its adapters.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    #[must_use]
    pub fn is_listening(&self) -> bool {
        self.active_session.is_some()
    }

    /// Latest microphone level in dBFS (-120 when nothing has arrived).
    #[must_use]
    pub fn audio_level_db(&self) -> f32 {
        self.audio_level_db
    }

    // ---- dispatch ----------------------------------------------------

    #[allow(clippy::too_many_lines)]
    pub fn dispatch(&mut self, action: AppAction, now: Clock) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.expire_toast(now.mono);
        match action {
            AppAction::ReplaceText { range, text } => {
                let result =
                    self.doc
                        .apply_manual_edit(range, &text, OverlapPolicy::CommitProvisional);
                if let Err(e) = result {
                    log::warn!("manual edit rejected ({e:?}); replacing whole document");
                    self.doc.replace_all(&text);
                }
                self.mark_dirty(now.mono);
            }
            AppAction::CursorMoved(pos) => self.doc.set_cursor(pos),
            AppAction::SpeechEventReceived(ev) => self.on_speech(&ev, now.mono),
            AppAction::SessionStarted(id) => {
                self.active_session = Some(id);
                self.doc.set_active_session(id);
                self.voice_since = None;
                self.last_progress = Some(now.mono);
                self.stall_flagged = false;
                if !matches!(
                    self.status,
                    SessionStatus::Degraded(_) | SessionStatus::Error(_)
                ) {
                    self.status = SessionStatus::Listening;
                }
            }
            AppAction::SessionStopping => {
                if self.status == SessionStatus::Listening {
                    self.status = SessionStatus::Finishing;
                }
                self.voice_since = None;
            }
            AppAction::SessionStopped => {
                self.active_session = None;
                self.doc.commit_provisional();
                self.mark_dirty(now.mono);
                self.voice_since = None;
                if matches!(
                    self.status,
                    SessionStatus::Listening | SessionStatus::Finishing
                ) {
                    self.status = SessionStatus::Idle;
                }
            }
            AppAction::AudioLevel(db) => {
                self.audio_level_db = db;
                if db > VOICE_DB {
                    self.voice_since.get_or_insert(now.mono);
                } else {
                    self.voice_since = None;
                }
                self.check_stall(now.mono);
            }
            AppAction::EngineUnavailable(why) => {
                self.active_session = None;
                self.status = SessionStatus::Error(why);
            }
            AppAction::AcknowledgeStatus => {
                self.status = if self.active_session.is_some() {
                    SessionStatus::Listening
                } else {
                    SessionStatus::Idle
                };
            }
            AppAction::CopyPrompt => self.begin_copy_or_send(PendingKind::Copy, now, &mut effects),
            AppAction::SendPrompt => self.begin_copy_or_send(PendingKind::Send, now, &mut effects),
            AppAction::ClipboardWriteFinished(result) => {
                if let Some(p) = &mut self.pending {
                    p.clipboard = Some(result);
                }
                self.settle_pending(now.mono);
            }
            AppAction::HistorySaveFinished { id, result } => {
                if let Some(p) = &mut self.pending
                    && p.snapshot.id == id
                {
                    p.history = Some(result);
                }
                self.settle_pending(now.mono);
            }
            AppAction::DraftSaveFinished(Err(e)) => {
                self.show_toast(format!("Draft autosave failed: {e}"), true, now.mono);
            }
            AppAction::DraftLoaded(Ok(Some(text))) => {
                if self.doc.is_empty() && !text.is_empty() {
                    self.doc.load(&text);
                    self.last_saved_draft = Some(text);
                    self.show_toast("Restored unsaved draft".to_owned(), false, now.mono);
                }
            }
            AppAction::DraftLoaded(Err(e)) => {
                self.show_toast(format!("Could not read draft: {e}"), true, now.mono);
            }
            AppAction::RecentLoaded(Ok(recent)) => self.recent = recent,
            AppAction::RecentLoaded(Err(e)) => {
                self.show_toast(format!("Could not read history: {e}"), true, now.mono);
            }
            AppAction::ClearPrompt => {
                if !self.doc.is_empty() {
                    self.doc.replace_all("");
                    self.mark_dirty(now.mono);
                    self.show_toast("Cleared. Undo restores it.".to_owned(), false, now.mono);
                }
            }
            AppAction::Undo => {
                if self.doc.undo() {
                    self.mark_dirty(now.mono);
                } else {
                    self.show_toast("Nothing to undo".to_owned(), false, now.mono);
                }
            }
            AppAction::SelectProject(i) => {
                if i < self.projects.len() {
                    self.selected_project = i;
                }
            }
            AppAction::DraftSaveFinished(Ok(())) | AppAction::DraftLoaded(Ok(None)) => {}
            AppAction::Tick => self.check_stall(now.mono),
        }
        self.maybe_autosave(now.mono, &mut effects);
        effects
    }

    // ---- helpers -----------------------------------------------------

    fn on_speech(&mut self, ev: &SpeechEvent, now: Duration) {
        match &ev.kind {
            SpeechEventKind::AudioGap { missing } => {
                let ms = (missing.end - missing.start) / 16;
                self.status = SessionStatus::Degraded(format!("Audio gap: {ms} ms lost"));
            }
            SpeechEventKind::ProcessingDelayed => {
                if !matches!(self.status, SessionStatus::Error(_)) {
                    self.status = SessionStatus::Degraded("Transcription delayed".to_owned());
                }
            }
            SpeechEventKind::Error(e) => {
                self.status = SessionStatus::Error(format!("{e:?}"));
            }
            _ => {}
        }
        match self.doc.apply_event(ev) {
            Ok(_) => {
                if matches!(
                    ev.kind,
                    SpeechEventKind::Partial { .. } | SpeechEventKind::Final { .. }
                ) {
                    self.last_progress = Some(now);
                    if self.stall_flagged {
                        // Recovered: the recognizer is advancing again.
                        self.stall_flagged = false;
                        if self.status == SessionStatus::Degraded(STALL_MESSAGE.to_owned()) {
                            self.status = SessionStatus::Listening;
                        }
                    }
                }
                if matches!(ev.kind, SpeechEventKind::Final { .. }) {
                    self.mark_dirty(now);
                }
            }
            Err(r) => log::debug!("speech event {}/{} ignored: {r:?}", ev.session, ev.sequence),
        }
    }

    /// Voice has been arriving continuously but no partial or final has
    /// landed for `STALL_AFTER`: warn, without blocking anything.
    fn check_stall(&mut self, now: Duration) {
        if self.status != SessionStatus::Listening || self.stall_flagged {
            return;
        }
        let (Some(voice), Some(progress)) = (self.voice_since, self.last_progress) else {
            return;
        };
        if now.saturating_sub(voice.max(progress)) >= STALL_AFTER {
            self.stall_flagged = true;
            self.status = SessionStatus::Degraded(STALL_MESSAGE.to_owned());
        }
    }

    fn begin_copy_or_send(&mut self, kind: PendingKind, now: Clock, effects: &mut Vec<Effect>) {
        if self.pending.is_some() {
            self.show_toast("Still copying…".to_owned(), false, now.mono);
            return;
        }
        self.doc.commit_provisional();
        let text = self.doc.committed().to_owned();
        if text.trim().is_empty() {
            self.show_toast("Nothing to copy".to_owned(), false, now.mono);
            return;
        }
        let id = self.fresh_send_id(now.wall);
        let snapshot = SentPrompt {
            id,
            text: text.clone(),
            sent_at: now.wall,
            project: self.projects[self.selected_project].name.clone(),
        };
        effects.push(Effect::WriteClipboard(text));
        if kind == PendingKind::Send {
            effects.push(Effect::SaveHistory(snapshot.clone()));
        }
        self.pending = Some(Pending {
            kind,
            snapshot,
            clipboard: None,
            history: None,
        });
    }

    fn settle_pending(&mut self, now: Duration) {
        let Some(p) = &self.pending else { return };
        let done = match p.kind {
            PendingKind::Copy => p.clipboard.is_some(),
            PendingKind::Send => p.clipboard.is_some() && p.history.is_some(),
        };
        if !done {
            return;
        }
        let p = self.pending.take().expect("checked above");
        let clipboard = p.clipboard.unwrap_or(Ok(()));
        let history = p.history.unwrap_or(Ok(()));
        match (p.kind, clipboard, history) {
            (PendingKind::Copy, Ok(()), _) => self.show_toast("Copied".to_owned(), false, now),
            (PendingKind::Copy, Err(e), _) => {
                self.show_toast(format!("Copy failed: {e}"), true, now);
            }
            (PendingKind::Send, Ok(()), Ok(())) => {
                // Only now is it safe to clear: clipboard and history both hold it.
                self.doc.replace_all("");
                self.mark_dirty(now);
                self.recent.insert(0, p.snapshot);
                self.recent.truncate(RECENT_LIMIT);
                self.show_toast("Prompt copied".to_owned(), false, now);
            }
            (PendingKind::Send, Err(e), _) => {
                self.show_toast(format!("Send failed: {e}. Prompt kept."), true, now);
            }
            (PendingKind::Send, Ok(()), Err(e)) => {
                self.show_toast(
                    format!("Copied, but history save failed: {e}. Prompt kept."),
                    true,
                    now,
                );
            }
        }
    }

    fn fresh_send_id(&mut self, wall: SystemTime) -> u64 {
        let nanos = wall
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        self.next_send_id = self.next_send_id.max(nanos).max(self.next_send_id + 1);
        self.next_send_id
    }

    fn mark_dirty(&mut self, now: Duration) {
        if self.draft_dirty_since.is_none() {
            self.draft_dirty_since = Some(now);
        }
    }

    fn maybe_autosave(&mut self, now: Duration, effects: &mut Vec<Effect>) {
        if let Some(since) = self.draft_dirty_since
            && now.saturating_sub(since) >= DRAFT_DEBOUNCE
        {
            self.draft_dirty_since = None;
            let text = self.doc.committed().to_owned();
            if self.last_saved_draft.as_deref() != Some(text.as_str()) {
                self.last_saved_draft = Some(text.clone());
                effects.push(Effect::SaveDraft(text));
            }
        }
    }

    fn show_toast(&mut self, text: String, is_error: bool, now: Duration) {
        self.toast = Some(Toast {
            text,
            is_error,
            expires_at: now + TOAST_DURATION,
        });
    }

    fn expire_toast(&mut self, now: Duration) {
        if self.toast.as_ref().is_some_and(|t| now >= t.expires_at) {
            self.toast = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(core: &mut AppCore, text: &str, at: u64) -> Vec<Effect> {
        let len = core.doc().rendered().len();
        core.dispatch(
            AppAction::ReplaceText {
                range: len..len,
                text: text.to_owned(),
            },
            Clock::at(at),
        )
    }

    fn toast_text(core: &AppCore) -> String {
        core.toast().map(|t| t.text.clone()).unwrap_or_default()
    }

    #[test]
    fn send_requests_clipboard_and_history_then_clears_on_both_ok() {
        let mut core = AppCore::new();
        typed(&mut core, "hello", 0);
        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(10));
        let Effect::SaveHistory(snapshot) = &effects[1] else {
            panic!("expected SaveHistory, got {effects:?}");
        };
        assert_eq!(effects[0], Effect::WriteClipboard("hello".into()));
        assert!(core.is_busy());
        assert_eq!(
            core.doc().committed(),
            "hello",
            "not cleared before results"
        );

        core.dispatch(AppAction::ClipboardWriteFinished(Ok(())), Clock::at(20));
        assert_eq!(
            core.doc().committed(),
            "hello",
            "not cleared with one result"
        );
        core.dispatch(
            AppAction::HistorySaveFinished {
                id: snapshot.id,
                result: Ok(()),
            },
            Clock::at(30),
        );
        assert!(!core.is_busy());
        assert_eq!(core.doc().committed(), "");
        assert_eq!(toast_text(&core), "Prompt copied");
        assert_eq!(core.recent()[0].text, "hello");
        // The clear is one undoable step.
        core.dispatch(AppAction::Undo, Clock::at(40));
        assert_eq!(core.doc().committed(), "hello");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn send_never_clears_when_either_side_fails_in_any_order() {
        let orders: [(&str, &str); 4] = [
            ("cb", "hist"),
            ("hist", "cb"),
            ("cb", "hist"),
            ("hist", "cb"),
        ];
        let failures = [(true, false), (false, true), (true, true), (false, false)];
        for ((first, _), (cb_fails, hist_fails)) in orders.iter().zip(failures) {
            let mut core = AppCore::new();
            typed(&mut core, "keep", 0);
            let effects = core.dispatch(AppAction::SendPrompt, Clock::at(1));
            let Effect::SaveHistory(snapshot) = &effects[1] else {
                panic!()
            };
            let cb = AppAction::ClipboardWriteFinished(if cb_fails {
                Err("nope".into())
            } else {
                Ok(())
            });
            let hist = AppAction::HistorySaveFinished {
                id: snapshot.id,
                result: if hist_fails {
                    Err("disk".into())
                } else {
                    Ok(())
                },
            };
            let (a, b) = if *first == "cb" {
                (cb, hist)
            } else {
                (hist, cb)
            };
            core.dispatch(a, Clock::at(2));
            core.dispatch(b, Clock::at(3));
            assert!(!core.is_busy());
            if cb_fails || hist_fails {
                assert_eq!(
                    core.doc().committed(),
                    "keep",
                    "{first} cb={cb_fails} hist={hist_fails}"
                );
                assert!(core.toast().unwrap().is_error);
                assert!(core.recent().is_empty());
            } else {
                assert_eq!(core.doc().committed(), "");
            }
            // Retry after failure is safe.
            let effects = core.dispatch(AppAction::SendPrompt, Clock::at(4));
            assert_eq!(effects.is_empty(), core.doc().committed().is_empty());
        }
    }

    #[test]
    fn stale_history_result_does_not_settle_a_new_send() {
        let mut core = AppCore::new();
        typed(&mut core, "a", 0);
        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(1));
        let Effect::SaveHistory(s) = &effects[1] else {
            panic!()
        };
        let old_id = s.id;
        core.dispatch(
            AppAction::ClipboardWriteFinished(Err("x".into())),
            Clock::at(2),
        );
        core.dispatch(
            AppAction::HistorySaveFinished {
                id: old_id,
                result: Ok(()),
            },
            Clock::at(3),
        );
        assert!(!core.is_busy());
        // New send with a new id; a late result for the old id must not count.
        core.dispatch(AppAction::SendPrompt, Clock::at(4_000));
        core.dispatch(AppAction::ClipboardWriteFinished(Ok(())), Clock::at(4_001));
        core.dispatch(
            AppAction::HistorySaveFinished {
                id: old_id,
                result: Ok(()),
            },
            Clock::at(4_002),
        );
        assert!(core.is_busy());
        assert_eq!(core.doc().committed(), "a");
    }

    #[test]
    fn copy_only_needs_clipboard_and_keeps_text() {
        let mut core = AppCore::new();
        typed(&mut core, "hi", 0);
        let effects = core.dispatch(AppAction::CopyPrompt, Clock::at(1));
        assert_eq!(effects, vec![Effect::WriteClipboard("hi".into())]);
        core.dispatch(AppAction::ClipboardWriteFinished(Ok(())), Clock::at(2));
        assert_eq!(core.doc().committed(), "hi");
        assert_eq!(toast_text(&core), "Copied");
        assert!(core.recent().is_empty());
    }

    #[test]
    fn empty_prompt_is_not_sent() {
        let mut core = AppCore::new();
        typed(&mut core, "   ", 0);
        assert!(
            core.dispatch(AppAction::SendPrompt, Clock::at(1))
                .is_empty()
        );
        assert_eq!(toast_text(&core), "Nothing to copy");
    }

    #[test]
    fn toast_expires_by_clock() {
        let mut core = AppCore::new();
        core.dispatch(AppAction::Undo, Clock::at(0));
        assert!(core.toast().is_some());
        core.dispatch(AppAction::Tick, Clock::at(2_499));
        assert!(core.toast().is_some());
        core.dispatch(AppAction::Tick, Clock::at(2_500));
        assert!(core.toast().is_none());
    }

    #[test]
    fn draft_autosave_is_debounced_and_deduplicated() {
        let mut core = AppCore::new();
        assert!(typed(&mut core, "a", 0).is_empty());
        assert!(typed(&mut core, "b", 100).is_empty());
        assert!(core.dispatch(AppAction::Tick, Clock::at(499)).is_empty());
        assert_eq!(
            core.dispatch(AppAction::Tick, Clock::at(500)),
            vec![Effect::SaveDraft("ab".into())]
        );
        assert!(core.dispatch(AppAction::Tick, Clock::at(5_000)).is_empty());
        // Undo then redo the same text: content unchanged, no second save.
        core.dispatch(AppAction::Undo, Clock::at(6_000));
        typed(&mut core, "b", 6_001);
        assert!(core.dispatch(AppAction::Tick, Clock::at(7_000)).is_empty());
    }

    #[test]
    fn draft_loads_only_into_an_empty_document() {
        let mut core = AppCore::new();
        core.dispatch(
            AppAction::DraftLoaded(Ok(Some("saved".into()))),
            Clock::at(0),
        );
        assert_eq!(core.doc().committed(), "saved");
        assert!(core.doc().history().is_empty());
        core.dispatch(
            AppAction::DraftLoaded(Ok(Some("other".into()))),
            Clock::at(1),
        );
        assert_eq!(core.doc().committed(), "saved");
    }

    fn speech(session: u64, seq: u64, kind: SpeechEventKind) -> AppAction {
        AppAction::SpeechEventReceived(SpeechEvent {
            session,
            sequence: seq,
            audio_range: 0..0,
            kind,
        })
    }

    #[test]
    fn status_follows_session_and_gaps_are_sticky_until_acknowledged() {
        let mut core = AppCore::new();
        assert_eq!(*core.status(), SessionStatus::Idle);
        core.dispatch(AppAction::SessionStarted(1), Clock::at(0));
        assert_eq!(*core.status(), SessionStatus::Listening);
        core.dispatch(
            speech(1, 1, SpeechEventKind::AudioGap { missing: 0..1600 }),
            Clock::at(1),
        );
        assert_eq!(
            *core.status(),
            SessionStatus::Degraded("Audio gap: 100 ms lost".into())
        );
        core.dispatch(AppAction::SessionStopped, Clock::at(2));
        assert!(
            matches!(core.status(), SessionStatus::Degraded(_)),
            "sticky"
        );
        core.dispatch(AppAction::AcknowledgeStatus, Clock::at(3));
        assert_eq!(*core.status(), SessionStatus::Idle);
    }

    #[test]
    fn stall_is_flagged_after_continuous_voice_without_progress_and_recovers() {
        let mut core = AppCore::new();
        core.dispatch(AppAction::SessionStarted(1), Clock::at(0));
        for t in (0..4_000).step_by(500) {
            core.dispatch(AppAction::AudioLevel(-20.0), Clock::at(t));
            assert_eq!(*core.status(), SessionStatus::Listening, "at {t}");
        }
        core.dispatch(AppAction::AudioLevel(-20.0), Clock::at(4_000));
        assert_eq!(
            *core.status(),
            SessionStatus::Degraded(STALL_MESSAGE.into())
        );
        // Progress clears the warning automatically.
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(4_100),
        );
        core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Partial {
                    utterance: 1,
                    revision: 1,
                    text: "hi".into(),
                },
            ),
            Clock::at(4_200),
        );
        assert_eq!(*core.status(), SessionStatus::Listening);
    }

    #[test]
    fn silence_resets_the_stall_timer() {
        let mut core = AppCore::new();
        core.dispatch(AppAction::SessionStarted(1), Clock::at(0));
        core.dispatch(AppAction::AudioLevel(-20.0), Clock::at(0));
        core.dispatch(AppAction::AudioLevel(-90.0), Clock::at(3_000));
        core.dispatch(AppAction::AudioLevel(-20.0), Clock::at(3_100));
        core.dispatch(AppAction::Tick, Clock::at(6_000));
        assert_eq!(*core.status(), SessionStatus::Listening);
        core.dispatch(AppAction::Tick, Clock::at(7_100));
        assert!(matches!(core.status(), SessionStatus::Degraded(_)));
    }

    #[test]
    fn stopping_keeps_accepting_late_events_until_stopped() {
        let mut core = AppCore::new();
        core.dispatch(AppAction::SessionStarted(1), Clock::at(0));
        core.dispatch(AppAction::SessionStopping, Clock::at(1));
        assert_eq!(*core.status(), SessionStatus::Finishing);
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(2),
        );
        core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "late words".into(),
                    confidence: None,
                },
            ),
            Clock::at(3),
        );
        assert_eq!(core.doc().committed(), "late words");
        core.dispatch(AppAction::SessionStopped, Clock::at(4));
        assert_eq!(*core.status(), SessionStatus::Idle);
    }

    #[test]
    fn engine_unavailable_is_an_error_status() {
        let mut core = AppCore::new();
        core.dispatch(
            AppAction::EngineUnavailable("no model".into()),
            Clock::at(0),
        );
        assert_eq!(*core.status(), SessionStatus::Error("no model".into()));
        assert!(!core.is_listening());
    }

    #[test]
    fn speech_partials_render_provisionally_and_finals_commit() {
        let mut core = AppCore::new();
        typed(&mut core, "Intro.", 0);
        core.dispatch(AppAction::SessionStarted(7), Clock::at(1));
        core.dispatch(
            speech(7, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(2),
        );
        core.dispatch(
            speech(
                7,
                2,
                SpeechEventKind::Partial {
                    utterance: 1,
                    revision: 1,
                    text: "run the".into(),
                },
            ),
            Clock::at(3),
        );
        assert_eq!(core.doc().rendered(), "Intro. run the");
        assert_eq!(core.doc().committed(), "Intro.");
        core.dispatch(
            speech(
                7,
                3,
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "Run the tests.".into(),
                    confidence: None,
                },
            ),
            Clock::at(4),
        );
        assert_eq!(core.doc().committed(), "Intro. Run the tests.");
        // A late event from a stale session is ignored.
        core.dispatch(
            speech(
                6,
                9,
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "old".into(),
                    confidence: None,
                },
            ),
            Clock::at(5),
        );
        assert_eq!(core.doc().committed(), "Intro. Run the tests.");
    }

    #[test]
    fn stopping_a_session_commits_the_live_span() {
        let mut core = AppCore::new();
        core.dispatch(AppAction::SessionStarted(1), Clock::at(0));
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(1),
        );
        core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Partial {
                    utterance: 1,
                    revision: 1,
                    text: "half".into(),
                },
            ),
            Clock::at(2),
        );
        core.dispatch(AppAction::SessionStopped, Clock::at(3));
        assert_eq!(core.doc().committed(), "half");
        assert!(core.doc().provisional().is_none());
    }

    #[test]
    fn send_commits_provisional_text_first() {
        let mut core = AppCore::new();
        core.dispatch(AppAction::SessionStarted(1), Clock::at(0));
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(1),
        );
        core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Partial {
                    utterance: 1,
                    revision: 1,
                    text: "words".into(),
                },
            ),
            Clock::at(2),
        );
        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(3));
        assert_eq!(effects[0], Effect::WriteClipboard("words".into()));
    }
}
