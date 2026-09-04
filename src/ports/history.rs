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

/// Light/dark appearance; `Auto` follows the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Auto,
    Light,
    Dark,
}

/// Small persisted UI preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Keep the window above other windows.
    #[serde(default)]
    pub always_on_top: bool,
    /// Voice-command trigger word; empty means the built-in default.
    #[serde(default)]
    pub trigger: String,
    /// `OpenAI` API key entered in the UI; empty means use the environment.
    #[serde(default)]
    pub openai_api_key: String,
    /// `OpenAI` model for rewrites; empty means the built-in default.
    #[serde(default)]
    pub openai_model: String,
    #[serde(default)]
    pub theme: ThemeChoice,
    /// Send pastes the prompt into the focused app (needs Accessibility).
    #[serde(default = "default_true")]
    pub type_on_send: bool,
    /// Press Return after pasting.
    #[serde(default = "default_true")]
    pub submit_after_paste: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            always_on_top: false,
            trigger: String::new(),
            openai_api_key: String::new(),
            openai_model: String::new(),
            theme: ThemeChoice::default(),
            type_on_send: true,
            submit_after_paste: true,
        }
    }
}

pub trait HistoryStore {
    fn load_settings(&mut self) -> Result<Settings, String>;
    fn save_settings(&mut self, settings: &Settings) -> Result<(), String>;
    /// Appends (or, for an existing `id`, replaces) a sent prompt.
    fn save_sent(&mut self, prompt: &SentPrompt) -> Result<(), String>;
    fn load_recent(&mut self, limit: usize) -> Result<Vec<SentPrompt>, String>;
    fn save_draft(&mut self, text: &str) -> Result<(), String>;
    fn load_draft(&mut self) -> Result<Option<String>, String>;
}
