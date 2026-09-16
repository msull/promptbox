//! Prompt Box library crate.
//!
//! Layering (see `voice-prompt-workbench-design.md`):
//! `core` is deterministic and egui-free; `ports` are the traits it needs;
//! `adapters` implement them; `app` holds the per-prompt `Editor` and the
//! standalone window, `voice` the process-wide speech runtime; `ui` draws
//! (`caption` and `preview` draw the on-screen overlays).
//! `src/main.rs` is a thin launcher.

pub mod adapters;
pub mod app;
pub mod caption;
pub mod core;
pub mod ports;
pub mod preview;
pub mod ui;
pub mod voice;

pub use app::{Editor, PromptBoxApp};
pub use voice::Voice;
