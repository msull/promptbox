//! Filesystem-backed history and draft storage. Writes are atomic (temp file
//! then rename); a corrupt history file is moved aside rather than lost.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::project::Project;
use crate::ports::history::{HistoryStore, SentPrompt, Settings};

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

    fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    fn projects_path(&self) -> PathBuf {
        self.dir.join("projects.json")
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
    fn load_settings(&mut self) -> Result<Settings, String> {
        match fs::read(self.settings_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("parse settings: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!("read settings: {e}")),
        }
    }

    fn save_settings(&mut self, settings: &Settings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        self.write_atomic(&self.settings_path(), &bytes)
    }

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

    fn load_projects(&mut self) -> Result<Vec<Project>, String> {
        let path = self.projects_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
    }

    fn save_projects(&mut self, projects: &[Project]) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(projects).map_err(|e| e.to_string())?;
        self.write_atomic(&self.projects_path(), &bytes)
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
    pub settings: Settings,
    pub projects: Vec<Project>,
}

impl HistoryStore for MemoryStore {
    fn load_settings(&mut self) -> Result<Settings, String> {
        Ok(self.settings.clone())
    }

    fn save_settings(&mut self, settings: &Settings) -> Result<(), String> {
        self.settings = settings.clone();
        Ok(())
    }

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

    fn load_projects(&mut self) -> Result<Vec<Project>, String> {
        Ok(self.projects.clone())
    }

    fn save_projects(&mut self, projects: &[Project]) -> Result<(), String> {
        self.projects = projects.to_vec();
        Ok(())
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
        assert_eq!(store.load_settings().unwrap(), Settings::default());
        store
            .save_settings(&Settings {
                always_on_top: true,
                ..Settings::default()
            })
            .unwrap();
        assert!(store.load_settings().unwrap().always_on_top);
        // Older settings files without newer fields still load.
        fs::write(
            dir.path().join("nested/settings.json"),
            b"{\"always_on_top\":false}",
        )
        .unwrap();
        assert_eq!(
            store.load_settings().unwrap().theme,
            crate::ports::history::ThemeChoice::Auto
        );
        assert!(!dir.path().join("nested/history.tmp").exists());
    }

    #[test]
    fn projects_round_trip_and_start_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::new(dir.path().to_path_buf());
        assert_eq!(store.load_projects().unwrap(), Vec::<Project>::new());
        let mut p = Project::new("Acme");
        p.vocabulary = vec!["Univer Sheets".into()];
        p.corrections = vec![crate::core::project::Correction {
            from: "you never sheets".into(),
            to: "Univer Sheets".into(),
        }];
        p.context = "A spreadsheet.".into();
        store.save_projects(&[p.clone()]).unwrap();
        assert_eq!(store.load_projects().unwrap(), vec![p]);
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
