//! whisper.cpp speech recognition: energy VAD segmentation, a recognition
//! worker per session, and emulated streaming partials. Ported from the
//! Milestone 0 spike (see `spikes/voice-spike/README.md` for the findings
//! that chose these defaults).

pub mod engine;
pub mod vad;
pub mod worker;

pub use engine::WhisperEngine;
