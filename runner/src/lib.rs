//! The host side of a conversion run, shared by everything that drives one.
//!
//! `pipeline` sequences the roles but names no provider and no UI. This crate is
//! the other half: the Anthropic transport ([`llm`]), the operator settings that
//! configure it ([`settings`]), the [`pipeline::TurnProvider`] binding the two
//! together ([`turns`]), and the run entry points ([`run`]) that build the agent,
//! open an edit-history session and record the result.
//!
//! So the desktop app and the CLI start a run through the same code and differ
//! only in how they report it and where they put the artefacts.

pub mod artifacts;
pub mod llm;
pub mod run;
pub mod settings;
pub mod turns;

pub use artifacts::{Artifact, artifact_filename};
pub use run::{Completed, RunOptions, run_feedback, run_fresh};
pub use settings::AppSettings;
pub use turns::{AnthropicTurns, TurnPlan};
