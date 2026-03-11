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
//! - `schemaLocationPrefix`  — prefix prepended to auto-discovered include paths
//!   (default: `"../"`)
//!
//! `xs:include` directives are generated automatically by indexing all `*.xsd`
//! files in `profiles/{name}/xsd/types/`. An include is emitted only when a
//! type declared in that file is actually referenced by the generated schema.

mod converter;

pub use converter::{BindRefMaps, compute_bind_refs, generate_xsd};

use serde::Deserialize;
use std::collections::HashMap;

// ============================================================================
// Profile types (TOML-deserializable)
// ============================================================================

fn default_schema_location_prefix() -> String {
    "../".to_string()
}

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

    /// Prefix prepended to every auto-discovered include path.
    ///
    /// For example, if the types directory contains `AFFragments/Signature.xsd`
    /// and `schemaLocationPrefix = "../"`, the generated include will be:
    /// `<xs:include schemaLocation="../AFFragments/Signature.xsd"/>`.
    ///
    /// Defaults to `"../"`.
    #[serde(default = "default_schema_location_prefix")]
    pub schema_location_prefix: String,
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
/// Contains the parsed profile plus an index mapping every declared XSD
/// type/element name to the `schemaLocation` string of the file that declares it.
/// This is built automatically by scanning the `types/` subdirectory.
#[derive(Debug, Clone)]
pub struct XsdConfig {
    /// The parsed profile.
    pub profile: XsdProfile,

    /// Index from declared type/element name → `schemaLocation` path.
    ///
    /// Built from all `*.xsd` files found recursively under `types/`
    /// using [`extract_declared_names`]. Only entries whose key appears in
    /// `used_type_refs` during generation produce an `<xs:include>` directive.
    pub type_to_file: HashMap<String, String>,
}

impl XsdConfig {
    /// Build an `XsdConfig` from a profile and a type-to-file index.
    pub fn new(profile: XsdProfile, type_to_file: HashMap<String, String>) -> Self {
        Self {
            profile,
            type_to_file,
        }
    }

    /// Build an `XsdConfig` from just a profile (empty type index).
    pub fn from_profile(profile: XsdProfile) -> Self {
        Self {
            profile,
            type_to_file: HashMap::new(),
        }
    }
}

/// Extract the names of all globally declared `xs:complexType`, `xs:simpleType`,
/// and `xs:element` definitions from an XSD file's text content.
///
/// Uses a simple line-by-line scan rather than full XML parsing.
/// Each matched line must start (after trimming) with one of the three tag
/// prefixes and contain a `name="..."` attribute on the same line.
///
/// # Example
///
/// Given the fragment:
/// ```xml
/// <xs:complexType name="SignatureType">
///   <xs:sequence>
///     <xs:element name="Place" type="xs:string" minOccurs="0"/>
/// ```
/// This returns `["SignatureType"]` (the element `"Place"` is not a global
/// declaration so it is typically inside a complexType body and still matched;
/// callers should ensure the file only contains global-scope declarations that
/// they actually want indexed, which is true for the `types/` convention).
pub fn extract_declared_names(content: &str) -> Vec<String> {
    const PREFIXES: &[&str] = &["<xs:complexType ", "<xs:simpleType ", "<xs:element "];

    let mut names = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            continue;
        }
        if let Some(name_start) = trimmed.find("name=\"") {
            let after = &trimmed[name_start + 6..];
            if let Some(name_end) = after.find('"') {
                names.push(after[..name_end].to_string());
            }
        }
    }
    names
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
