//! The transcript document: authoritative committed text plus at most one
//! provisional span, with a single edit history shared by voice and manual
//! edits. All offsets are UTF-8 byte offsets.
//!
//! Invariants exercised by the tests in `tests.rs`:
//! 1. `committed` is authoritative user content; `rendered()` is a projection.
//! 2. At most one provisional span exists.
//! 3. A partial may only replace the span with the same session+utterance.
//! 4. A final commits as exactly one history entry.
//! 5. Every mutation is a `RangeReplace`; replaying history reproduces `committed`.
//! 6. Stale, duplicate, and out-of-order events do not change the document.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::ports::speech::{SessionId, SpeechEvent, SpeechEventKind, UtteranceId};

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalSpan {
    pub session: SessionId,
    pub utterance: UtteranceId,
    pub revision: u64,
    /// Byte offset into `committed` where the span is rendered.
    pub anchor: usize,
    /// Text as rendered (spacing already applied).
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditSource {
    Voice {
        session: SessionId,
        utterance: UtteranceId,
    },
    Manual,
    /// Whole-prompt replacement produced by an AI rewrite.
    Ai,
}

/// One entry of the single authoritative edit history, in committed coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeReplace {
    pub range: Range<usize>,
    pub old: String,
    pub new: String,
    pub source: EditSource,
    /// Provenance only: what the provisional span showed when this committed.
    pub provisional_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    StaleSession,
    DuplicateSeq,
    OutOfOrderSeq,
    StaleRevision,
    UtteranceFinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    AnchorCaptured,
    Provisional,
    Committed,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    OutOfBounds,
    NotGraphemeBoundary,
}

/// What to do with a live provisional span when a manual edit overlaps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapPolicy {
    CommitProvisional,
    CancelProvisional,
}

#[derive(Debug, Default)]
pub struct Document {
    committed: String,
    provisional: Option<ProvisionalSpan>,
    history: Vec<RangeReplace>,
    /// Entries undone and not yet superseded by a new edit.
    redo: Vec<RangeReplace>,
    last_seq: HashMap<SessionId, u64>,
    active_session: Option<SessionId>,
    finished: HashSet<(SessionId, UtteranceId)>,
    anchors: HashMap<(SessionId, UtteranceId), usize>,
    /// Cursor in rendered coordinates.
    cursor: usize,
}

impl Document {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn committed(&self) -> &str {
        &self.committed
    }

    #[must_use]
    pub fn provisional(&self) -> Option<&ProvisionalSpan> {
        self.provisional.as_ref()
    }

    #[must_use]
    pub fn history(&self) -> &[RangeReplace] {
        &self.history
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.committed.is_empty() && self.provisional.is_none()
    }

    /// Reverts the most recent history entry. A live provisional span is
    /// cancelled first (it was never committed, so it is not history).
    /// Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        if self.provisional.take().is_some() {
            return true;
        }
        let Some(e) = self.history.pop() else {
            return false;
        };
        let end = e.range.start + e.new.len();
        self.committed.replace_range(e.range.start..end, &e.old);
        self.cursor = e.range.start + e.old.len();
        self.redo.push(e);
        true
    }

    /// Re-applies the most recently undone entry. Returns false when there
    /// is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else {
            return false;
        };
        let end = e.range.start + e.old.len();
        self.committed.replace_range(e.range.start..end, &e.new);
        self.cursor = e.range.start + e.new.len();
        self.history.push(e);
        true
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Loads persisted text as the starting point, without a history entry.
    pub fn load(&mut self, text: &str) {
        self.provisional = None;
        self.anchors.clear();
        self.history.clear();
        self.redo.clear();
        text.clone_into(&mut self.committed);
        self.cursor = self.committed.len();
    }

    /// Replaces the whole document with `text` as one manual history entry.
    pub fn replace_all(&mut self, text: &str) {
        self.replace_all_from(text, EditSource::Manual);
    }

    /// Replaces the whole document as one undoable entry with `source`.
    pub fn replace_all_from(&mut self, text: &str, source: EditSource) {
        self.provisional = None;
        // Anchors captured in the old text are meaningless now; an utterance
        // in progress continues at the end of the new text instead.
        self.anchors.clear();
        let old = std::mem::take(&mut self.committed);
        self.redo.clear();
        self.history.push(RangeReplace {
            range: 0..old.len(),
            old,
            new: text.to_owned(),
            source,
            provisional_text: None,
        });
        text.clone_into(&mut self.committed);
        self.cursor = self.committed.len();
    }

    pub fn set_active_session(&mut self, session: SessionId) {
        self.active_session = Some(session);
    }

    /// Moves the cursor (rendered coordinates). Clamped to a char boundary.
    pub fn set_cursor(&mut self, pos: usize) {
        let r = self.rendered();
        let mut p = pos.min(r.len());
        while !r.is_char_boundary(p) {
            p -= 1;
        }
        self.cursor = p;
    }

    /// Committed text with the provisional span spliced in.
    #[must_use]
    pub fn rendered(&self) -> String {
        match &self.provisional {
            None => self.committed.clone(),
            Some(p) => {
                let mut s = String::with_capacity(self.committed.len() + p.text.len());
                s.push_str(&self.committed[..p.anchor]);
                s.push_str(&p.text);
                s.push_str(&self.committed[p.anchor..]);
                s
            }
        }
    }

    /// What a closed-caption overlay should show: the last committed
    /// sentence before the live span, then the provisional text. With no
    /// live span it is the sentence ending at the cursor, so the caption
    /// keeps showing a just-finalized utterance until the next one starts.
    #[must_use]
    pub fn caption(&self) -> String {
        let (end, live) = match &self.provisional {
            Some(p) => (p.anchor, p.text.trim()),
            None => (self.cursor, ""),
        };
        let last = crate::core::text::last_sentence_range(&self.committed, end)
            .map_or("", |r| self.committed[r].trim());
        match (last.is_empty(), live.is_empty()) {
            (true, _) => live.to_owned(),
            (false, true) => last.to_owned(),
            (false, false) => format!("{last} {live}"),
        }
    }

    /// Range of the provisional span in rendered coordinates.
    #[must_use]
    pub fn provisional_range(&self) -> Option<Range<usize>> {
        self.provisional
            .as_ref()
            .map(|p| p.anchor..p.anchor + p.text.len())
    }

    /// Replays the history from an empty string; must equal `committed`.
    #[must_use]
    pub fn replay_history(&self) -> String {
        let mut s = String::new();
        for e in &self.history {
            debug_assert_eq!(&s[e.range.clone()], e.old);
            s.replace_range(e.range.clone(), &e.new);
        }
        s
    }

    // ---- speech events -------------------------------------------------

    pub fn apply_event(&mut self, ev: &SpeechEvent) -> Result<Applied, Rejection> {
        if self.active_session != Some(ev.session) {
            return Err(Rejection::StaleSession);
        }
        let last = self.last_seq.get(&ev.session).copied().unwrap_or(0);
        if ev.sequence == last {
            return Err(Rejection::DuplicateSeq);
        }
        if ev.sequence < last {
            return Err(Rejection::OutOfOrderSeq);
        }
        self.last_seq.insert(ev.session, ev.sequence);

        match &ev.kind {
            SpeechEventKind::VoiceStarted { utterance } => {
                let key = (ev.session, *utterance);
                if let Some(p) = &self.provisional
                    && (p.session, p.utterance) != key
                {
                    self.commit_provisional();
                }
                let anchor = self.committed_pos(self.cursor);
                self.anchors.insert(key, anchor);
                Ok(Applied::AnchorCaptured)
            }
            SpeechEventKind::Partial {
                utterance,
                revision,
                text,
            } => {
                let key = (ev.session, *utterance);
                if self.finished.contains(&key) {
                    return Err(Rejection::UtteranceFinished);
                }
                match &mut self.provisional {
                    Some(p) if (p.session, p.utterance) == key => {
                        if *revision <= p.revision {
                            return Err(Rejection::StaleRevision);
                        }
                        p.revision = *revision;
                        p.text = Self::spaced(&self.committed, p.anchor, text);
                    }
                    other => {
                        if other.is_some() {
                            self.commit_provisional();
                        }
                        let anchor = self.anchor_for(key);
                        let spaced = Self::spaced(&self.committed, anchor, text);
                        self.provisional = Some(ProvisionalSpan {
                            session: ev.session,
                            utterance: *utterance,
                            revision: *revision,
                            anchor,
                            text: spaced,
                        });
                    }
                }
                Ok(Applied::Provisional)
            }
            SpeechEventKind::Final {
                utterance, text, ..
            } => {
                let key = (ev.session, *utterance);
                if self.finished.contains(&key) {
                    return Err(Rejection::UtteranceFinished);
                }
                let (anchor, prov_text) = match self.provisional.take() {
                    Some(p) if (p.session, p.utterance) == key => (p.anchor, Some(p.text)),
                    Some(p) => {
                        self.provisional = Some(p);
                        self.commit_provisional();
                        (self.anchor_for(key), None)
                    }
                    None => (self.anchor_for(key), None),
                };
                self.finished.insert(key);
                self.anchors.remove(&key);
                if text.trim().is_empty() {
                    self.cursor = anchor;
                    return Ok(Applied::Ignored);
                }
                let spaced = Self::spaced(&self.committed, anchor, text);
                self.insert_committed(
                    anchor,
                    &spaced,
                    EditSource::Voice {
                        session: ev.session,
                        utterance: *utterance,
                    },
                    prov_text,
                );
                self.cursor = anchor + spaced.len();
                Ok(Applied::Committed)
            }
            SpeechEventKind::VoiceEnded { .. }
            | SpeechEventKind::ProcessingDelayed
            | SpeechEventKind::AudioGap { .. }
            | SpeechEventKind::Error(_) => Ok(Applied::Ignored),
        }
    }

    // ---- manual edits --------------------------------------------------

    /// Replaces `range` (rendered coordinates, grapheme-aligned) with `new`.
    pub fn apply_manual_edit(
        &mut self,
        range: Range<usize>,
        new: &str,
        policy: OverlapPolicy,
    ) -> Result<(), EditError> {
        let rendered = self.rendered();
        if range.start > range.end || range.end > rendered.len() {
            return Err(EditError::OutOfBounds);
        }
        if !Self::is_grapheme_boundary(&rendered, range.start)
            || !Self::is_grapheme_boundary(&rendered, range.end)
        {
            return Err(EditError::NotGraphemeBoundary);
        }

        let committed_range = match self.provisional_range() {
            None => range.clone(),
            Some(pr) if range.end <= pr.start => {
                if let Some(p) = &mut self.provisional {
                    p.anchor = p.anchor - (range.end - range.start) + new.len();
                }
                range.clone()
            }
            Some(pr) if range.start >= pr.end => {
                let len = pr.end - pr.start;
                range.start - len..range.end - len
            }
            Some(pr) => match policy {
                OverlapPolicy::CommitProvisional => {
                    self.commit_provisional();
                    range.clone()
                }
                OverlapPolicy::CancelProvisional => {
                    let len = pr.end - pr.start;
                    self.provisional = None;
                    let start = range.start.min(pr.start);
                    let end = if range.end > pr.end {
                        range.end - len
                    } else {
                        pr.start
                    };
                    start..end
                }
            },
        };

        let old = self.committed[committed_range.clone()].to_owned();
        self.redo.clear();
        self.history.push(RangeReplace {
            range: committed_range.clone(),
            old,
            new: new.to_owned(),
            source: EditSource::Manual,
            provisional_text: None,
        });
        self.committed.replace_range(committed_range.clone(), new);
        // Cursor lands after the inserted text, in rendered coordinates.
        let mut cursor = committed_range.start + new.len();
        if let Some(pr) = self.provisional_range()
            && cursor >= pr.start
        {
            cursor += pr.end - pr.start;
        }
        self.cursor = cursor;
        Ok(())
    }

    // ---- helpers -------------------------------------------------------

    fn anchor_for(&self, key: (SessionId, UtteranceId)) -> usize {
        let anchor = self
            .anchors
            .get(&key)
            .copied()
            .unwrap_or_else(|| self.committed_pos(self.cursor));
        // Defensive: never let a stale offset index past the committed text.
        let mut anchor = anchor.min(self.committed.len());
        while !self.committed.is_char_boundary(anchor) {
            anchor -= 1;
        }
        anchor
    }

    /// Maps a rendered position into committed coordinates.
    fn committed_pos(&self, rendered_pos: usize) -> usize {
        match self.provisional_range() {
            Some(pr) if rendered_pos >= pr.end => rendered_pos - (pr.end - pr.start),
            Some(pr) if rendered_pos > pr.start => pr.start,
            _ => rendered_pos,
        }
    }

    /// Adds a leading space when inserting directly after a non-space char.
    fn spaced(committed: &str, anchor: usize, text: &str) -> String {
        let needs_space = anchor > 0
            && !committed[..anchor].ends_with(char::is_whitespace)
            && !text.starts_with(char::is_whitespace)
            && !text.is_empty();
        if needs_space {
            format!(" {text}")
        } else {
            text.to_owned()
        }
    }

    fn insert_committed(
        &mut self,
        anchor: usize,
        text: &str,
        source: EditSource,
        provisional_text: Option<String>,
    ) {
        self.redo.clear();
        self.history.push(RangeReplace {
            range: anchor..anchor,
            old: String::new(),
            new: text.to_owned(),
            source,
            provisional_text,
        });
        self.committed.insert_str(anchor, text);
    }

    /// Commits whatever provisional span is live, as if a Final with the
    /// same text had arrived.
    pub fn commit_provisional(&mut self) {
        if let Some(p) = self.provisional.take() {
            let key = (p.session, p.utterance);
            self.finished.insert(key);
            self.anchors.remove(&key);
            self.insert_committed(
                p.anchor,
                &p.text,
                EditSource::Voice {
                    session: p.session,
                    utterance: p.utterance,
                },
                Some(p.text.clone()),
            );
            self.cursor = p.anchor + p.text.len();
        }
    }

    fn is_grapheme_boundary(s: &str, pos: usize) -> bool {
        pos == s.len() || s.grapheme_indices(true).any(|(i, _)| i == pos)
    }
}
