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
//! - `[elements.<name>]` — synonym mappings for fields → xs:element declarations
//! - `schemaLocationPrefix`  — prefix prepended to auto-discovered include paths
//!   (default: `"../"`)
//!
//! `xs:include` directives are generated automatically by indexing all `*.xsd`
//! files in `profiles/{name}/xsd/types/`. An include is emitted only when a
//! type declared in that file is actually referenced by the generated schema.
//!
//! Complex types are auto-matched: the `xs:complexType` definitions from the
//! `types/` directory are parsed (including `xs:extension` inheritance and
//! `xs:element ref` resolution) to build a registry of known types with their
//! child elements (name + type pairs). During generation, a heading's resolved
//! children are compared against this registry — if all children form a subset
//! of a registered type's elements, the best-matching type (most element
//! overlap) is used and the corresponding file is included via `xs:include`.

mod converter;

pub use converter::{BindRefMaps, compute_bind_refs, generate_xsd};

use quick_xml::Reader;
use quick_xml::events::Event;
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
// Registered complex types (auto-discovered from types/ directory)
// ============================================================================

/// A child element within a registered complex type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeChildElement {
    /// Element name (e.g. `"City"`).
    pub name: String,
    /// Element type (e.g. `"xs:string"`).
    pub type_ref: String,
}

/// A complex type registered from the `types/` directory.
///
/// Contains the flattened list of child elements (including those inherited
/// via `xs:extension base`), and the `schemaLocation` path of the file that
/// declares it.
#[derive(Debug, Clone)]
pub struct RegisteredComplexType {
    /// The complexType name (e.g. `"SignatureType"`).
    pub name: String,
    /// Flattened child elements (name + type), including inherited elements.
    pub elements: Vec<TypeChildElement>,
    /// The `schemaLocation` path for the file that declares this type.
    pub file: String,
}

// ============================================================================
// Resolved config (ready for generation)
// ============================================================================

/// Resolved XSD configuration, ready for schema generation.
///
/// Contains the parsed profile, a name→file index, and a registry of
/// complex types with their child elements for auto-matching.
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

    /// Registry of complex types with their flattened child elements.
    ///
    /// Built by parsing all `*.xsd` files in `types/`, extracting
    /// `xs:complexType` definitions with their child elements, and resolving
    /// `xs:extension base` inheritance and `xs:element ref` references.
    pub registered_types: HashMap<String, RegisteredComplexType>,
}

impl XsdConfig {
    /// Build an `XsdConfig` from a profile, a type-to-file index, and registered types.
    pub fn new(
        profile: XsdProfile,
        type_to_file: HashMap<String, String>,
        registered_types: HashMap<String, RegisteredComplexType>,
    ) -> Self {
        Self {
            profile,
            type_to_file,
            registered_types,
        }
    }

    /// Build an `XsdConfig` from just a profile (empty type index and registry).
    pub fn from_profile(profile: XsdProfile) -> Self {
        Self {
            profile,
            type_to_file: HashMap::new(),
            registered_types: HashMap::new(),
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
// XSD type parsing (using quick-xml)
// ============================================================================

/// Intermediate representation of a child element within a complexType,
/// before inheritance and ref resolution.
#[derive(Debug, Clone)]
enum RawChild {
    /// A child with explicit name and type: `<xs:element name="X" type="Y"/>`.
    Named { name: String, type_ref: String },
    /// A child referencing a global element: `<xs:element ref="X"/>`.
    Ref(String),
}

/// Intermediate representation of a complexType parsed from an XSD file.
#[derive(Debug, Clone)]
struct RawComplexType {
    /// The complexType name.
    name: String,
    /// Optional base type name from `xs:extension base="..."`.
    base: Option<String>,
    /// Direct child elements (before inheritance resolution).
    children: Vec<RawChild>,
}

/// Intermediate result of parsing a single XSD file.
#[derive(Debug, Clone)]
pub struct ParsedSchema {
    /// Complex types defined in this file.
    complex_types: Vec<RawComplexType>,
    /// Global element declarations: name → type.
    global_elements: HashMap<String, String>,
}

/// Parse an XSD file and extract complex types with their children
/// and global element declarations.
pub fn parse_schema(content: &str) -> ParsedSchema {
    let mut reader = Reader::from_str(content);

    let mut complex_types = Vec::new();
    let mut global_elements = HashMap::new();

    // Parsing state
    let mut depth: u32 = 0;
    let mut in_complex_type = false;
    let mut current_ct_name = String::new();
    let mut current_ct_base: Option<String> = None;
    let mut current_ct_children: Vec<RawChild> = Vec::new();
    let mut ct_start_depth: u32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local_name = e.local_name();
                let tag = std::str::from_utf8(local_name.as_ref()).unwrap_or("");

                match tag {
                    "complexType" => {
                        if let Some(name) = get_attr(e, b"name") {
                            in_complex_type = true;
                            current_ct_name = name;
                            current_ct_base = None;
                            current_ct_children = Vec::new();
                            ct_start_depth = depth;
                        }
                    }
                    "extension" if in_complex_type => {
                        if let Some(base) = get_attr(e, b"base") {
                            current_ct_base = Some(base);
                        }
                    }
                    "element" if in_complex_type => {
                        // Child element within a complexType
                        if let Some(ref_name) = get_attr(e, b"ref") {
                            current_ct_children.push(RawChild::Ref(ref_name));
                        } else if let Some(name) = get_attr(e, b"name") {
                            let type_ref =
                                get_attr(e, b"type").unwrap_or_else(|| "xs:string".to_string());
                            current_ct_children.push(RawChild::Named { name, type_ref });
                        }
                    }
                    "element" if !in_complex_type && depth <= 2 => {
                        // Global element declaration
                        if let Some(name) = get_attr(e, b"name") {
                            if let Some(type_ref) = get_attr(e, b"type") {
                                global_elements.insert(name, type_ref);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local_name = e.local_name();
                let tag = std::str::from_utf8(local_name.as_ref()).unwrap_or("");

                if tag == "element" {
                    if in_complex_type {
                        if let Some(ref_name) = get_attr(e, b"ref") {
                            current_ct_children.push(RawChild::Ref(ref_name));
                        } else if let Some(name) = get_attr(e, b"name") {
                            let type_ref =
                                get_attr(e, b"type").unwrap_or_else(|| "xs:string".to_string());
                            current_ct_children.push(RawChild::Named { name, type_ref });
                        }
                    } else if depth <= 1 {
                        // Global element declaration (self-closing)
                        if let Some(name) = get_attr(e, b"name") {
                            if let Some(type_ref) = get_attr(e, b"type") {
                                global_elements.insert(name, type_ref);
                            }
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                if in_complex_type && depth == ct_start_depth {
                    // Close current complexType
                    complex_types.push(RawComplexType {
                        name: current_ct_name.clone(),
                        base: current_ct_base.take(),
                        children: std::mem::take(&mut current_ct_children),
                    });
                    in_complex_type = false;
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    ParsedSchema {
        complex_types,
        global_elements,
    }
}

/// Extract an attribute value from a quick-xml event element.
fn get_attr(e: &quick_xml::events::BytesStart, attr_name: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == attr_name {
            return std::str::from_utf8(&attr.value).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Build the registered complex types registry from multiple parsed schemas.
///
/// This function:
/// 1. Collects all complex types and global elements from all parsed schemas.
/// 2. Resolves `xs:element ref="X"` references using global element declarations.
/// 3. Resolves `xs:extension base="Y"` inheritance by prepending base type elements.
/// 4. Associates each type with its `schemaLocation` path.
pub fn build_registered_types(
    parsed_schemas: &[(ParsedSchema, String)], // (schema, schemaLocation)
) -> HashMap<String, RegisteredComplexType> {
    // Collect all global elements across all files (for ref resolution)
    let mut all_global_elements: HashMap<String, String> = HashMap::new();
    for (schema, _) in parsed_schemas {
        for (name, type_ref) in &schema.global_elements {
            all_global_elements.insert(name.clone(), type_ref.clone());
        }
    }

    // Collect all raw complex types with their file paths
    let mut raw_types: HashMap<String, (RawComplexType, String)> = HashMap::new();
    for (schema, file) in parsed_schemas {
        for ct in &schema.complex_types {
            raw_types.insert(ct.name.clone(), (ct.clone(), file.clone()));
        }
    }

    // Resolve children: first resolve refs, then resolve inheritance
    let mut resolved: HashMap<String, RegisteredComplexType> = HashMap::new();

    // We need iterative resolution since inheritance can be multi-level
    // First pass: resolve refs for all types (without inheritance)
    let mut direct_elements: HashMap<String, Vec<TypeChildElement>> = HashMap::new();
    for (name, (ct, _)) in &raw_types {
        let mut elements = Vec::new();
        for child in &ct.children {
            match child {
                RawChild::Named {
                    name: n,
                    type_ref: t,
                } => {
                    elements.push(TypeChildElement {
                        name: n.clone(),
                        type_ref: t.clone(),
                    });
                }
                RawChild::Ref(ref_name) => {
                    // Look up the global element to get its type
                    let type_ref = all_global_elements
                        .get(ref_name)
                        .cloned()
                        .unwrap_or_else(|| "xs:string".to_string());
                    elements.push(TypeChildElement {
                        name: ref_name.clone(),
                        type_ref,
                    });
                }
            }
        }
        direct_elements.insert(name.clone(), elements);
    }

    // Resolve inheritance iteratively (handles multi-level chains)
    let mut inherited_elements: HashMap<String, Vec<TypeChildElement>> = HashMap::new();
    let max_iterations = raw_types.len();
    for _ in 0..max_iterations {
        let mut progress = false;
        for (name, (ct, _)) in &raw_types {
            if inherited_elements.contains_key(name) {
                continue;
            }
            match &ct.base {
                None => {
                    // No base → just use direct elements
                    inherited_elements.insert(name.clone(), direct_elements[name].clone());
                    progress = true;
                }
                Some(base_name) => {
                    // Base must be resolved first
                    if let Some(base_elements) = inherited_elements.get(base_name) {
                        let mut all_elements = base_elements.clone();
                        all_elements.extend(direct_elements[name].iter().cloned());
                        inherited_elements.insert(name.clone(), all_elements);
                        progress = true;
                    }
                    // else: base not yet resolved, try next iteration
                }
            }
        }
        if !progress {
            break;
        }
    }

    // Handle any types whose base couldn't be resolved (missing base type)
    for (name, (ct, _)) in &raw_types {
        if !inherited_elements.contains_key(name) {
            // Base type not found — use direct elements only
            inherited_elements.insert(name.clone(), direct_elements[name].clone());
            if ct.base.is_some() {
                // Silently ignore missing base types
            }
        }
    }

    // Build the final registry
    for (name, (_, file)) in &raw_types {
        if let Some(elements) = inherited_elements.get(name) {
            resolved.insert(
                name.clone(),
                RegisteredComplexType {
                    name: name.clone(),
                    elements: elements.clone(),
                    file: file.clone(),
                },
            );
        }
    }

    resolved
}

// ============================================================================
// Complex type matching
// ============================================================================

/// Find the best matching registered complex type for a set of child elements.
///
/// A registered type matches if all `children` (name + type) are present in
/// the registered type's elements (i.e. `children` is a subset).
/// If multiple types match, the one with the most element overlap is returned.
/// Returns `None` if no type matches or `children` is empty.
pub fn find_matching_type<'a>(
    children: &[(String, String)],
    registered_types: &'a HashMap<String, RegisteredComplexType>,
) -> Option<&'a RegisteredComplexType> {
    if children.is_empty() {
        return None;
    }

    let mut best_match: Option<(&RegisteredComplexType, usize)> = None;

    for reg_type in registered_types.values() {
        // Check if all children are a subset of the registered type's elements
        let all_match = children.iter().all(|(name, type_ref)| {
            reg_type
                .elements
                .iter()
                .any(|e| e.name == *name && e.type_ref == *type_ref)
        });

        if all_match {
            // Count overlap (how many of the registered type's elements match)
            let overlap = children
                .iter()
                .filter(|(name, type_ref)| {
                    reg_type
                        .elements
                        .iter()
                        .any(|e| e.name == *name && e.type_ref == *type_ref)
                })
                .count();

            match &best_match {
                None => best_match = Some((reg_type, overlap)),
                Some((_, best_overlap)) => {
                    if overlap > *best_overlap {
                        best_match = Some((reg_type, overlap));
                    }
                }
            }
        }
    }

    best_match.map(|(t, _)| t)
}

// ============================================================================
// Synonym resolution helpers
// ============================================================================

/// Result of resolving a field label against the `[elements]` config.
#[derive(Debug, Clone)]
pub struct ResolvedElement {
    /// The canonical name (TOML key).
    pub name: String,
    /// The XSD type.
    pub type_ref: String,
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
