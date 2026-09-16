//! The native save panel ([`NativeSaver`]) and a scripted double for tests.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::ports::saver::FileSaver;

/// The file name the save panel suggests.
pub const SUGGESTED_NAME: &str = "prompt.md";

/// Opens the platform's save dialog on the main thread and writes the
/// file the user chose.
#[derive(Debug, Default)]
pub struct NativeSaver;

impl FileSaver for NativeSaver {
    fn save(&mut self, seed: &Path, text: &str) -> Result<Option<PathBuf>, String> {
        let Some(path) = rfd::FileDialog::new()
            .set_directory(seed)
            .set_file_name(SUGGESTED_NAME)
            .save_file()
        else {
            return Ok(None);
        };
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(path))
    }
}

/// One saved file as the fake saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub seed: PathBuf,
    pub path: PathBuf,
    pub text: String,
}

/// Picks `choose` (or cancels when `None`) instead of showing a dialog,
/// and records what would have been written.
#[derive(Debug, Default, Clone)]
pub struct FakeSaver {
    pub choose: Option<PathBuf>,
    pub fail_with: Option<String>,
    pub saved: Arc<Mutex<Vec<Saved>>>,
}

impl FileSaver for FakeSaver {
    fn save(&mut self, seed: &Path, text: &str) -> Result<Option<PathBuf>, String> {
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        let Some(path) = &self.choose else {
            return Ok(None);
        };
        self.saved.lock().expect("saver mutex").push(Saved {
            seed: seed.to_path_buf(),
            path: path.clone(),
            text: text.to_owned(),
        });
        Ok(Some(path.clone()))
    }
}
