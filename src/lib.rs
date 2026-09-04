//! Prompt Box library crate.
//!
//! Layering (see `voice-prompt-workbench-design.md`):
//! `core` is deterministic and egui-free; `ports` are the traits it needs;
//! `adapters` implement them; `app` wires them together and runs effects;
//! `ui` draws. `src/main.rs` is a thin launcher.

pub mod adapters;
pub mod app;
pub mod core;
pub mod ports;
pub mod ui;

pub use app::PromptBoxApp;
