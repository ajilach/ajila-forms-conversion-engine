//! Fragment scanner and parser for AEM fragment `.content.xml` files.
//!
//! Scans a `fragments/` directory tree for `.content.xml` files,
//! extracts `fragmentModelRoot` and `bindRef` attributes,
//! and produces a list of [`ParsedFragment`] structs used for matching
//! against XSD types during AEM conversion.

use std::path::Path;

/// A parsed AEM fragment with its XSD type binding information.
#[derive(Debug, Clone)]
pub struct ParsedFragment {
    /// Directory name of the fragment (e.g. `"affrg_Address1"`).
    pub dir_name: String,

    /// JCR path used as `fragRef` attribute in the generated XML
    /// (e.g. `"/content/forms/af/afforms_ubs_fragmentlib/affrg_Address1"`).
    pub frag_ref: String,

    /// The AEM `name` attribute for the fragment node
    /// (e.g. `"PN_affrg_Address1"`).
    pub name: String,

    /// XSD complex type name extracted from `fragmentModelRoot`
    /// (e.g. `"AddressType"` from `/AddressType`).
    pub xsd_type_name: String,

    /// Element names extracted from `bindRef` attributes within the fragment
    /// (e.g. `["Street", "Number", "City"]`).
    pub bound_elements: Vec<String>,
}

/// Recursively scan `fragments_dir` for `.content.xml` files and parse each
/// into a [`ParsedFragment`].
///
/// `fragment_ref_prefix` is the JCR path prefix for building `fragRef` values
/// (e.g. `"/content/forms/af/"`).
pub fn scan_fragments(fragments_dir: &Path, fragment_ref_prefix: &str) -> Vec<ParsedFragment> {
    let mut fragments = Vec::new();
    walk_fragments(
        fragments_dir,
        fragments_dir,
        fragment_ref_prefix,
        &mut fragments,
    );
    fragments
}

/// Recursively walk directories looking for `.content.xml` files.
fn walk_fragments(
    base_dir: &Path,
    current_dir: &Path,
    fragment_ref_prefix: &str,
    fragments: &mut Vec<ParsedFragment>,
) {
    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut sorted_entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted_entries.sort_by_key(|e| e.file_name());

    for entry in sorted_entries {
        let path = entry.path();
        if path.is_dir() {
            // Check if this directory contains a .content.xml
            let content_xml = path.join(".content.xml");
            if content_xml.is_file() {
                if let Some(fragment) =
                    parse_fragment_xml(&content_xml, &path, base_dir, fragment_ref_prefix)
                {
                    fragments.push(fragment);
                }
            }
            // Recurse into subdirectory
            walk_fragments(base_dir, &path, fragment_ref_prefix, fragments);
        }
    }
}

/// Parse a single fragment `.content.xml` file and extract fragment metadata.
fn parse_fragment_xml(
    xml_path: &Path,
    fragment_dir: &Path,
    base_dir: &Path,
    fragment_ref_prefix: &str,
) -> Option<ParsedFragment> {
    let content = std::fs::read_to_string(xml_path).ok()?;

    // Build relative directory path and delegate parsing.
    let rel_path = fragment_dir.strip_prefix(base_dir).ok()?;
    let rel_str = rel_path.to_string_lossy().replace('\\', "/");
    parse_fragment_content(&rel_str, fragment_ref_prefix, &content)
}

/// Parse fragment metadata from `.content.xml` text and a known relative
/// fragment directory path.
///
/// `relative_dir_path` is path-like (e.g. `"afforms_ubs_fragmentlib/affrg_Address1"`).
pub fn parse_fragment_content(
    relative_dir_path: &str,
    fragment_ref_prefix: &str,
    content: &str,
) -> Option<ParsedFragment> {
    let relative_dir_path = relative_dir_path.trim_matches('/');
    if relative_dir_path.is_empty() {
        return None;
    }

    // Extract fragmentModelRoot (e.g. fragmentModelRoot="/AddressType")
    let xsd_type_name = extract_attr_value(content, "fragmentModelRoot")?;
    let xsd_type_name = xsd_type_name.trim_start_matches('/').to_string();
    if xsd_type_name.is_empty() {
        return None;
    }

    // Build fragRef from the relative directory path.
    let prefix = fragment_ref_prefix.trim_end_matches('/');
    let frag_ref = format!("{}/{}", prefix, relative_dir_path);

    // Directory name for identity.
    let dir_name = relative_dir_path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();

    let name = format!("PN_affrg_{}", dir_name.trim_start_matches("affrg_"));

    // Extract all bindRef values to collect bound element names
    let bound_elements = extract_bind_ref_elements(content, &xsd_type_name);

    Some(ParsedFragment {
        dir_name,
        frag_ref,
        name,
        xsd_type_name,
        bound_elements,
    })
}

/// Extract the value of a named XML attribute from raw XML text.
///
/// Looks for `attr_name="value"` patterns. Returns the first match.
fn extract_attr_value(xml: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr_name);
    let start = xml.find(&pattern)?;
    let value_start = start + pattern.len();
    let rest = &xml[value_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extract element names from `bindRef` attributes in the XML content.
///
/// `bindRef` values look like `/TypeName/ElementName` — we extract the
/// leaf element name (after the last `/`), but only if the path starts
/// with the expected type root.
fn extract_bind_ref_elements(xml: &str, type_name: &str) -> Vec<String> {
    let mut elements = Vec::new();
    let pattern = "bindRef=\"";
    let mut search_from = 0;

    while let Some(pos) = xml[search_from..].find(pattern) {
        let abs_pos = search_from + pos;
        let value_start = abs_pos + pattern.len();
        if let Some(end) = xml[value_start..].find('"') {
            let bind_ref = &xml[value_start..value_start + end];
            // Expected format: /TypeName/ElementName
            let expected_prefix = format!("/{}/", type_name);
            if let Some(rest) = bind_ref.strip_prefix(&expected_prefix) {
                // Take only the immediate child (no nested paths)
                if !rest.contains('/') && !rest.is_empty() {
                    elements.push(rest.to_string());
                }
            }
            search_from = value_start + end + 1;
        } else {
            break;
        }
    }

    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_attr_value() {
        let xml = r#"fragmentModelRoot="/BankingRelationshipType" guideCss="guideContainer""#;
        assert_eq!(
            extract_attr_value(xml, "fragmentModelRoot"),
            Some("/BankingRelationshipType".into())
        );
        assert_eq!(
            extract_attr_value(xml, "guideCss"),
            Some("guideContainer".into())
        );
        assert_eq!(extract_attr_value(xml, "missing"), None);
    }

    #[test]
    fn test_extract_bind_ref_elements() {
        let xml = r#"
            bindRef="/AddressType/Street"
            bindRef="/AddressType/Number"
            bindRef="/AddressType/City"
            bindRef="/OtherType/Foo"
        "#;
        let elements = extract_bind_ref_elements(xml, "AddressType");
        assert_eq!(elements, vec!["Street", "Number", "City"]);
    }

    #[test]
    fn test_extract_bind_ref_elements_nested_ignored() {
        let xml = r#"bindRef="/AddressType/Nested/Deep""#;
        let elements = extract_bind_ref_elements(xml, "AddressType");
        assert!(elements.is_empty());
    }

    #[test]
    fn test_parse_fragment_name_generation() {
        assert_eq!(
            format!("PN_affrg_{}", "affrg_Address1".trim_start_matches("affrg_")),
            "PN_affrg_Address1"
        );
        assert_eq!(
            format!(
                "PN_affrg_{}",
                "affrg_BankingRelationship1".trim_start_matches("affrg_")
            ),
            "PN_affrg_BankingRelationship1"
        );
    }

    #[test]
    fn test_scan_fragments_with_real_directory() {
        // Test with the actual fragments directory if it exists
        let fragments_dir = Path::new("../profiles/ubs/aem/fragments");
        if !fragments_dir.is_dir() {
            return; // Skip if not running from expected working directory
        }
        let fragments = scan_fragments(fragments_dir, "/content/forms/af/");
        // We should find at least the Address, BankingRelationship, Individual fragments
        assert!(
            !fragments.is_empty(),
            "Expected at least one fragment to be parsed"
        );

        // Check that the Address fragment was parsed correctly
        let address = fragments.iter().find(|f| f.xsd_type_name == "AddressType");
        assert!(
            address.is_some(),
            "Expected AddressType fragment; found types: {:?}",
            fragments
                .iter()
                .map(|f| &f.xsd_type_name)
                .collect::<Vec<_>>()
        );
        let address = address.unwrap();
        assert!(
            address.bound_elements.contains(&"Street".to_string()),
            "Expected 'Street' in bound elements: {:?}",
            address.bound_elements
        );
        assert!(
            address.bound_elements.contains(&"City".to_string()),
            "Expected 'City' in bound elements: {:?}",
            address.bound_elements
        );
        assert!(
            address.frag_ref.starts_with("/content/forms/af/"),
            "fragRef should start with prefix: {}",
            address.frag_ref
        );
    }
}
