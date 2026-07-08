//! Headless conversion-agent engine.
//!
//! This crate holds the framework-agnostic core that drives the form-conversion
//! engine via tools: the [`ConversionAgent`] (its tool catalog and executor),
//! the edit-history store ([`db`]), the per-profile reference store
//! ([`references`]), the AEM HTTP client ([`aem_client`]), and image encoding
//! helpers ([`image_encode`]).
//!
//! It carries **no UI (Dioxus) and no LLM** dependency, so it can be embedded in
//! the desktop app *and* in a standalone MCP server. The LLM agent loop that
//! streams turns and drives these tools lives in the consumer (the app), as does
//! any UI state.

pub mod aem_client;
pub mod aem_translated_edit;
pub mod conversion;
pub mod db;
pub mod image_encode;
pub mod references;

pub use conversion::{
    ANALYST_ADDENDUM, AUTHOR_ADDENDUM, ConversionAgent, REVIEWER_ADDENDUM, ReviewResult,
    SHARED_PREAMBLE, SYSTEM_PROMPT, ToolReply, aem_connection_from_settings, validate_package_bytes,
};
