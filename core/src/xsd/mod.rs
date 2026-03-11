//! XSD (XML Schema Definition) Output Module
//!
//! Converts structured form nodes into an XSD schema that describes the form's
//! data model. Headings create nested `xs:complexType` hierarchies, fields
//! become `xs:element` declarations, conditional sections map to `xs:choice`,
//! and repeatable sections use `minOccurs`/`maxOccurs`.
//!
//! # Architecture
//!
//! ```text
//! StructuredNode ──► generate_xsd() ──► XSD String
//! ```
//!
//! # Profile configuration
//!
//! The module reads a TOML config from `profiles/{name}/xsd/config.toml` with:
//! - `[complexTypes.<name>]` — synonym mappings for headings → complexType objects
//! - `[elements.<name>]` — synonym mappings for fields → xs:element declarations
//!
//! Predefined `xs:simpleType` / `xs:complexType` definitions are auto-loaded
//! from `profiles/{name}/xsd/types/*.xsd`.

mod converter;

pub use converter::generate_xsd;

use serde::Deserialize;
use std::collections::HashMap;

// ============================================================================
// Profile types (TOML-deserializable)
// ============================================================================

/// TOML-deserializable XSD profile loaded from
/// `profiles/{name}/xsd/config.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct XsdProfile {
    /// Mapping from canonical complexType names to their config.
    #[serde(default)]
    pub complex_types: HashMap<String, ComplexTypeMapping>,

    /// Mapping from canonical element names to their config.
    #[serde(default)]
    pub elements: HashMap<String, ElementMapping>,

    /// External schema includes, keyed by a logical type name.
    /// Each entry generates an `<xs:include schemaLocation="..."/>` directive.
    #[serde(default)]
    pub includes: HashMap<String, IncludeMapping>,
}

/// Configuration for an external schema include.
///
/// Generates `<xs:include schemaLocation="{path}"/>` at the top of the
/// output XSD, before any type definitions or the root element.
#[derive(Debug, Clone, Deserialize)]
pub struct IncludeMapping {
    /// The `schemaLocation` path for the `xs:include` directive.
    pub path: String,
}

/// Configuration for a complexType synonym mapping.
///
/// When a heading's text matches one of the `synonyms`, the resulting XSD
/// element uses the canonical name (the TOML key). If `type_ref` is set,
/// the element references a predefined complexType by name instead of
/// generating an inline definition.
///
/// If `required_children` / `optional_children` are set, the heading only
/// matches this entry if its children (post-synonym-resolution) exactly
/// satisfy the constraint: all required present, no undeclared extras.
#[derive(Debug, Clone, Deserialize)]
pub struct ComplexTypeMapping {
    /// Synonym strings to match against heading text (case-insensitive substring).
    pub synonyms: Vec<String>,

    /// Optional reference to a predefined complexType name.
    /// If set, the element uses `type="<name>"` instead of generating inline children.
    #[serde(rename = "type")]
    pub type_ref: Option<String>,

    /// Canonical child names that MUST be present for this mapping to match.
    #[serde(default)]
    pub required_children: Option<Vec<String>>,

    /// Canonical child names that MAY be present (in addition to required).
    /// Any child not in `required_children ∪ optional_children` causes a mismatch.
    #[serde(default)]
    pub optional_children: Option<Vec<String>>,
}

/// Configuration for an element synonym mapping.
///
/// When a field label matches one of the `synonyms`, the resulting XSD
/// element uses the canonical name (the TOML key) and the specified type.
#[derive(Debug, Clone, Deserialize)]
pub struct ElementMapping {
    /// Synonym strings to match against field labels (case-insensitive substring).
    pub synonyms: Vec<String>,

    /// XSD type for this element (e.g. `"xs:string"`, `"xs:decimal"`, or a
    /// predefined type name like `"CurrencyType"`).
    #[serde(rename = "type")]
    pub type_ref: String,
}

// ============================================================================
// Resolved config (ready for generation)
// ============================================================================

/// Resolved XSD configuration, ready for schema generation.
///
/// Contains the parsed profile plus any predefined type definitions
/// loaded from the `types/` subdirectory.
#[derive(Debug, Clone)]
pub struct XsdConfig {
    /// The parsed profile.
    pub profile: XsdProfile,

    /// Raw XSD fragments from `types/*.xsd` files, to be included verbatim
    /// inside the output `<xs:schema>` element.
    pub predefined_types: Vec<String>,
}

impl XsdConfig {
    /// Build an `XsdConfig` from a profile and a list of predefined type fragments.
    pub fn new(profile: XsdProfile, predefined_types: Vec<String>) -> Self {
        Self {
            profile,
            predefined_types,
        }
    }

    /// Build an `XsdConfig` from just a profile (no predefined types).
    pub fn from_profile(profile: XsdProfile) -> Self {
        Self {
            profile,
            predefined_types: Vec::new(),
        }
    }
}

// ============================================================================
// Synonym resolution helpers
// ============================================================================

/// Result of resolving a heading label against the `[complexTypes]` config.
#[derive(Debug, Clone)]
pub struct ResolvedComplexType {
    /// The canonical name (TOML key).
    pub name: String,
    /// Optional predefined type reference.
    pub type_ref: Option<String>,
    /// The matching config entry (for child validation).
    pub mapping: ComplexTypeMapping,
}

/// Result of resolving a field label against the `[elements]` config.
#[derive(Debug, Clone)]
pub struct ResolvedElement {
    /// The canonical name (TOML key).
    pub name: String,
    /// The XSD type.
    pub type_ref: String,
}

/// Attempt to resolve a label against the `[complexTypes]` config.
///
/// Returns the first entry whose any synonym appears as a case-insensitive
/// substring of the label.
pub fn resolve_complex_type(label: &str, profile: &XsdProfile) -> Option<ResolvedComplexType> {
    let label_lower = label.to_lowercase();
    for (name, mapping) in &profile.complex_types {
        for synonym in &mapping.synonyms {
            if label_lower.contains(&synonym.to_lowercase()) {
                return Some(ResolvedComplexType {
                    name: name.clone(),
                    type_ref: mapping.type_ref.clone(),
                    mapping: mapping.clone(),
                });
            }
        }
    }
    None
}

/// Attempt to resolve a label against the `[elements]` config.
///
/// Returns the first entry whose any synonym appears as a case-insensitive
/// substring of the label.
pub fn resolve_element(label: &str, profile: &XsdProfile) -> Option<ResolvedElement> {
    let label_lower = label.to_lowercase();
    for (name, mapping) in &profile.elements {
        for synonym in &mapping.synonyms {
            if label_lower.contains(&synonym.to_lowercase()) {
                return Some(ResolvedElement {
                    name: name.clone(),
                    type_ref: mapping.type_ref.clone(),
                });
            }
        }
    }
    None
}

/// Convert a label string to a snake_case identifier suitable for XSD names.
///
/// - Strips non-alphanumeric characters
/// - Splits on whitespace
/// - All words are lowercased and joined with underscores
///
/// Example: `"Date of Birth"` → `"date_of_birth"`
pub fn to_snake_case(label: &str) -> String {
    let words: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    if words.is_empty() {
        return "unknown".to_string();
    }

    words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}
