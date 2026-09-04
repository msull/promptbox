//! Deterministic application core: no egui, no threads, no I/O. Everything
//! enters through [`AppCore::dispatch`] and leaves as [`Effect`]s.

pub mod action;
pub mod commands;
pub mod document;
pub mod project;
pub mod text;

pub use action::{
    AppAction, AppCore, CaptureKind, Clock, Effect, InstructionCapture, PendingTool, SessionStatus,
    Toast, describe_call,
};
pub use commands::Command;
pub use document::{Document, OverlapPolicy};
pub use project::{Correction, GlossaryEntry, Project};
