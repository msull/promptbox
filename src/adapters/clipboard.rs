//! System clipboard via `arboard`, created lazily so a headless environment
//! fails at first use rather than at startup.

use crate::ports::clipboard::Clipboard;

#[derive(Default)]
pub struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard for SystemClipboard {
    fn write_text(&mut self, text: &str) -> Result<(), String> {
        if self.inner.is_none() {
            self.inner = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        let cb = self.inner.as_mut().expect("initialised above");
        cb.set_text(text.to_owned()).map_err(|e| {
            self.inner = None; // force re-init next time
            e.to_string()
        })
    }
}

/// Test double: records writes, optionally fails.
#[derive(Debug, Default)]
pub struct FakeClipboard {
    pub writes: Vec<String>,
    pub fail_with: Option<String>,
}

impl Clipboard for FakeClipboard {
    fn write_text(&mut self, text: &str) -> Result<(), String> {
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        self.writes.push(text.to_owned());
        Ok(())
    }
}
