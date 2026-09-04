//! AI rewrite capability: an explicit, user-requested transformation of the
//! whole prompt. Never part of the live transcription path.

use crate::ports::tools::{ToolCall, ToolManifest};

/// One rewrite job. `content` is the full prompt; `instruction` is what to
/// do with it (a fixed clean-up instruction, or whatever the user typed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteRequest {
    pub id: u64,
    pub instruction: String,
    pub content: String,
    /// Project glossary and notes; empty when the project has none.
    pub context: String,
}

/// Outcome of a rewrite, including token usage so spend can be tracked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResponse {
    pub text: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Ask the model which registered tool a spoken request means, and with
/// which arguments. The prompt is supplied as context only; it is never a
/// tool argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolChoiceRequest {
    pub id: u64,
    pub request: String,
    pub prompt: String,
    pub context: String,
    pub tools: Vec<ToolManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolChoice {
    /// `None` when the model answered in words instead; `message` says why.
    pub call: Option<ToolCall>,
    pub message: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

pub trait Rewriter: Send + Sync {
    /// Blocking; the app runs this on a worker thread.
    fn rewrite(&self, request: &RewriteRequest) -> Result<RewriteResponse, String>;

    /// Blocking; picks a tool for the request.
    fn choose_tool(&self, request: &ToolChoiceRequest) -> Result<ToolChoice, String>;
}

/// System instruction for tool selection.
pub const TOOL_SYSTEM_PROMPT: &str = "You route a software developer's spoken request \
to one of the registered tools and fill in its arguments. The developer's current \
prompt text is provided for context: draw argument values from it when the request \
refers to it, but never copy the whole prompt into an argument; the tool receives the \
prompt separately. If no tool fits, reply in one short sentence saying so.";

/// The system instruction shared by every rewrite.
pub const SYSTEM_PROMPT: &str = "You edit text that a software developer dictated \
while composing a prompt for a coding agent. Apply the user's instruction to the \
text and return ONLY the resulting prompt text: no preamble, no explanation, no \
quotation marks, no markdown code fences.";

/// Instruction used by the one-click clean-up button.
pub const CLEAN_UP_INSTRUCTION: &str = "Clean up this speech-to-text transcript. \
Fix recognition errors, punctuation, capitalization, and sentence boundaries. \
Remove filler words, false starts, and repeated words. Keep the author's meaning, \
wording, and order; do not add ideas or content.";
