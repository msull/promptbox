//! Clipboard capability. A GUI request to copy is not proof the platform
//! accepted it, so the port reports success or failure.

pub trait Clipboard {
    fn write_text(&mut self, text: &str) -> Result<(), String>;
}
