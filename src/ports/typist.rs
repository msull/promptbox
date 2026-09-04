//! Keyboard injection into whatever app is focused. Used by Send so a
//! voice command can deliver the prompt straight into a chat box.

pub trait Typist: Send {
    /// Whether the OS will let this process synthesize input at all.
    fn permission_granted(&self) -> bool;
    /// Asks the OS to show its permission prompt, if it has one.
    fn request_permission(&self);
    /// Pastes the clipboard into the focused app (⌘V) and optionally
    /// presses Return afterwards.
    fn paste_and_submit(&mut self, submit: bool) -> Result<(), String>;
}
