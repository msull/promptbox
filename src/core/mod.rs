//! Deterministic application core: no egui, no threads, no I/O. Everything
//! enters through [`AppCore::dispatch`] and leaves as [`Effect`]s.

pub mod action;
pub mod document;
pub mod project;
pub mod text;

pub use action::{AppAction, AppCore, Clock, Effect, SessionStatus, Toast};
pub use document::{Document, OverlapPolicy};
pub use project::Project;
