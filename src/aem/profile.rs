//! AEM profile configuration loaded from a TOML file.
//!
//! Defines the schema for AEM output profiles and handles deserialization.
//! Profile values use [Tera](https://keats.github.io/tera/) template syntax
//! and can reference two namespaces:
//!
//! - `xfa.*`  — raw XFA `<variables><text>` values extracted from the PDF
//! - `variables.*` — user-defined intermediate values (themselves Tera templates)
//!
//! Runtime values injected automatically:
//! - `master_language` — primary language code (from profile)
//! - `languages` — comma-separated list of all detected language codes
//! - `author` — authoring user name

use serde::Deserialize;
use std::collections::HashMap;

/// An AEM output profile loaded from a TOML file.
///
/// All template-typed fields accept Tera syntax. Non-template fields are
/// plain strings passed through as-is.
#[derive(Debug, Clone, Deserialize)]
pub struct AemProfile {
    /// Master / primary language code (e.g. `"en"`, `"de"`).
    /// Default: `"en"`.
    pub master_language: Option<String>,

    /// Tera template for the human-readable form title / form code.
    /// Default: `"{{ xfa.formrange_code }}"`.
    pub title: Option<String>,

    /// Tera template for the JCR path segment between
    /// `content/forms/af/` and the form directory.
    pub form_path: Option<String>,

    /// Tera template for the JCR folder name
    /// (e.g. `"AF_{{ xfa.formrange_code }}"`).
    pub form_dir: Option<String>,

    /// Reusable intermediate variables. Each value is a Tera template that
    /// can reference `xfa.*` and previously resolved `variables.*`.
    #[serde(default)]
    pub variables: HashMap<String, String>,

    /// Language synonym mappings (e.g. `de = ["de-ch"]`).
    #[serde(default)]
    pub language_synonyms: HashMap<String, Vec<String>>,
}
