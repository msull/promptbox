//! Test double for [`PromptSink`]: remembers what was delivered, shared
//! with the test through an `Arc` so the harness keeps a handle.

use std::sync::{Arc, Mutex};

use crate::ports::sink::PromptSink;

#[derive(Debug, Default, Clone)]
pub struct FakeSink {
    pub delivered: Arc<Mutex<Vec<String>>>,
    pub fail_with: Option<String>,
}

impl PromptSink for FakeSink {
    fn deliver(&mut self, text: &str) -> Result<(), String> {
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        self.delivered
            .lock()
            .expect("sink mutex")
            .push(text.to_owned());
        Ok(())
    }
}
