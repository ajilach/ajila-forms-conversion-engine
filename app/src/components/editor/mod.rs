//! Structured document editor components.
//!
//! This module provides a graphical editor for modifying the structured
//! document output before final generation.

#[allow(clippy::module_inception)]
mod editor;
mod metadata_editor;
mod node_renderer;
mod state;
mod text_editor;
mod toolbar;

pub use editor::{EnvelopeWrapper, StructuredEditor};
