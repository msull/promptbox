//! Scripted speech events on a clock, so the listening/provisional/degraded
//! states can be exercised end to end before a microphone exists. Emits the
//! same event stream shape as the whisper adapter: partials that
//! occasionally revise an earlier word, then a Final.

use std::ops::Range;
use std::time::Duration;

use crate::ports::speech::{SessionId, SpeechEvent, SpeechEventKind};

const SAMPLE_RATE_MS: u64 = 16; // samples per millisecond

pub const DEMO_SCRIPT: &[&str] = &[
    "Add a Pydantic model for the DynamoDB item and use a conditional expression so the write isn't overwritten.",
    "Then move that validation down into the service layer.",
    "Zevro new paragraph.",
    "Add unit tests for both cases before you refactor anything.",
];

struct Scheduled {
    at: Duration,
    kind: SpeechEventKind,
    audio_range: Range<u64>,
}

pub struct FakeDictation {
    session: SessionId,
    schedule: Vec<Scheduled>,
    next: usize,
    sequence: u64,
    started_at: Duration,
}

impl FakeDictation {
    /// Builds the schedule for `script`. With `inject_gap`, an `AudioGap`
    /// event is emitted during the second utterance.
    #[must_use]
    pub fn new(
        session: SessionId,
        script: &[&str],
        started_at: Duration,
        inject_gap: bool,
    ) -> Self {
        let mut schedule = Vec::new();
        let mut t = Duration::from_millis(400);
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX) * SAMPLE_RATE_MS;
        for (u, sentence) in script.iter().enumerate() {
            let utterance = u as u64 + 1;
            let start = ms(t);
            schedule.push(Scheduled {
                at: t,
                kind: SpeechEventKind::VoiceStarted { utterance },
                audio_range: start..start,
            });
            let words: Vec<&str> = sentence.split_whitespace().collect();
            let mut revision = 0;
            let mut shown = 0;
            let mut n = 0;
            while shown < words.len() {
                t += Duration::from_millis(450);
                shown = (shown + 2).min(words.len());
                revision += 1;
                n += 1;
                let mut text = words[..shown].join(" ").to_lowercase();
                // Every third partial garbles the newest word, as whisper does.
                if n % 3 == 0 && shown < words.len() {
                    let last = words[shown - 1].to_lowercase();
                    text.truncate(text.len() - last.len());
                    text.push_str(&last.chars().rev().collect::<String>());
                }
                schedule.push(Scheduled {
                    at: t,
                    kind: SpeechEventKind::Partial {
                        utterance,
                        revision,
                        text,
                    },
                    audio_range: start..ms(t),
                });
                if inject_gap && u == 1 && n == 2 {
                    let at = ms(t);
                    schedule.push(Scheduled {
                        at: t + Duration::from_millis(10),
                        kind: SpeechEventKind::AudioGap {
                            missing: at..at + 800 * SAMPLE_RATE_MS,
                        },
                        audio_range: at..at + 800 * SAMPLE_RATE_MS,
                    });
                }
            }
            t += Duration::from_millis(600);
            schedule.push(Scheduled {
                at: t,
                kind: SpeechEventKind::VoiceEnded { utterance },
                audio_range: ms(t)..ms(t),
            });
            t += Duration::from_millis(150);
            schedule.push(Scheduled {
                at: t,
                kind: SpeechEventKind::Final {
                    utterance,
                    text: (*sentence).to_owned(),
                    confidence: Some(0.9),
                },
                audio_range: start..ms(t),
            });
            t += Duration::from_millis(900);
        }
        Self {
            session,
            schedule,
            next: 0,
            sequence: 0,
            started_at,
        }
    }

    #[must_use]
    pub fn session(&self) -> SessionId {
        self.session
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.next >= self.schedule.len()
    }

    /// Returns every event due by `now`, in order.
    pub fn poll(&mut self, now: Duration) -> Vec<SpeechEvent> {
        let elapsed = now.saturating_sub(self.started_at);
        let mut out = Vec::new();
        while let Some(s) = self.schedule.get(self.next)
            && s.at <= elapsed
        {
            self.sequence += 1;
            out.push(SpeechEvent {
                session: self.session,
                sequence: self.sequence,
                audio_range: s.audio_range.clone(),
                kind: s.kind.clone(),
            });
            self.next += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_started_partials_final_in_order_with_increasing_sequence() {
        let mut fake = FakeDictation::new(3, &["Run the tests now."], Duration::ZERO, false);
        assert!(fake.poll(Duration::from_millis(100)).is_empty());
        let events = fake.poll(Duration::from_secs(60));
        assert!(fake.is_finished());
        let seqs: Vec<u64> = events.iter().map(|e| e.sequence).collect();
        assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
        assert!(events.iter().all(|e| e.session == 3));
        assert!(matches!(
            events[0].kind,
            SpeechEventKind::VoiceStarted { utterance: 1 }
        ));
        assert!(matches!(
            events[1].kind,
            SpeechEventKind::Partial { revision: 1, .. }
        ));
        let SpeechEventKind::Final { text, .. } = &events.last().unwrap().kind else {
            panic!("last event should be Final");
        };
        assert_eq!(text, "Run the tests now.");
        assert!(fake.poll(Duration::from_secs(61)).is_empty());
    }

    #[test]
    fn gap_is_injected_in_second_utterance_only_when_requested() {
        let has_gap = |inject| {
            let mut f = FakeDictation::new(1, DEMO_SCRIPT, Duration::ZERO, inject);
            f.poll(Duration::from_secs(120))
                .iter()
                .any(|e| matches!(e.kind, SpeechEventKind::AudioGap { .. }))
        };
        assert!(!has_gap(false));
        assert!(has_gap(true));
    }
}
