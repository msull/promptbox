//! Keyboard injection via `enigo` (Core Graphics events on macOS), gated
//! by the Accessibility permission.

use std::time::Duration;

use crate::ports::typist::Typist;

/// Pastes with ⌘V rather than typing characters: a multi-line prompt typed
/// key by key would submit at every newline, and pastes are instant.
#[derive(Default)]
pub struct SystemTypist {
    enigo: Option<enigo::Enigo>,
}

impl SystemTypist {
    fn enigo(&mut self) -> Result<&mut enigo::Enigo, String> {
        if self.enigo.is_none() {
            let e = enigo::Enigo::new(&enigo::Settings::default())
                .map_err(|e| format!("input connection: {e}"))?;
            self.enigo = Some(e);
        }
        Ok(self.enigo.as_mut().expect("initialised above"))
    }
}

impl Typist for SystemTypist {
    fn permission_granted(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            macos_accessibility_client::accessibility::application_is_trusted()
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }

    fn request_permission(&self) {
        #[cfg(target_os = "macos")]
        {
            macos_accessibility_client::accessibility::application_is_trusted_with_prompt();
        }
    }

    fn paste_and_submit(&mut self, submit: bool) -> Result<(), String> {
        use enigo::{Direction, Key, Keyboard};
        if !self.permission_granted() {
            return Err("Accessibility permission not granted".into());
        }
        let e = self.enigo()?;
        let err = |e: enigo::InputError| e.to_string();
        // Give the clipboard write a moment to land before pasting.
        std::thread::sleep(Duration::from_millis(60));
        e.key(Key::Meta, Direction::Press).map_err(err)?;
        let paste = e.key(Key::Unicode('v'), Direction::Click).map_err(err);
        e.key(Key::Meta, Direction::Release).map_err(err)?;
        paste?;
        if submit {
            std::thread::sleep(Duration::from_millis(80));
            e.key(Key::Return, Direction::Click).map_err(err)?;
        }
        Ok(())
    }
}

/// Test double that records calls and can fail.
#[derive(Debug, Default)]
pub struct FakeTypist {
    pub granted: bool,
    pub fail_with: Option<String>,
    pub pastes: Vec<bool>,
    pub permission_requests: usize,
}

impl Typist for FakeTypist {
    fn permission_granted(&self) -> bool {
        self.granted
    }

    fn request_permission(&self) {}

    fn paste_and_submit(&mut self, submit: bool) -> Result<(), String> {
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        self.pastes.push(submit);
        Ok(())
    }
}
