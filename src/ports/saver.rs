//! Saving the prompt to a file the user picks: quick file creation from
//! the editor, for the notes and specs that never go to the clipboard.

use std::path::{Path, PathBuf};

/// Asks where to save (seeded at `seed`) and writes `text` there exactly,
/// no trailing newline added. `Ok(None)` when the user cancelled.
pub trait FileSaver {
    fn save(&mut self, seed: &Path, text: &str) -> Result<Option<PathBuf>, String>;
}
