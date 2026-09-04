//! External tools ("plugins"): scripts the user registers with a manifest,
//! chosen and given arguments by the model, run by the app. The prompt text
//! is never part of a tool's argument schema; it is handed to the script
//! separately on every invocation.

use serde::{Deserialize, Serialize};

/// `tool.json` inside a tool's folder. `parameters` is a JSON Schema object
/// and is passed to the model verbatim, so anything `OpenAI` accepts works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolManifest {
    pub name: String,
    pub description: String,
    #[serde(default = "empty_object")]
    pub parameters: serde_json::Value,
    /// Program and arguments, resolved relative to the tool's folder.
    pub command: Vec<String>,
    /// Show the chosen call and wait for Run instead of running at once.
    #[serde(default)]
    pub review: bool,
    /// Folder the manifest was loaded from; the script's working directory.
    #[serde(skip)]
    pub dir: std::path::PathBuf,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

/// What the model decided: which tool and with which arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// What a script reported on stdout. Plain text becomes `message`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// Shown in the notification strip.
    #[serde(default)]
    pub message: String,
    /// When present, replaces the prompt (one undoable edit).
    #[serde(default)]
    pub replace_prompt: Option<String>,
}

/// What a script receives on stdin, as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInput {
    pub arguments: serde_json::Value,
    /// The whole prompt at the time of the call.
    pub prompt: String,
}

pub trait ToolRunner: Send + Sync {
    /// Blocking; the app runs this on a worker thread.
    fn run(&self, tool: &ToolManifest, input: &ToolInput) -> Result<ToolOutcome, String>;
}

/// Test double: records the call and returns a fixed outcome.
pub struct FakeToolRunner {
    pub outcome: Result<ToolOutcome, String>,
    pub calls: std::sync::Mutex<Vec<(String, ToolInput)>>,
}

impl FakeToolRunner {
    #[must_use]
    pub fn new(outcome: Result<ToolOutcome, String>) -> Self {
        Self {
            outcome,
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl ToolRunner for FakeToolRunner {
    fn run(&self, tool: &ToolManifest, input: &ToolInput) -> Result<ToolOutcome, String> {
        self.calls
            .lock()
            .unwrap()
            .push((tool.name.clone(), input.clone()));
        self.outcome.clone()
    }
}
