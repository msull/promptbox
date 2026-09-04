//! whisper.cpp speech recognition: energy VAD segmentation, a recognition
//! worker per session, and emulated streaming partials. The defaults come
//! from the measurements in `spikes/voice-spike/README.md`.

pub mod engine;
pub mod vad;
pub mod worker;

pub use engine::WhisperEngine;
