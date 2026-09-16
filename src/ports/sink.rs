//! Where a Send goes when Prompt Box is embedded in another app: the
//! host takes the finished prompt directly, and the clipboard is left
//! alone.

/// Receives the prompt on Send. The result decides whether the editor
/// clears: a failure keeps the prompt, as a failed clipboard write does.
pub trait PromptSink {
    fn deliver(&mut self, text: &str) -> Result<(), String>;
}
