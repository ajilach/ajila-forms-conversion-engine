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

    /// Tera template for the generated form XSD file path.
    ///
    /// This is a full JCR file path used in DAM metadata `xsdRef`
    /// (e.g. `"/content/dam/formsanddocuments/afforms_xsd/AFForms/AF_{{ variables.form_code }}.xsd"`).
    ///
    /// Required when `bind_to_xsd = true`.
    pub xsd_path: Option<String>,

    /// Reusable intermediate variables. Each value is a Tera template that
    /// can reference `xfa.*` and previously resolved `variables.*`.
    #[serde(default)]
    pub variables: HashMap<String, String>,

    /// Language synonym mappings (e.g. `de = ["de-ch"]`).
    #[serde(default)]
    pub language_synonyms: HashMap<String, Vec<String>>,

    /// When `true`, the generated AEM package will include the XSD schema and
    /// all form fields / panels will receive a `bindRef` attribute pointing to
    /// their corresponding XSD element path.
    ///
    /// Requires an XSD profile (`xsd/config.toml`) to be present alongside the
    /// AEM profile so that name resolution is consistent.  Default: `false`.
    pub bind_to_xsd: Option<bool>,

    /// When `true`, recursively scan the `fragments/` subdirectory of the AEM
    /// profile for fragment `.content.xml` files. Matched XSD types in the
    /// generated AEM node tree are replaced by fragment references.
    /// Default: `false`.
    pub use_fragments: Option<bool>,

    /// The DAM path prefix used in fragment `xsdRef` attributes
    /// (e.g. `"/content/dam/formsanddocuments/afforms_xsd/"`).
    /// This prefix is replaced with `xsd/types/` when resolving fragment
    /// XSD files locally.
    pub fragment_xsd_ref: Option<String>,

    /// JCR path prefix for constructing fragment `fragRef` values
    /// (e.g. `"/content/forms/af/"`).
    /// The `fragRef` is built as `{prefix}{relative_fragment_dir_path}`.
    pub fragment_ref_prefix: Option<String>,
}
