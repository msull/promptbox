//! Actions, effects, and the [`AppCore`] state machine.
//!
//! Buttons, shortcuts, speech events, and completed side effects all arrive
//! as [`AppAction`]s. Side effects the core wants (clipboard, persistence)
//! are returned as [`Effect`]s; the adapter runs them and reports back with
//! another action. Time is passed in, never read, so tests never sleep.

use std::ops::Range;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::commands::{self, Command, DEFAULT_TRIGGER};
use crate::core::document::{Document, EditSource, OverlapPolicy};
use crate::core::project::{Project, placeholder_projects};
use crate::core::text::{last_paragraph_range, last_sentence_range, paragraph_break_for};
use crate::ports::ai::{CLEAN_UP_INSTRUCTION, RewriteRequest, RewriteResponse};
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
    /// Result of pasting into the focused app after a Send.
    TypeFinished {
        id: u64,
        result: Result<(), String>,
    },
    DraftSaveFinished(Result<(), String>),
    DraftLoaded(Result<Option<String>, String>),
    RecentLoaded(Result<Vec<SentPrompt>, String>),
    ClearPrompt,
    Undo,
    Redo,
    /// Removes the sentence ending at or containing the cursor.
    DeleteSentence,
    /// Removes the paragraph ending at or containing the cursor.
    DeleteParagraph,
    /// Inserts a line break at the cursor.
    Newline,
    /// Starts a new paragraph at the cursor (one blank line).
    NewParagraph,
    SelectProject(usize),
    /// Ask the AI to transform the whole prompt with this instruction.
    AiRewrite {
        instruction: String,
    },
    /// One-click clean-up of the dictated text.
    AiCleanUp,
    AiRewriteFinished {
        id: u64,
        result: Result<RewriteResponse, String>,
    },
    Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    WriteClipboard(String),
    SaveHistory(SentPrompt),
    SaveDraft(String),
    /// A voice command asked to stop the microphone.
    StopListening,
    /// Run this rewrite on a worker and report back with `AiRewriteFinished`.
    AiRewrite(RewriteRequest),
    /// Paste the clipboard into the focused app; `submit` presses Return.
    TypeIntoActiveApp {
        id: u64,
        submit: bool,
    },
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
    typing: TypingStage,
}

/// Progress of the optional paste-into-app stage of a Send.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TypingStage {
    NotWanted,
    /// Enabled for this send; requested once both stores succeed.
    Wanted,
    Requested,
    Done(Result<(), String>),
}

/// Whether Send should also paste into the focused app, decided per send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypingPolicy {
    pub enabled: bool,
    pub submit: bool,
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
    trigger: String,
    ai_pending: Option<u64>,
    ai_prompt_tokens: u64,
    ai_completion_tokens: u64,
    typing: TypingPolicy,
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
            trigger: DEFAULT_TRIGGER.to_owned(),
            ai_pending: None,
            ai_prompt_tokens: 0,
            ai_completion_tokens: 0,
            typing: TypingPolicy {
                enabled: false,
                submit: true,
            },
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

    /// Set by the app each frame: typing is only sensible when another app
    /// is focused and the user has enabled it.
    pub fn set_typing_policy(&mut self, policy: TypingPolicy) {
        self.typing = policy;
    }

    /// True while an AI rewrite is in flight.
    #[must_use]
    pub fn ai_busy(&self) -> bool {
        self.ai_pending.is_some()
    }

    /// Tokens spent on AI rewrites this session: (prompt, completion).
    #[must_use]
    pub fn ai_tokens(&self) -> (u64, u64) {
        (self.ai_prompt_tokens, self.ai_completion_tokens)
    }

    /// The word that opens the voice-command channel.
    #[must_use]
    pub fn trigger(&self) -> &str {
        &self.trigger
    }

    /// Changes the trigger word (from settings). Empty input is ignored.
    pub fn set_trigger(&mut self, trigger: &str) {
        let t = trigger.trim();
        if !t.is_empty() {
            t.clone_into(&mut self.trigger);
        }
    }

    /// Rendered-coordinate range of a command being spoken inside the live
    /// provisional span, for highlighting.
    #[must_use]
    pub fn pending_command_range(&self) -> Option<Range<usize>> {
        let p = self.doc.provisional()?;
        let offset = commands::pending_command_offset(&p.text, &self.trigger)?;
        Some(p.anchor + offset..p.anchor + p.text.len())
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
            AppAction::SpeechEventReceived(ev) => self.on_speech(&ev, now, &mut effects),
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
                self.settle_pending(now.mono, &mut effects);
            }
            AppAction::HistorySaveFinished { id, result } => {
                if let Some(p) = &mut self.pending
                    && p.snapshot.id == id
                {
                    p.history = Some(result);
                }
                self.settle_pending(now.mono, &mut effects);
            }
            AppAction::TypeFinished { id, result } => {
                if let Some(p) = &mut self.pending
                    && p.snapshot.id == id
                    && p.typing == TypingStage::Requested
                {
                    p.typing = TypingStage::Done(result);
                }
                self.settle_pending(now.mono, &mut effects);
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
            AppAction::Redo => {
                if self.doc.redo() {
                    self.mark_dirty(now.mono);
                } else {
                    self.show_toast("Nothing to redo".to_owned(), false, now.mono);
                }
            }
            AppAction::DeleteSentence => {
                self.delete_unit(last_sentence_range, "No sentence to delete", now.mono);
            }
            AppAction::DeleteParagraph => {
                self.delete_unit(last_paragraph_range, "No paragraph to delete", now.mono);
            }
            AppAction::Newline => self.insert_at_cursor("\n", now.mono),
            AppAction::NewParagraph => {
                self.doc.commit_provisional();
                let text = paragraph_break_for(self.doc.committed(), self.doc.cursor());
                self.insert_at_cursor(text, now.mono);
            }
            AppAction::SelectProject(i) => {
                if i < self.projects.len() {
                    self.selected_project = i;
                }
            }
            AppAction::DraftSaveFinished(Ok(())) | AppAction::DraftLoaded(Ok(None)) => {}
            AppAction::AiCleanUp => {
                self.begin_rewrite(CLEAN_UP_INSTRUCTION.to_owned(), now, &mut effects);
            }
            AppAction::AiRewrite { instruction } => {
                self.begin_rewrite(instruction, now, &mut effects);
            }
            AppAction::AiRewriteFinished { id, result } => {
                if self.ai_pending == Some(id) {
                    self.ai_pending = None;
                    match result {
                        Ok(r) => {
                            self.ai_prompt_tokens += r.prompt_tokens;
                            self.ai_completion_tokens += r.completion_tokens;
                            self.doc.replace_all_from(&r.text, EditSource::Ai);
                            self.mark_dirty(now.mono);
                            self.show_toast(
                                "Rewritten. Undo restores the original.".to_owned(),
                                false,
                                now.mono,
                            );
                        }
                        Err(e) => {
                            self.show_toast(format!("AI rewrite failed: {e}"), true, now.mono);
                        }
                    }
                } else {
                    log::debug!("ignoring stale AI result {id}");
                }
            }
            AppAction::Tick => self.check_stall(now.mono),
        }
        self.maybe_autosave(now.mono, &mut effects);
        effects
    }

    // ---- helpers -----------------------------------------------------

    fn on_speech(&mut self, ev: &SpeechEvent, clock: Clock, effects: &mut Vec<Effect>) {
        let now = clock.mono;
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
        // Commands live only in finals; a partial keeps its command words
        // visible (highlighted by the UI) until the Final removes them.
        let mut commands = Vec::new();
        let ev = match &ev.kind {
            SpeechEventKind::Final {
                utterance,
                text,
                confidence,
            } => {
                let extracted = commands::extract(text, &self.trigger);
                commands = extracted.commands;
                SpeechEvent {
                    kind: SpeechEventKind::Final {
                        utterance: *utterance,
                        text: extracted.dictation,
                        confidence: *confidence,
                    },
                    ..ev.clone()
                }
            }
            _ => ev.clone(),
        };
        let ev = &ev;
        match self.doc.apply_event(ev) {
            Ok(_) => {
                // The document accepted this Final exactly once (duplicates
                // and stale sessions are rejected above), so commands run
                // at most once per utterance.
                for cmd in &commands {
                    self.run_command(cmd, clock, effects);
                }
                if matches!(
                    ev.kind,
                    SpeechEventKind::Partial { .. } | SpeechEventKind::Final { .. }
                ) {
                    self.last_progress = Some(now);
                    if self.stall_flagged {
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

    fn run_command(&mut self, cmd: &Command, clock: Clock, effects: &mut Vec<Effect>) {
        let action = match cmd {
            Command::DeleteSentence => AppAction::DeleteSentence,
            Command::DeleteParagraph => AppAction::DeleteParagraph,
            Command::Undo => AppAction::Undo,
            Command::Redo => AppAction::Redo,
            Command::Newline => AppAction::Newline,
            Command::NewParagraph => AppAction::NewParagraph,
            Command::Clear => AppAction::ClearPrompt,
            Command::Copy => AppAction::CopyPrompt,
            Command::Send => AppAction::SendPrompt,
            Command::StopListening => {
                effects.push(Effect::StopListening);
                self.show_toast("Voice: stop listening".to_owned(), false, clock.mono);
                return;
            }
            Command::Aborted => {
                self.show_toast("Voice: command aborted".to_owned(), false, clock.mono);
                return;
            }
            Command::Unknown(heard) => {
                let msg = if heard.is_empty() {
                    format!("Heard \"{}\" with no command", self.trigger)
                } else {
                    format!("Unknown voice command \"{heard}\"")
                };
                self.show_toast(msg, true, clock.mono);
                return;
            }
        };
        effects.extend(self.dispatch(action, clock));
        // Commands that already toast (undo with nothing to undo, send)
        // keep their message; otherwise confirm what was heard.
        if self
            .toast
            .as_ref()
            .is_none_or(|t| t.expires_at < clock.mono + TOAST_DURATION)
        {
            self.show_toast(format!("Voice: {}", cmd.label()), false, clock.mono);
        }
    }

    fn begin_rewrite(&mut self, instruction: String, now: Clock, effects: &mut Vec<Effect>) {
        if self.ai_pending.is_some() {
            self.show_toast("AI is still working…".to_owned(), false, now.mono);
            return;
        }
        if instruction.trim().is_empty() {
            self.show_toast(
                "Type what the AI should do first".to_owned(),
                false,
                now.mono,
            );
            return;
        }
        self.doc.commit_provisional();
        let content = self.doc.committed().to_owned();
        if content.trim().is_empty() {
            self.show_toast("Nothing to rewrite".to_owned(), false, now.mono);
            return;
        }
        let id = self.fresh_send_id(now.wall);
        self.ai_pending = Some(id);
        effects.push(Effect::AiRewrite(RewriteRequest {
            id,
            instruction,
            content,
        }));
    }

    /// Commits any live span, then removes the unit `range_of` picks out
    /// relative to the cursor, as one undoable edit.
    fn delete_unit(
        &mut self,
        range_of: fn(&str, usize) -> Option<Range<usize>>,
        empty_msg: &str,
        now: Duration,
    ) {
        self.doc.commit_provisional();
        let Some(range) = range_of(self.doc.committed(), self.doc.cursor()) else {
            self.show_toast(empty_msg.to_owned(), false, now);
            return;
        };
        if self
            .doc
            .apply_manual_edit(range, "", OverlapPolicy::CommitProvisional)
            .is_ok()
        {
            self.mark_dirty(now);
        }
    }

    fn insert_at_cursor(&mut self, text: &str, now: Duration) {
        if text.is_empty() {
            return;
        }
        self.doc.commit_provisional();
        let at = self.doc.cursor();
        if self
            .doc
            .apply_manual_edit(at..at, text, OverlapPolicy::CommitProvisional)
            .is_ok()
        {
            self.mark_dirty(now);
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
        let typing = if kind == PendingKind::Send && self.typing.enabled {
            TypingStage::Wanted
        } else {
            TypingStage::NotWanted
        };
        self.pending = Some(Pending {
            kind,
            snapshot,
            clipboard: None,
            history: None,
            typing,
        });
    }

    fn settle_pending(&mut self, now: Duration, effects: &mut Vec<Effect>) {
        let Some(p) = &mut self.pending else { return };
        let stored = match p.kind {
            PendingKind::Copy => p.clipboard.is_some(),
            PendingKind::Send => p.clipboard.is_some() && p.history.is_some(),
        };
        if !stored {
            return;
        }
        // Typing is the third stage: only once clipboard and history both
        // succeeded, and requested exactly once.
        if p.clipboard == Some(Ok(())) && p.history == Some(Ok(())) {
            match p.typing {
                TypingStage::Wanted => {
                    p.typing = TypingStage::Requested;
                    effects.push(Effect::TypeIntoActiveApp {
                        id: p.snapshot.id,
                        submit: self.typing.submit,
                    });
                    return;
                }
                TypingStage::Requested => return,
                TypingStage::NotWanted | TypingStage::Done(_) => {}
            }
        }
        let p = self.pending.take().expect("checked above");
        let clipboard = p.clipboard.unwrap_or(Ok(()));
        let history = p.history.unwrap_or(Ok(()));
        if let TypingStage::Done(Err(e)) = &p.typing
            && clipboard.is_ok()
            && history.is_ok()
        {
            self.show_toast(
                format!("Copied, but could not type into the app: {e}. Prompt kept."),
                true,
                now,
            );
            return;
        }
        let typed = p.typing == TypingStage::Done(Ok(()));
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
                let msg = if typed {
                    "Prompt sent"
                } else {
                    "Prompt copied"
                };
                self.show_toast(msg.to_owned(), false, now);
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

    fn settle_stores(core: &mut AppCore, id: u64, at: u64) -> Vec<Effect> {
        core.dispatch(AppAction::ClipboardWriteFinished(Ok(())), Clock::at(at));
        core.dispatch(
            AppAction::HistorySaveFinished { id, result: Ok(()) },
            Clock::at(at + 1),
        )
    }

    #[test]
    fn send_with_typing_pastes_only_after_both_stores_succeed_then_clears() {
        let mut core = AppCore::new();
        core.set_typing_policy(TypingPolicy {
            enabled: true,
            submit: true,
        });
        typed(&mut core, "hello", 0);
        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(1));
        let Effect::SaveHistory(s) = &effects[1] else {
            panic!()
        };
        let id = s.id;
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::TypeIntoActiveApp { .. }))
        );
        core.dispatch(AppAction::ClipboardWriteFinished(Ok(())), Clock::at(2));
        assert_eq!(core.doc().committed(), "hello");
        let effects = core.dispatch(
            AppAction::HistorySaveFinished { id, result: Ok(()) },
            Clock::at(3),
        );
        assert_eq!(
            effects,
            vec![Effect::TypeIntoActiveApp { id, submit: true }]
        );
        assert!(core.is_busy());
        assert_eq!(core.doc().committed(), "hello", "not cleared until typed");
        let effects = core.dispatch(
            AppAction::HistorySaveFinished { id, result: Ok(()) },
            Clock::at(4),
        );
        assert!(effects.is_empty(), "typing requested once");
        core.dispatch(AppAction::TypeFinished { id, result: Ok(()) }, Clock::at(5));
        assert!(!core.is_busy());
        assert_eq!(core.doc().committed(), "");
        assert_eq!(toast_text(&core), "Prompt sent");
    }

    #[test]
    fn typing_failure_keeps_the_prompt_and_store_failure_skips_typing() {
        let mut core = AppCore::new();
        core.set_typing_policy(TypingPolicy {
            enabled: true,
            submit: false,
        });
        typed(&mut core, "keep", 0);
        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(1));
        let Effect::SaveHistory(s) = &effects[1] else {
            panic!()
        };
        let id = s.id;
        let effects = settle_stores(&mut core, id, 2);
        assert_eq!(
            effects,
            vec![Effect::TypeIntoActiveApp { id, submit: false }]
        );
        core.dispatch(
            AppAction::TypeFinished {
                id,
                result: Err("no permission".into()),
            },
            Clock::at(4),
        );
        assert!(!core.is_busy());
        assert_eq!(core.doc().committed(), "keep");
        assert!(toast_text(&core).contains("no permission"));
        assert!(core.recent().is_empty());

        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(10_000));
        let Effect::SaveHistory(s) = &effects[1] else {
            panic!()
        };
        let id = s.id;
        core.dispatch(
            AppAction::ClipboardWriteFinished(Err("x".into())),
            Clock::at(10_001),
        );
        let effects = core.dispatch(
            AppAction::HistorySaveFinished { id, result: Ok(()) },
            Clock::at(10_002),
        );
        assert!(effects.is_empty(), "no typing after a store failure");
        assert!(!core.is_busy());
        assert_eq!(core.doc().committed(), "keep");
    }

    #[test]
    fn typing_disabled_keeps_the_two_stage_send() {
        let mut core = AppCore::new();
        typed(&mut core, "plain", 0);
        let effects = core.dispatch(AppAction::SendPrompt, Clock::at(1));
        let Effect::SaveHistory(s) = &effects[1] else {
            panic!()
        };
        let effects = settle_stores(&mut core, s.id, 2);
        assert!(effects.is_empty());
        assert_eq!(core.doc().committed(), "");
        assert_eq!(toast_text(&core), "Prompt copied");
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
    fn delete_sentence_removes_the_most_recent_sentence_and_is_undoable() {
        let mut core = AppCore::new();
        typed(&mut core, "First one. Second one.", 0);
        core.dispatch(AppAction::DeleteSentence, Clock::at(1));
        assert_eq!(core.doc().committed(), "First one.");
        assert_eq!(core.doc().cursor(), 10);
        core.dispatch(AppAction::Undo, Clock::at(2));
        assert_eq!(core.doc().committed(), "First one. Second one.");
        core.dispatch(AppAction::Redo, Clock::at(3));
        assert_eq!(core.doc().committed(), "First one.");
        core.dispatch(AppAction::DeleteSentence, Clock::at(4));
        assert_eq!(core.doc().committed(), "");
        core.dispatch(AppAction::DeleteSentence, Clock::at(5));
        assert_eq!(toast_text(&core), "No sentence to delete");
    }

    #[test]
    fn delete_sentence_commits_a_live_partial_first() {
        let mut core = AppCore::new();
        typed(&mut core, "Keep this.", 0);
        core.dispatch(AppAction::SessionStarted(1), Clock::at(1));
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(2),
        );
        core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Partial {
                    utterance: 1,
                    revision: 1,
                    text: "drop this".into(),
                },
            ),
            Clock::at(3),
        );
        core.dispatch(AppAction::DeleteSentence, Clock::at(4));
        assert_eq!(core.doc().committed(), "Keep this.");
        assert!(core.doc().provisional().is_none());
    }

    #[test]
    fn delete_paragraph_and_paragraph_breaks() {
        let mut core = AppCore::new();
        typed(&mut core, "Para one.", 0);
        core.dispatch(AppAction::NewParagraph, Clock::at(1));
        assert_eq!(core.doc().committed(), "Para one.\n\n");
        core.dispatch(AppAction::NewParagraph, Clock::at(2));
        assert_eq!(
            core.doc().committed(),
            "Para one.\n\n",
            "no extra blank lines"
        );
        typed(&mut core, "Para two.", 3);
        core.dispatch(AppAction::Newline, Clock::at(4));
        typed(&mut core, "same para", 5);
        assert_eq!(core.doc().committed(), "Para one.\n\nPara two.\nsame para");
        core.dispatch(AppAction::DeleteParagraph, Clock::at(6));
        assert_eq!(core.doc().committed(), "Para one.");
        core.dispatch(AppAction::Undo, Clock::at(7));
        assert_eq!(core.doc().committed(), "Para one.\n\nPara two.\nsame para");
    }

    #[test]
    fn dictation_after_a_paragraph_break_does_not_add_a_leading_space() {
        let mut core = AppCore::new();
        typed(&mut core, "Para one.", 0);
        core.dispatch(AppAction::NewParagraph, Clock::at(1));
        core.dispatch(AppAction::SessionStarted(1), Clock::at(2));
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(3),
        );
        core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "Para two.".into(),
                    confidence: None,
                },
            ),
            Clock::at(4),
        );
        assert_eq!(core.doc().committed(), "Para one.\n\nPara two.");
    }

    #[test]
    fn voice_command_runs_once_and_its_words_never_enter_the_prompt() {
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
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "First sentence.".into(),
                    confidence: None,
                },
            ),
            Clock::at(2),
        );
        core.dispatch(
            speech(1, 3, SpeechEventKind::VoiceStarted { utterance: 2 }),
            Clock::at(3),
        );
        core.dispatch(
            speech(
                1,
                4,
                SpeechEventKind::Partial {
                    utterance: 2,
                    revision: 1,
                    text: "Second one. Zevro del".into(),
                },
            ),
            Clock::at(4),
        );
        assert_eq!(
            core.doc().rendered(),
            "First sentence. Second one. Zevro del"
        );
        let r = core.pending_command_range().unwrap();
        assert_eq!(
            &core.doc().rendered()[r],
            "Zevro del",
            "command words are highlighted"
        );
        let final_ev = speech(
            1,
            5,
            SpeechEventKind::Final {
                utterance: 2,
                text: "Second one. Zevro delete sentence.".into(),
                confidence: None,
            },
        );
        core.dispatch(final_ev.clone(), Clock::at(5));
        assert_eq!(core.doc().committed(), "First sentence.");
        assert_eq!(toast_text(&core), "Voice: delete sentence");
        // A duplicate Final (same sequence) is rejected by the document, so
        // the command does not run a second time.
        core.dispatch(final_ev, Clock::at(6));
        assert_eq!(core.doc().committed(), "First sentence.");
        // Undo restores the deleted sentence, not the command words.
        core.dispatch(AppAction::Undo, Clock::at(7));
        assert_eq!(core.doc().committed(), "First sentence. Second one.");
    }

    #[test]
    fn voice_send_copies_and_clears_and_stop_is_an_effect() {
        let mut core = AppCore::new();
        typed(&mut core, "Ship it.", 0);
        core.dispatch(AppAction::SessionStarted(1), Clock::at(1));
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(2),
        );
        let effects = core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "Zevro send".into(),
                    confidence: None,
                },
            ),
            Clock::at(3),
        );
        assert_eq!(effects[0], Effect::WriteClipboard("Ship it.".into()));
        assert!(matches!(effects[1], Effect::SaveHistory(_)));
        core.dispatch(
            speech(1, 3, SpeechEventKind::VoiceStarted { utterance: 2 }),
            Clock::at(4),
        );
        let effects = core.dispatch(
            speech(
                1,
                4,
                SpeechEventKind::Final {
                    utterance: 2,
                    text: "zevro stop listening".into(),
                    confidence: None,
                },
            ),
            Clock::at(5),
        );
        assert!(effects.contains(&Effect::StopListening));
    }

    #[test]
    fn aborted_voice_command_changes_nothing() {
        let mut core = AppCore::new();
        typed(&mut core, "Keep me.", 0);
        core.dispatch(AppAction::SessionStarted(1), Clock::at(1));
        core.dispatch(
            speech(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }),
            Clock::at(2),
        );
        let effects = core.dispatch(
            speech(
                1,
                2,
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "Zevro clear abort".into(),
                    confidence: None,
                },
            ),
            Clock::at(3),
        );
        assert!(effects.is_empty());
        assert_eq!(core.doc().committed(), "Keep me.");
        assert_eq!(toast_text(&core), "Voice: command aborted");
    }

    #[test]
    fn unknown_voice_command_is_reported_and_rest_is_kept() {
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
                SpeechEventKind::Final {
                    utterance: 1,
                    text: "Zevro frobnicate the thing.".into(),
                    confidence: None,
                },
            ),
            Clock::at(2),
        );
        assert_eq!(
            core.doc().committed(),
            "",
            "unknown command words are not dictated"
        );
        assert!(core.toast().unwrap().is_error);
        assert!(toast_text(&core).contains("frobnicate the thing"));
    }

    fn finished(id: u64, text: &str) -> AppAction {
        AppAction::AiRewriteFinished {
            id,
            result: Ok(RewriteResponse {
                text: text.into(),
                prompt_tokens: 100,
                completion_tokens: 40,
            }),
        }
    }

    #[test]
    fn ai_rewrite_replaces_the_prompt_as_one_undoable_edit() {
        let mut core = AppCore::new();
        typed(&mut core, "um so like add a a pydantic model", 0);
        let effects = core.dispatch(AppAction::AiCleanUp, Clock::at(1));
        let Effect::AiRewrite(req) = &effects[0] else {
            panic!("{effects:?}")
        };
        assert_eq!(req.content, "um so like add a a pydantic model");
        assert_eq!(req.instruction, CLEAN_UP_INSTRUCTION);
        assert!(core.ai_busy());
        // A second request while busy is refused.
        assert!(core.dispatch(AppAction::AiCleanUp, Clock::at(2)).is_empty());
        core.dispatch(finished(req.id, "Add a Pydantic model."), Clock::at(3));
        assert!(!core.ai_busy());
        assert_eq!(core.doc().committed(), "Add a Pydantic model.");
        assert_eq!(core.ai_tokens(), (100, 40));
        core.dispatch(AppAction::Undo, Clock::at(4));
        assert_eq!(core.doc().committed(), "um so like add a a pydantic model");
        core.dispatch(AppAction::Redo, Clock::at(5));
        assert_eq!(core.doc().committed(), "Add a Pydantic model.");
    }

    #[test]
    fn stale_or_failed_ai_results_leave_the_prompt_alone() {
        let mut core = AppCore::new();
        typed(&mut core, "text", 0);
        core.dispatch(finished(999, "stale"), Clock::at(1));
        assert_eq!(core.doc().committed(), "text");
        let effects = core.dispatch(
            AppAction::AiRewrite {
                instruction: "make it formal".into(),
            },
            Clock::at(2),
        );
        let Effect::AiRewrite(req) = &effects[0] else {
            panic!()
        };
        core.dispatch(
            AppAction::AiRewriteFinished {
                id: req.id,
                result: Err("quota".into()),
            },
            Clock::at(3),
        );
        assert!(!core.ai_busy());
        assert_eq!(core.doc().committed(), "text");
        assert!(core.toast().unwrap().is_error);
        assert!(
            core.dispatch(
                AppAction::AiRewrite {
                    instruction: "  ".into()
                },
                Clock::at(4)
            )
            .is_empty()
        );
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
