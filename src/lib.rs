//! Prompt Box library crate.
//!
//! The application lives in the library so that integration tests in
//! `tests/` can drive it. `src/main.rs` is a thin launcher.

pub mod app;

pub use app::PromptBoxApp;
