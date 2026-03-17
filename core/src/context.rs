//! Context module for pipeline-wide data.
//!
//! The Context struct is passed through the entire processing pipeline,
//! collecting information from various stages and modules. It starts with
//! basic information (language, XFA variables) and is enriched by modules
//! throughout processing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Context that is passed through the entire processing pipeline.
///
/// This struct starts with basic information (language, variables extracted
/// from the XFA template) and is enriched by analysis modules as the document
/// is processed. The final context is included in the structured output as
/// metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// Document language (e.g., "de", "en", "fr"), extracted from the root
    /// subform's `locale` attribute per XFA 3.3 spec.
    language: String,

    /// All `<variables><text>` values from the XFA template, keyed by name.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, String>,
}

impl Context {
    /// Create a new context with the specified language and XFA variables.
    pub fn new(language: String, variables: HashMap<String, String>) -> Self {
        Self {
            language,
            variables,
        }
    }

    /// Create a new context with only a language (no XFA variables).
    ///
    /// Use this for contexts not backed by XFA data (e.g. translation merging,
    /// convenience test helpers).
    pub fn with_language(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            variables: HashMap::new(),
        }
    }

    /// The document language (e.g., "de", "en", "fr").
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Set the language (used by translation merger to combine languages).
    pub fn set_language(&mut self, language: String) {
        self.language = language;
    }

    /// Get an XFA variable value by name.
    pub fn get_variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let ctx = Context::with_language("de");
        assert_eq!(ctx.language(), "de");
        assert!(ctx.variables.is_empty());
    }

    #[test]
    fn test_context_with_variables() {
        let mut vars = HashMap::new();
        vars.insert("formrange_language".to_string(), "DE".to_string());
        vars.insert("formrange_code".to_string(), "AAEI".to_string());

        let ctx = Context::new("de".to_string(), vars);
        assert_eq!(ctx.language(), "de");
        assert_eq!(ctx.get_variable("formrange_language"), Some("DE"));
        assert_eq!(ctx.get_variable("formrange_code"), Some("AAEI"));
        assert_eq!(ctx.get_variable("nonexistent"), None);
    }

    #[test]
    fn test_context_serialization() {
        let mut vars = HashMap::new();
        vars.insert("formrange_language".to_string(), "FR".to_string());

        let ctx = Context::new("fr".to_string(), vars);

        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"language\":\"fr\""));
        assert!(json.contains("\"variables\""));
        assert!(json.contains("\"formrange_language\":\"FR\""));
    }

    #[test]
    fn test_context_serialization_empty_variables_omitted() {
        let ctx = Context::with_language("en");
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(!json.contains("\"variables\""));
    }
}
