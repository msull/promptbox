//! Filesystem-backed history and draft storage. Writes are atomic (temp file
//! then rename); a corrupt history file is moved aside rather than lost.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ports::history::{HistoryStore, SentPrompt};

pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Platform data directory, e.g. `~/Library/Application Support/promptbox`.
    #[must_use]
    pub fn default_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "promptbox").map_or_else(
            || PathBuf::from(".promptbox"),
            |d| d.data_dir().to_path_buf(),
        )
    }

    fn history_path(&self) -> PathBuf {
        self.dir.join("history.json")
    }

    fn draft_path(&self) -> PathBuf {
        self.dir.join("draft.txt")
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| format!("create {}: {e}", self.dir.display()))?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))
    }

    fn read_history(&self) -> Result<Vec<SentPrompt>, String> {
        let path = self.history_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
    }

    fn quarantine_corrupt_history(&self) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let from = self.history_path();
        let to = self.dir.join(format!("history.corrupt-{stamp}.json"));
        if let Err(e) = fs::rename(&from, &to) {
            log::warn!("could not quarantine corrupt history: {e}");
        }
    }
}

impl HistoryStore for FileStore {
    fn save_sent(&mut self, prompt: &SentPrompt) -> Result<(), String> {
        let mut items = match self.read_history() {
            Ok(items) => items,
            Err(e) => {
                log::warn!("{e}; starting a fresh history file");
                self.quarantine_corrupt_history();
                Vec::new()
            }
        };
        items.retain(|p| p.id != prompt.id);
        items.insert(0, prompt.clone());
        items.truncate(crate::core::action::RECENT_LIMIT);
        let bytes = serde_json::to_vec_pretty(&items).map_err(|e| e.to_string())?;
        self.write_atomic(&self.history_path(), &bytes)
    }

    fn load_recent(&mut self, limit: usize) -> Result<Vec<SentPrompt>, String> {
        let mut items = self.read_history()?;
        items.truncate(limit);
        Ok(items)
    }

    fn save_draft(&mut self, text: &str) -> Result<(), String> {
        self.write_atomic(&self.draft_path(), text.as_bytes())
    }

    fn load_draft(&mut self) -> Result<Option<String>, String> {
        match fs::read_to_string(self.draft_path()) {
            Ok(s) if s.is_empty() => Ok(None),
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read draft: {e}")),
        }
    }
}

/// In-memory test double with injectable failures.
#[derive(Debug, Default)]
pub struct MemoryStore {
    pub sent: Vec<SentPrompt>,
    pub draft: Option<String>,
    pub fail_sent: Option<String>,
    pub fail_draft: Option<String>,
}

impl HistoryStore for MemoryStore {
    fn save_sent(&mut self, prompt: &SentPrompt) -> Result<(), String> {
        if let Some(e) = &self.fail_sent {
            return Err(e.clone());
        }
        self.sent.retain(|p| p.id != prompt.id);
        self.sent.insert(0, prompt.clone());
        Ok(())
    }

    fn load_recent(&mut self, limit: usize) -> Result<Vec<SentPrompt>, String> {
        Ok(self.sent.iter().take(limit).cloned().collect())
    }

    fn save_draft(&mut self, text: &str) -> Result<(), String> {
        if let Some(e) = &self.fail_draft {
            return Err(e.clone());
        }
        self.draft = Some(text.to_owned());
        Ok(())
    }

    fn load_draft(&mut self) -> Result<Option<String>, String> {
        Ok(self.draft.clone().filter(|s| !s.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(id: u64, text: &str) -> SentPrompt {
        SentPrompt {
            id,
            text: text.to_owned(),
            sent_at: UNIX_EPOCH,
            project: "Default".to_owned(),
        }
    }

    #[test]
    fn round_trips_history_and_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::new(dir.path().join("nested"));
        assert_eq!(store.load_recent(10).unwrap(), vec![]);
        assert_eq!(store.load_draft().unwrap(), None);
        store.save_sent(&prompt(1, "one")).unwrap();
        store.save_sent(&prompt(2, "two")).unwrap();
        store.save_sent(&prompt(1, "one again")).unwrap();
        let recent = store.load_recent(10).unwrap();
        assert_eq!(
            recent.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            ["one again", "two"]
        );
        store.save_draft("draft").unwrap();
        assert_eq!(store.load_draft().unwrap().as_deref(), Some("draft"));
        store.save_draft("").unwrap();
        assert_eq!(store.load_draft().unwrap(), None);
        assert!(!dir.path().join("nested/history.tmp").exists());
    }

    #[test]
    fn corrupt_history_is_reported_on_load_and_quarantined_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::new(dir.path().to_path_buf());
        fs::write(dir.path().join("history.json"), b"{not json").unwrap();
        assert!(store.load_recent(10).unwrap_err().contains("parse"));
        store.save_sent(&prompt(1, "fresh")).unwrap();
        assert_eq!(store.load_recent(10).unwrap()[0].text, "fresh");
        let quarantined = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("history.corrupt-")
            });
        assert!(quarantined);
    }
}
