//! Structured document editor components.
//!
//! This module provides a graphical editor for modifying the structured
//! document output before final generation.

#[allow(clippy::module_inception)]
mod editor;
mod metadata_editor;
mod node_renderer;
pub mod smart_edit;
#[allow(dead_code)]
mod smart_edit_modal;
mod state;
mod text_editor;
mod toolbar;

pub use editor::{EnvelopeWrapper, StructuredEditor};
