//! Cancel/restart behaviour against the real engine. Ignored by default
//! because it needs `models/ggml-base.en.bin` and a synthesized fixture:
//!
//! ```sh
//! cargo run --release -- fixtures
//! cargo test --release -- --ignored
//! ```

use std::time::Duration;

use voice_spike::audio::WavAudio;
use voice_spike::engine::EngineConfig;
use voice_spike::events::SpeechEventKind;
use voice_spike::feeder::{FeedOptions, feed};
use voice_spike::whisper_engine::WhisperEngine;
use voice_spike::{crate_dir, resolve_model};

#[test]
#[ignore = "needs a downloaded model and generated fixtures"]
fn late_events_keep_old_session_id_after_restart() {
    let model = resolve_model("base.en");
    let wav = crate_dir().join("fixtures/01_pydantic_Samantha_180.wav");
    assert!(model.exists(), "missing {}", model.display());
    assert!(wav.exists(), "missing {}", wav.display());

    let audio = WavAudio::load(&wav).unwrap();
    let mut engine = WhisperEngine::load(&model, EngineConfig::default()).unwrap();
    let result = feed(
        &mut engine,
        &audio,
        &FeedOptions {
            chunk_ms: 100,
            realtime: false,
            stop_after_ms: Some(2500),
            restart: true,
            drain_timeout: Duration::from_secs(60),
        },
    )
    .unwrap();

    assert!(!result.drain_timed_out);
    assert_eq!(result.sessions, vec![1, 2]);

    let events: Vec<_> = result.timeline.iter().map(|e| &e.event).collect();
    // Both sessions produced a Final, each tagged with its own session id.
    let finals: Vec<u64> = events
        .iter()
        .filter(|e| matches!(e.kind, SpeechEventKind::Final { .. }))
        .map(|e| e.session)
        .collect();
    assert!(
        finals.contains(&1) && finals.contains(&2),
        "finals: {finals:?}"
    );

    // Sequence numbers are strictly increasing within each session.
    for s in [1u64, 2] {
        let seqs: Vec<u64> = events
            .iter()
            .filter(|e| e.session == s)
            .map(|e| e.sequence)
            .collect();
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "session {s}: {seqs:?}"
        );
    }

    // At least one session-1 event was observed after session 2 started:
    // this is the "late event" case the document model must reject.
    let first_s2 = events.iter().position(|e| e.session == 2).unwrap();
    assert!(events[first_s2..].iter().any(|e| e.session == 1));
}

#[test]
#[ignore = "needs a downloaded model and generated fixtures"]
fn tiny_queue_reports_audio_gaps() {
    let model = resolve_model("base.en");
    let wav = crate_dir().join("fixtures/02_rust_egui_Samantha_180.wav");
    let audio = WavAudio::load(&wav).unwrap();
    let cfg = EngineConfig {
        audio_queue: 2,
        ..EngineConfig::default()
    };
    let mut engine = WhisperEngine::load(&model, cfg).unwrap();
    let result = feed(
        &mut engine,
        &audio,
        &FeedOptions {
            chunk_ms: 10,
            realtime: true,
            stop_after_ms: None,
            restart: false,
            drain_timeout: Duration::from_secs(60),
        },
    )
    .unwrap();
    let gaps = result
        .timeline
        .iter()
        .filter(|e| matches!(e.event.kind, SpeechEventKind::AudioGap { .. }))
        .count();
    assert!(
        result.dropped_chunks > 0,
        "expected the feeder to drop chunks"
    );
    assert!(gaps > 0, "expected AudioGap events");
}
