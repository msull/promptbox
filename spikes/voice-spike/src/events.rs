//! Engine-facing speech event types. Mirrors the design doc's conceptual
//! `SpeechEvent`; audio ranges are sample offsets within a session.

use std::ops::Range;

pub type SessionId = u64;
pub type UtteranceId = u64;

#[derive(Debug, Clone, PartialEq)]
pub struct SpeechEvent {
    pub session: SessionId,
    /// Per-session, strictly increasing across every emitted event.
    pub sequence: u64,
    /// Sample offsets within the session this event refers to.
    pub audio_range: Range<u64>,
    pub kind: SpeechEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpeechEventKind {
    VoiceStarted {
        utterance: UtteranceId,
    },
    VoiceEnded {
        utterance: UtteranceId,
    },
    Partial {
        utterance: UtteranceId,
        /// Per-utterance, increases only when the text changes.
        revision: u64,
        text: String,
    },
    Final {
        utterance: UtteranceId,
        text: String,
        confidence: Option<f32>,
    },
    ProcessingDelayed,
    AudioGap {
        missing: Range<u64>,
    },
    Error(SpeechError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpeechError {
    EventOverflow { dropped: u64 },
    Engine(String),
}

impl SpeechEvent {
    #[must_use]
    pub fn utterance(&self) -> Option<UtteranceId> {
        match &self.kind {
            SpeechEventKind::VoiceStarted { utterance }
            | SpeechEventKind::VoiceEnded { utterance }
            | SpeechEventKind::Partial { utterance, .. }
            | SpeechEventKind::Final { utterance, .. } => Some(*utterance),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(&self) -> String {
        match &self.kind {
            SpeechEventKind::VoiceStarted { utterance } => format!("VoiceStarted u{utterance}"),
            SpeechEventKind::VoiceEnded { utterance } => format!("VoiceEnded   u{utterance}"),
            SpeechEventKind::Partial {
                utterance,
                revision,
                text,
            } => format!("Partial      u{utterance} r{revision:<3} {text:?}"),
            SpeechEventKind::Final {
                utterance, text, ..
            } => format!("Final        u{utterance}      {text:?}"),
            SpeechEventKind::ProcessingDelayed => "ProcessingDelayed".to_owned(),
            SpeechEventKind::AudioGap { missing } => {
                format!("AudioGap     {}..{} samples", missing.start, missing.end)
            }
            SpeechEventKind::Error(e) => format!("Error        {e:?}"),
        }
    }
}
