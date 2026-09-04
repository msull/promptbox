//! Real-engine test against a synthesized fixture from the spike. Ignored
//! by default: needs the downloaded model and `spikes/voice-spike` fixtures.
//!
//! ```sh
//! (cd spikes/voice-spike && cargo run --release -- fixtures)
//! cargo test --release --test whisper -- --ignored
//! ```

use std::path::PathBuf;

use promptbox::adapters::model::{DEFAULT_MODEL, model_path};
use promptbox::adapters::speech::WhisperEngine;
use promptbox::ports::engine::{AudioChunk, EngineConfig, SpeechEngine};
use promptbox::ports::speech::SpeechEventKind;

#[test]
#[ignore = "needs a downloaded model and spike fixtures"]
fn transcribes_a_fixture_through_the_engine() {
    let model = model_path(DEFAULT_MODEL);
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("spikes/voice-spike/fixtures/03_three_sentences_Samantha_180.wav");
    assert!(model.exists(), "missing {}", model.display());
    assert!(wav.exists(), "missing {}", wav.display());

    let mut reader = hound::WavReader::open(&wav).unwrap();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();

    let mut engine = WhisperEngine::load(&model, EngineConfig::default()).unwrap();
    engine.start().unwrap();
    let mut finals = Vec::new();
    let collect = |engine: &mut WhisperEngine, finals: &mut Vec<String>| {
        for ev in engine.poll(64) {
            if let SpeechEventKind::Final { text, .. } = ev.kind {
                finals.push(text);
            }
        }
    };
    for (i, chunk) in samples.chunks(320).enumerate() {
        let chunk = AudioChunk {
            start_sample: (i * 320) as u64,
            samples: chunk.to_vec(),
        };
        while engine.push_audio(chunk.clone()).is_err() {
            collect(&mut engine, &mut finals);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    engine.stop();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        collect(&mut engine, &mut finals);
        if engine.is_drained() || std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let text = finals.join(" ").to_lowercase();
    assert!(text.contains("dynamodb"), "got: {text}");
    assert!(text.contains("unit tests"), "got: {text}");
}
