//! Voice spike library: engine boundary, whisper.cpp streaming emulation,
//! metrics, and the provisional-span document prototype.

pub mod audio;
pub mod doc;
pub mod engine;
pub mod events;
pub mod feeder;
pub mod metrics;
pub mod report;
pub mod vad;
pub mod whisper_engine;
pub mod worker;

use std::path::{Path, PathBuf};

/// Directory of this crate (fixtures and models live under it).
#[must_use]
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Resolves `base.en` -> `models/ggml-base.en.bin`, or passes a path through.
#[must_use]
pub fn resolve_model(name: &str) -> PathBuf {
    let p = Path::new(name);
    if p.exists() {
        return p.to_path_buf();
    }
    crate_dir().join("models").join(format!("ggml-{name}.bin"))
}

/// `01_pydantic_Samantha_180.wav` -> `01_pydantic`.
#[must_use]
pub fn script_stem(wav: &Path) -> String {
    let stem = wav.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let parts: Vec<&str> = stem.rsplitn(3, '_').collect();
    if parts.len() == 3 {
        parts[2].to_owned()
    } else {
        stem.to_owned()
    }
}

/// Reads a script and strips `[[...]]` speech-synthesis commands.
pub fn read_reference(path: &Path) -> anyhow::Result<String> {
    let raw = std::fs::read_to_string(path)?;
    Ok(strip_say_commands(&raw))
}

#[must_use]
pub fn strip_say_commands(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("[[") {
        out.push_str(&rest[..i]);
        match rest[i..].find("]]") {
            Some(j) => rest = &rest[i + j + 2..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_slnc_commands() {
        assert_eq!(strip_say_commands("a [[slnc 800]] b"), "a b");
        assert_eq!(strip_say_commands("no commands"), "no commands");
    }

    #[test]
    fn script_stem_drops_voice_and_rate() {
        assert_eq!(
            script_stem(Path::new("x/01_pydantic_Samantha_180.wav")),
            "01_pydantic"
        );
        assert_eq!(script_stem(Path::new("custom.wav")), "custom");
    }
}
