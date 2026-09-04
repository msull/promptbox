//! AI rewrite capability: an explicit, user-requested transformation of the
//! whole prompt. Never part of the live transcription path.

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

pub trait Rewriter: Send + Sync {
    /// Blocking; the app runs this on a worker thread.
    fn rewrite(&self, request: &RewriteRequest) -> Result<RewriteResponse, String>;
}

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
