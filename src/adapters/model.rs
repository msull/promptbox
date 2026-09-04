//! Whisper model files: where they live and how to fetch one on first run.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};

pub const DEFAULT_MODEL: &str = "base.en";
const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

#[must_use]
pub fn models_dir() -> PathBuf {
    crate::adapters::persistence::FileStore::default_dir().join("models")
}

#[must_use]
pub fn model_path(name: &str) -> PathBuf {
    models_dir().join(format!("ggml-{name}.bin"))
}

#[must_use]
pub fn model_url(name: &str) -> String {
    format!("{BASE_URL}/ggml-{name}.bin")
}

/// A background download. Poll `progress()` for the UI and `try_result()`
/// once per frame; the file is renamed into place only when complete.
pub struct Download {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    result: Receiver<Result<PathBuf, String>>,
}

impl Download {
    #[must_use]
    pub fn start(name: &str) -> Self {
        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let (d, t) = (Arc::clone(&done), Arc::clone(&total));
        let url = model_url(name);
        let target = model_path(name);
        std::thread::Builder::new()
            .name("model-download".into())
            .spawn(move || {
                let _ = tx.send(fetch(&url, &target, &d, &t));
            })
            .expect("spawn download thread");
        Self {
            done,
            total,
            result: rx,
        }
    }

    /// (bytes done, bytes total or 0 if unknown).
    #[must_use]
    pub fn progress(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    #[must_use]
    pub fn try_result(&self) -> Option<Result<PathBuf, String>> {
        self.result.try_recv().ok()
    }
}

fn fetch(
    url: &str,
    target: &std::path::Path,
    done: &AtomicU64,
    total: &AtomicU64,
) -> Result<PathBuf, String> {
    let dir = target.parent().ok_or("model path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    if let Some(len) = response.body().content_length() {
        total.store(len, Ordering::Relaxed);
    }
    let part = target.with_extension("part");
    let mut file =
        std::fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write: {e}"))?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&part, target).map_err(|e| format!("rename: {e}"))?;
    Ok(target.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_urls_follow_ggml_naming() {
        assert!(model_path("base.en").ends_with("models/ggml-base.en.bin"));
        assert_eq!(
            model_url("small.en"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
        );
    }
}
