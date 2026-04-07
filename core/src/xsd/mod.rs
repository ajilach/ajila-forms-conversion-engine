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

pub use converter::{BindRefMaps, compute_bind_refs, generate_xsd, generate_xsd_schema};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::util::escape_html as xml_escape;

// ============================================================================
// XSD node types (intermediate representation)
// ============================================================================

/// An XSD restriction facet.
#[derive(Debug, Clone, PartialEq)]
pub enum XsdRestriction {
    Pattern(String),
    MinLength(usize),
    MaxLength(usize),
    MinInclusive(String),
    MaxInclusive(String),
    Enumeration(String),
}

/// An XSD node in the intermediate schema tree.
#[derive(Debug, Clone, PartialEq)]
pub enum XsdNode {
    /// `<xs:element name="..." type="..." .../>`
    Element {
        name: String,
        type_ref: Option<String>,
        min_occurs: Option<u32>,
        max_occurs: Option<Option<u32>>,
        content: Option<Box<XsdNode>>,
    },
    /// `<xs:complexType>` (inline when `name` is `None`, named when `Some`)
    ComplexType {
        name: Option<String>,
        sequence: Vec<XsdNode>,
    },
    /// `<xs:simpleType>` with restriction facets
    SimpleType {
        base: String,
        restrictions: Vec<XsdRestriction>,
    },
    /// `<xs:choice>` — each option is one `<xs:sequence>` branch
    Choice { options: Vec<Vec<XsdNode>> },
}

/// A complete XSD schema with includes and a root element.
#[derive(Debug, Clone, PartialEq)]
pub struct XsdSchema {
    pub includes: Vec<String>,
    pub root: XsdNode,
}

impl XsdSchema {
    /// Serialize to a complete XSD XML string.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str("<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\n");

        for path in &self.includes {
            out.push_str(&format!("  <xs:include schemaLocation=\"{}\"/>\n", path));
        }
        if !self.includes.is_empty() {
            out.push('\n');
        }

        self.root.write_xml(&mut out, 2);

        out.push_str("</xs:schema>\n");
        out
    }
}

impl XsdNode {
    /// Write this node as XSD XML into `out` at the given indentation level.
    pub fn write_xml(&self, out: &mut String, indent: usize) {
        let pad = " ".repeat(indent);
        match self {
            XsdNode::Element {
                name,
                type_ref,
                min_occurs,
                max_occurs,
                content,
            } => {
                let occur = build_occurrence_attrs(*min_occurs, *max_occurs);
                match (type_ref, content) {
                    (Some(tr), None) => {
                        out.push_str(&format!(
                            "{}<xs:element name=\"{}\" type=\"{}\"{}/>",
                            pad, name, tr, occur
                        ));
                        out.push('\n');
                    }
                    (None, Some(child)) => {
                        out.push_str(&format!("{}<xs:element name=\"{}\"{}>\n", pad, name, occur));
                        child.write_xml(out, indent + 2);
                        out.push_str(&format!("{}</xs:element>\n", pad));
                    }
                    _ => {
                        // Fallback: element with no type and no content
                        out.push_str(&format!(
                            "{}<xs:element name=\"{}\" type=\"xs:string\"{}/>",
                            pad, name, occur
                        ));
                        out.push('\n');
                    }
                }
            }
            XsdNode::ComplexType { name, sequence } => {
                match name {
                    Some(n) => out.push_str(&format!("{}<xs:complexType name=\"{}\">\n", pad, n)),
                    None => out.push_str(&format!("{}<xs:complexType>\n", pad)),
                }
                out.push_str(&format!("{}  <xs:sequence>\n", pad));
                for child in sequence {
                    child.write_xml(out, indent + 4);
                }
                out.push_str(&format!("{}  </xs:sequence>\n", pad));
                out.push_str(&format!("{}</xs:complexType>\n", pad));
            }
            XsdNode::SimpleType { base, restrictions } => {
                out.push_str(&format!("{}<xs:simpleType>\n", pad));
                out.push_str(&format!("{}  <xs:restriction base=\"{}\">\n", pad, base));
                for r in restrictions {
                    r.write_xml(out, indent + 4);
                }
                out.push_str(&format!("{}  </xs:restriction>\n", pad));
                out.push_str(&format!("{}</xs:simpleType>\n", pad));
            }
            XsdNode::Choice { options } => {
                out.push_str(&format!("{}<xs:choice>\n", pad));
                for option in options {
                    out.push_str(&format!("{}  <xs:sequence>\n", pad));
                    for child in option {
                        child.write_xml(out, indent + 4);
                    }
                    out.push_str(&format!("{}  </xs:sequence>\n", pad));
                }
                out.push_str(&format!("{}</xs:choice>\n", pad));
            }
        }
    }
}

impl XsdRestriction {
    fn write_xml(&self, out: &mut String, indent: usize) {
        let pad = " ".repeat(indent);
        match self {
            XsdRestriction::Pattern(v) => {
                out.push_str(&format!(
                    "{}<xs:pattern value=\"{}\"/>\n",
                    pad,
                    xml_escape(v)
                ));
            }
            XsdRestriction::MinLength(v) => {
                out.push_str(&format!("{}<xs:minLength value=\"{}\"/>\n", pad, v));
            }
            XsdRestriction::MaxLength(v) => {
                out.push_str(&format!("{}<xs:maxLength value=\"{}\"/>\n", pad, v));
            }
            XsdRestriction::MinInclusive(v) => {
                out.push_str(&format!("{}<xs:minInclusive value=\"{}\"/>\n", pad, v));
            }
            XsdRestriction::MaxInclusive(v) => {
                out.push_str(&format!("{}<xs:maxInclusive value=\"{}\"/>\n", pad, v));
            }
            XsdRestriction::Enumeration(v) => {
                out.push_str(&format!(
                    "{}<xs:enumeration value=\"{}\"/>\n",
                    pad,
                    xml_escape(v)
                ));
            }
        }
    }
}

/// Build `minOccurs`/`maxOccurs` attribute string for an element.
fn build_occurrence_attrs(min_occurs: Option<u32>, max_occurs: Option<Option<u32>>) -> String {
    let mut attrs = String::new();
    if let Some(min) = min_occurs {
        if min != 1 {
            attrs.push_str(&format!(" minOccurs=\"{}\"", min));
        }
    }
    if let Some(max) = max_occurs {
        match max {
            Some(n) => {
                if n != 1 {
                    attrs.push_str(&format!(" maxOccurs=\"{}\"", n));
                }
            }
            None => {
                attrs.push_str(" maxOccurs=\"unbounded\"");
            }
        }
    }
    attrs
}

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

    /// Optional master language code used for element name resolution.
    ///
    /// When set (for example via `masterLanguage = "en"` in
    /// `xsd/config.toml`), element names derived from multilingual labels
    /// prefer that language.
    #[serde(default)]
    pub master_language: Option<String>,
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

    /// Reverse map from complex type name → global element name.
    ///
    /// E.g. `"AddressType" → "Address"`, `"IndividualBasicType" → "IndividualBasic"`.
    /// Built from global element declarations in the parsed XSD files.
    pub type_to_element_name: HashMap<String, String>,

    /// Optional master language code (e.g. `"en"`).
    ///
    /// When set, element names derived from headings and field labels will
    /// prefer the translation in this language instead of picking an
    /// arbitrary first entry from the translation map.
    pub master_language: Option<String>,
}

impl XsdConfig {
    /// Build an `XsdConfig` from a profile, a type-to-file index, and registered types.
    pub fn new(
        profile: XsdProfile,
        type_to_file: HashMap<String, String>,
        registered_types: HashMap<String, RegisteredComplexType>,
        type_to_element_name: HashMap<String, String>,
    ) -> Self {
        let master_language = profile.master_language.clone();
        Self {
            profile,
            type_to_file,
            registered_types,
            type_to_element_name,
            master_language,
        }
    }

    /// Build an `XsdConfig` from just a profile (empty type index and registry).
    pub fn from_profile(profile: XsdProfile) -> Self {
        let master_language = profile.master_language.clone();
        Self {
            profile,
            type_to_file: HashMap::new(),
            registered_types: HashMap::new(),
            type_to_element_name: HashMap::new(),
            master_language,
        }
    }

    /// Set the master language for element name resolution.
    pub fn with_master_language(mut self, lang: impl Into<String>) -> Self {
        self.master_language = Some(lang.into());
        self
    }

    /// Get the plain text from an `InlineText`, preferring the master
    /// language when available.
    pub fn label_text(&self, text: &crate::structured::InlineText) -> String {
        match &self.master_language {
            Some(lang) => text.plain_text_in(lang),
            None => text.as_plain_text(),
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
///
/// Returns `(registered_types, type_to_element_name)` where `type_to_element_name`
/// maps complex type names to their global element names (e.g. `"AddressType" → "Address"`).
pub fn build_registered_types(
    parsed_schemas: &[(ParsedSchema, String)], // (schema, schemaLocation)
) -> (
    HashMap<String, RegisteredComplexType>,
    HashMap<String, String>,
) {
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

    // Build reverse map: type name → element name (e.g. "AddressType" → "Address")
    let mut type_to_element_name: HashMap<String, String> = HashMap::new();
    for (elem_name, type_name) in &all_global_elements {
        type_to_element_name.insert(type_name.clone(), elem_name.clone());
    }

    (resolved, type_to_element_name)
}

/// Build a full [`XsdConfig`] from pre-discovered type source files.
///
/// `sources` is a list of `(relative_path_from_types_dir, xsd_content)` pairs,
/// for example `("AFFragments/Address.xsd", "<xs:schema ...")`.
pub fn build_xsd_config_from_type_sources(
    profile: XsdProfile,
    sources: &[(String, String)],
) -> XsdConfig {
    let mut type_to_file = HashMap::new();
    let mut parsed_schemas = Vec::new();

    for (relative_path, content) in sources {
        let schema_location = format!("{}{}", profile.schema_location_prefix, relative_path);
        for name in extract_declared_names(content) {
            type_to_file.insert(name, schema_location.clone());
        }
        parsed_schemas.push((parse_schema(content), schema_location));
    }

    let (registered_types, type_to_element_name) = build_registered_types(&parsed_schemas);
    XsdConfig::new(
        profile,
        type_to_file,
        registered_types,
        type_to_element_name,
    )
}

/// Collect all `*.xsd` files recursively from `types_dir` and return
/// `(relative_path_from_types_dir, file_content)` tuples.
pub fn collect_xsd_type_sources_from_dir(
    types_dir: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut xsd_files: Vec<PathBuf> = Vec::new();
    walk_xsd_files(types_dir, &mut xsd_files);
    xsd_files.sort();

    let mut sources = Vec::new();
    for xsd_path in &xsd_files {
        let rel = xsd_path.strip_prefix(types_dir).unwrap_or(xsd_path);
        let rel = path_to_forward_slash_string(rel);
        let content = std::fs::read_to_string(xsd_path)
            .map_err(|e| format!("Failed to read type file '{}': {}", xsd_path.display(), e))?;
        sources.push((rel, content));
    }

    Ok(sources)
}

/// Load an [`XsdConfig`] from a filesystem `xsd/` directory.
///
/// The directory may contain an optional `config.toml` and an optional
/// `types/` subtree with `*.xsd` files.
pub fn load_xsd_config_from_dir(xsd_dir: &Path) -> Result<XsdConfig, String> {
    let profile = {
        let config_path = xsd_dir.join("config.toml");
        if config_path.exists() {
            let toml_str = std::fs::read_to_string(&config_path)
                .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
            toml::from_str::<XsdProfile>(&toml_str)
                .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?
        } else {
            XsdProfile::default()
        }
    };

    let type_sources = {
        let types_dir = xsd_dir.join("types");
        if types_dir.is_dir() {
            collect_xsd_type_sources_from_dir(&types_dir)?
        } else {
            Vec::new()
        }
    };

    Ok(build_xsd_config_from_type_sources(profile, &type_sources))
}

fn walk_xsd_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            walk_xsd_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("xsd") {
            out.push(path);
        }
    }
}

fn path_to_forward_slash_string(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::path_to_forward_slash_string;

    #[test]
    fn path_to_forward_slash_string_uses_forward_slashes() {
        let path = std::path::PathBuf::from("AFFragments")
            .join("Nested")
            .join("Address.xsd");
        assert_eq!(
            path_to_forward_slash_string(&path),
            "AFFragments/Nested/Address.xsd"
        );
    }

    #[test]
    fn path_to_forward_slash_string_keeps_single_file_name() {
        let path = std::path::PathBuf::from("Address.xsd");
        assert_eq!(path_to_forward_slash_string(&path), "Address.xsd");
    }
}

// ============================================================================
// Complex type matching
// ============================================================================

/// Find the best set of 1..n pairwise-disjoint registered complex types
/// whose combined elements cover all `children`.
///
/// **Disjoint** means no two selected types share an element with the same
/// `(name, type_ref)` pair.
///
/// **Ranking** – among all valid covers, pick the one with the fewest total
/// type elements (tightest fit).  Every selected type must contribute at
/// least one child.
///
/// Returns an empty `Vec` if no cover exists or `children` is empty.
/// Collect the leaf element names of a registered complex type by recursively
/// expanding children whose `type_ref` is itself a registered complex type.
fn collect_leaf_names(
    rt: &RegisteredComplexType,
    registered_types: &HashMap<String, RegisteredComplexType>,
) -> HashSet<String> {
    let mut leaves = HashSet::new();
    let mut stack: Vec<&RegisteredComplexType> = vec![rt];
    let mut visited = HashSet::new();
    while let Some(current) = stack.pop() {
        if !visited.insert(&current.name as &str as *const str) {
            continue;
        }
        for elem in &current.elements {
            if let Some(child_type) = registered_types.get(&elem.type_ref) {
                stack.push(child_type);
            } else {
                leaves.insert(elem.name.clone());
            }
        }
    }
    leaves
}

pub fn find_matching_types<'a>(
    children: &[(String, String)],
    registered_types: &'a HashMap<String, RegisteredComplexType>,
) -> Vec<&'a RegisteredComplexType> {
    if children.is_empty() {
        return Vec::new();
    }

    // Pre-filter: keep only types that cover at least one child.
    let candidates: Vec<&RegisteredComplexType> = registered_types
        .values()
        .filter(|rt| {
            children.iter().any(|(name, type_ref)| {
                rt.elements
                    .iter()
                    .any(|e| e.name == *name && e.type_ref == *type_ref)
            })
        })
        .collect();

    if candidates.is_empty() {
        return Vec::new();
    }

    // Pre-compute leaf element names for each candidate (recursively expanded).
    let candidate_leaves: Vec<HashSet<String>> = candidates
        .iter()
        .map(|rt| collect_leaf_names(rt, registered_types))
        .collect();

    // For each candidate, compute a bitmask of which children it covers.
    let child_masks: Vec<u64> = candidates
        .iter()
        .map(|rt| {
            let mut mask = 0u64;
            for (i, (name, type_ref)) in children.iter().enumerate() {
                if rt
                    .elements
                    .iter()
                    .any(|e| e.name == *name && e.type_ref == *type_ref)
                {
                    mask |= 1u64 << i;
                }
            }
            mask
        })
        .collect();

    let full_mask = (1u64 << children.len()) - 1;

    // Best solution found so far: (total_elements, indices).
    let mut best: Option<(usize, Vec<usize>)> = None;

    // Recursive search with pruning.
    fn search(
        candidates: &[&RegisteredComplexType],
        candidate_leaves: &[HashSet<String>],
        child_masks: &[u64],
        full_mask: u64,
        start: usize,
        covered: u64,
        selected: &mut Vec<usize>,
        total_elems: usize,
        best: &mut Option<(usize, Vec<usize>)>,
    ) {
        if covered == full_mask {
            // Valid cover — check if it's better than the best.
            if best
                .as_ref()
                .is_none_or(|(best_total, _)| total_elems < *best_total)
            {
                *best = Some((total_elems, selected.clone()));
            }
            return;
        }

        // Prune: if current total already >= best, stop.
        if let Some((best_total, _)) = best {
            if total_elems >= *best_total {
                return;
            }
        }

        for i in start..candidates.len() {
            let mask_i = child_masks[i];

            // Must contribute at least one new child.
            if mask_i & !covered == 0 {
                continue;
            }

            // Must be leaf-disjoint with all already-selected types:
            // no shared leaf element names after recursive expansion.
            let disjoint = selected
                .iter()
                .all(|&j| candidate_leaves[i].is_disjoint(&candidate_leaves[j]));
            if !disjoint {
                continue;
            }

            selected.push(i);
            search(
                candidates,
                candidate_leaves,
                child_masks,
                full_mask,
                i + 1,
                covered | mask_i,
                selected,
                total_elems + candidates[i].elements.len(),
                best,
            );
            selected.pop();
        }
    }

    let mut selected = Vec::new();
    search(
        &candidates,
        &candidate_leaves,
        &child_masks,
        full_mask,
        0,
        0,
        &mut selected,
        0,
        &mut best,
    );

    match best {
        Some((_, indices)) => indices.iter().map(|&i| candidates[i]).collect(),
        None => Vec::new(),
    }
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
/// Finds the best match by picking the longest matching synonym
/// (most specific). Matching is case-insensitive substring.
pub fn resolve_element(label: &str, profile: &XsdProfile) -> Option<ResolvedElement> {
    let label_lower = label.to_lowercase();
    let mut best: Option<(usize, ResolvedElement)> = None;
    for (name, mapping) in &profile.elements {
        for synonym in &mapping.synonyms {
            let syn_lower = synonym.to_lowercase();
            if label_lower.contains(&syn_lower) {
                let len = syn_lower.len();
                if best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                    best = Some((
                        len,
                        ResolvedElement {
                            name: name.clone(),
                            type_ref: mapping.type_ref.clone(),
                        },
                    ));
                }
            }
        }
    }
    best.map(|(_, resolved)| resolved)
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
