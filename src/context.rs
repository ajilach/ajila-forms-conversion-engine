//! Context module for pipeline-wide data.
//!
//! The Context struct is passed through the entire processing pipeline,
//! collecting information from various stages and modules. It starts with
//! basic information (language, XFA variables) and is enriched by modules
//! throughout processing.

use serde::{Serialize, Deserialize};
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
    
    /// Module-specific data, keyed by module name
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub modules: HashMap<String, ModuleData>,
}

impl Context {
    /// Create a new context with the specified language and XFA variables.
    pub fn new(language: String, variables: HashMap<String, String>) -> Self {
        Self {
            language,
            variables,
            modules: HashMap::new(),
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
            modules: HashMap::new(),
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
    
    /// Add or update module data in the context.
    pub fn set_module_data(&mut self, module_name: impl Into<String>, data: ModuleData) {
        self.modules.insert(module_name.into(), data);
    }
    
    /// Get module data by name.
    pub fn get_module_data(&self, module_name: &str) -> Option<&ModuleData> {
        self.modules.get(module_name)
    }
    
    /// Check if a module has stored data in the context.
    pub fn has_module_data(&self, module_name: &str) -> bool {
        self.modules.contains_key(module_name)
    }
}

/// Module-specific data that can be stored in the context.
/// 
/// This is a flexible container that allows modules to store arbitrary
/// JSON-serializable data in the context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModuleData {
    /// A simple string value
    String(String),
    /// A numeric value
    Number(f64),
    /// A boolean value
    Bool(bool),
    /// A structured object with key-value pairs
    Object(HashMap<String, serde_json::Value>),
    /// A JSON value for maximum flexibility
    Json(serde_json::Value),
}

impl From<String> for ModuleData {
    fn from(s: String) -> Self {
        ModuleData::String(s)
    }
}

impl From<&str> for ModuleData {
    fn from(s: &str) -> Self {
        ModuleData::String(s.to_string())
    }
}

impl From<f64> for ModuleData {
    fn from(n: f64) -> Self {
        ModuleData::Number(n)
    }
}

impl From<bool> for ModuleData {
    fn from(b: bool) -> Self {
        ModuleData::Bool(b)
    }
}

impl From<HashMap<String, serde_json::Value>> for ModuleData {
    fn from(obj: HashMap<String, serde_json::Value>) -> Self {
        ModuleData::Object(obj)
    }
}

impl From<serde_json::Value> for ModuleData {
    fn from(val: serde_json::Value) -> Self {
        ModuleData::Json(val)
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
        assert!(ctx.modules.is_empty());
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
    fn test_module_data() {
        let mut ctx = Context::with_language("en");
        
        // Add string data
        ctx.set_module_data("test_module", ModuleData::from("test value"));
        assert!(ctx.has_module_data("test_module"));
        
        // Retrieve and verify
        let data = ctx.get_module_data("test_module").unwrap();
        match data {
            ModuleData::String(s) => assert_eq!(s, "test value"),
            _ => panic!("Expected String variant"),
        }
    }
    
    #[test]
    fn test_context_serialization() {
        let mut vars = HashMap::new();
        vars.insert("formrange_language".to_string(), "FR".to_string());
        
        let mut ctx = Context::new("fr".to_string(), vars);
        ctx.set_module_data("example", ModuleData::from(42.0));
        
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"language\":\"fr\""));
        assert!(json.contains("\"variables\""));
        assert!(json.contains("\"formrange_language\":\"FR\""));
        assert!(json.contains("\"modules\""));
    }
    
    #[test]
    fn test_context_serialization_empty_variables_omitted() {
        let ctx = Context::with_language("en");
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(!json.contains("\"variables\""));
    }
}
