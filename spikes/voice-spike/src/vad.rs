//! Minimal energy VAD over fixed 20 ms frames. Enough to segment clean TTS
//! audio; a real microphone will need re-tuning or a model-based VAD.

pub const FRAME_LEN: usize = 320; // 20 ms at 16 kHz

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadTransition {
    /// Speech began; `sample` is the first sample of speech (after pre-roll).
    Started { sample: u64 },
    /// Speech ended; `sample` is the first sample of the trailing silence.
    Ended { sample: u64 },
}

#[derive(Debug)]
pub struct EnergyVad {
    threshold_db: f32,
    onset_frames: u32,
    hangover_frames: u32,
    preroll_samples: u64,
    speech_run: u32,
    silence_run: u32,
    in_speech: bool,
    frames_seen: u64,
}

impl EnergyVad {
    #[must_use]
    pub fn new(threshold_db: f32, onset_ms: u64, hangover_ms: u64, preroll_ms: u64) -> Self {
        let frames = |ms: u64| u32::try_from((ms / 20).max(1)).unwrap_or(u32::MAX);
        Self {
            threshold_db,
            onset_frames: frames(onset_ms),
            hangover_frames: frames(hangover_ms),
            preroll_samples: preroll_ms * 16,
            speech_run: 0,
            silence_run: 0,
            in_speech: false,
            frames_seen: 0,
        }
    }

    #[must_use]
    pub fn in_speech(&self) -> bool {
        self.in_speech
    }

    #[must_use]
    pub fn frame_db(frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return -120.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let mean = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
        20.0 * (mean.sqrt() + 1e-9).log10()
    }

    /// Feeds one complete frame. `frame_start` is the sample offset of the
    /// frame's first sample.
    pub fn push_frame(&mut self, frame: &[f32], frame_start: u64) -> Option<VadTransition> {
        self.frames_seen += 1;
        let speech = Self::frame_db(frame) > self.threshold_db;
        if speech {
            self.speech_run += 1;
            self.silence_run = 0;
        } else {
            self.silence_run += 1;
            self.speech_run = 0;
        }
        if !self.in_speech && self.speech_run >= self.onset_frames {
            self.in_speech = true;
            let onset = frame_start - u64::from(self.onset_frames - 1) * FRAME_LEN as u64;
            return Some(VadTransition::Started {
                sample: onset.saturating_sub(self.preroll_samples),
            });
        }
        if self.in_speech && self.silence_run >= self.hangover_frames {
            self.in_speech = false;
            let silence_start =
                frame_start - u64::from(self.hangover_frames - 1) * FRAME_LEN as u64;
            return Some(VadTransition::Ended {
                sample: silence_start,
            });
        }
        None
    }

    /// Forces the detector back to silence (used on forced splits/stop).
    pub fn reset(&mut self) {
        self.speech_run = 0;
        self.silence_run = 0;
        self.in_speech = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(vad: &mut EnergyVad, frames: &[(f32, usize)]) -> Vec<(u64, VadTransition)> {
        let mut out = Vec::new();
        let mut pos = 0u64;
        for &(amp, n) in frames {
            for _ in 0..n {
                let frame = vec![amp; FRAME_LEN];
                if let Some(t) = vad.push_frame(&frame, pos) {
                    out.push((pos, t));
                }
                pos += FRAME_LEN as u64;
            }
        }
        out
    }

    #[test]
    fn db_of_silence_is_very_low() {
        assert!(EnergyVad::frame_db(&[0.0; FRAME_LEN]) < -100.0);
        assert!(EnergyVad::frame_db(&[0.1; FRAME_LEN]) > -25.0);
    }

    #[test]
    fn onset_and_hangover_frame_counts() {
        // onset 60 ms = 3 frames, hangover 100 ms = 5 frames, no pre-roll.
        let mut vad = EnergyVad::new(-40.0, 60, 100, 0);
        let t = feed(&mut vad, &[(0.0, 10), (0.1, 20), (0.0, 10)]);
        assert_eq!(
            t,
            vec![
                (12 * 320, VadTransition::Started { sample: 10 * 320 }),
                (34 * 320, VadTransition::Ended { sample: 30 * 320 }),
            ]
        );
    }

    #[test]
    fn preroll_is_clamped_at_zero() {
        let mut vad = EnergyVad::new(-40.0, 20, 100, 500);
        let t = feed(&mut vad, &[(0.1, 5)]);
        assert_eq!(t, vec![(0, VadTransition::Started { sample: 0 })]);
    }

    #[test]
    fn short_blip_does_not_start_speech() {
        let mut vad = EnergyVad::new(-40.0, 60, 100, 0);
        let t = feed(&mut vad, &[(0.0, 5), (0.1, 2), (0.0, 10)]);
        assert!(t.is_empty());
    }
}
