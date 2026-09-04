//! Persistence of sent prompts and the autosaved draft.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// An immutable snapshot taken when Send is pressed. `id` is stable so
/// retrying the same send never duplicates history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentPrompt {
    pub id: u64,
    pub text: String,
    pub sent_at: SystemTime,
    pub project: String,
}

pub trait HistoryStore {
    /// Appends (or, for an existing `id`, replaces) a sent prompt.
    fn save_sent(&mut self, prompt: &SentPrompt) -> Result<(), String>;
    fn load_recent(&mut self, limit: usize) -> Result<Vec<SentPrompt>, String>;
    fn save_draft(&mut self, text: &str) -> Result<(), String>;
    fn load_draft(&mut self) -> Result<Option<String>, String>;
}
