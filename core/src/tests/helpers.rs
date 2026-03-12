#![allow(dead_code)] // Helpers may not all be used in current tests

use crate::structured::{
    ConditionalNode, FieldId, FieldNode, FieldType, HeadingNode, InlineNode, ListNode,
    StructuredNode,
};
use crate::xfa::script_executor::ScriptExecutor;
use crate::{Flattened, XfaNode, extract_xfa_from_pdf};

/// Build a path to a file in the `input/` test data directory.
///
/// Uses `CARGO_MANIFEST_DIR` so tests work regardless of the current working
/// directory (e.g. running from workspace root vs. package directory).
pub fn input_path(filename: &str) -> String {
    format!("{}/input/{}", env!("CARGO_MANIFEST_DIR"), filename)
}

/// Build a path to a directory or file inside the `profiles/` directory.
///
/// Profiles are at the workspace root, so we go up one level from CARGO_MANIFEST_DIR.
pub fn profiles_path(subpath: &str) -> String {
    format!("{}/../profiles/{}", env!("CARGO_MANIFEST_DIR"), subpath)
}

/// Recursively walk a tree of `StructuredNode`s, calling `callback` on every
/// node encountered (depth-first, pre-order).
///
/// The walker handles all node variants that can contain children:
/// `Group`, `Conditional`, `Repeatable`, `GridLayout`, and `Table`.
///
/// # Examples
///
/// Collecting all `FieldNode`s:
/// ```ignore
/// let mut fields = Vec::new();
/// walk_structured_nodes(&nodes, &mut |node| {
///     if let StructuredNode::Field(f) = node {
///         fields.push(f.clone());
///     }
/// });
/// ```
pub fn walk_structured_nodes(nodes: &[StructuredNode], callback: &mut impl FnMut(&StructuredNode)) {
    for node in nodes {
        callback(node);
        match node {
            StructuredNode::Group(group) => {
                walk_structured_nodes(&group.children, callback);
            }
            StructuredNode::Conditional(cond) => {
                walk_structured_nodes(std::slice::from_ref(cond.content.as_ref()), callback);
            }
            StructuredNode::Repeatable(rep) => {
                walk_structured_nodes(std::slice::from_ref(rep.item.as_ref()), callback);
            }
            StructuredNode::GridLayout(grid) => {
                for element in &grid.elements {
                    walk_structured_nodes(std::slice::from_ref(&element.node), callback);
                }
            }
            StructuredNode::Table(table) => {
                if let Some(header) = &table.header {
                    walk_structured_nodes(&header.cells, callback);
                }
                for row in &table.rows {
                    walk_structured_nodes(&row.cells, callback);
                }
            }
            _ => {}
        }
    }
}

// ============================================================================
// Typed collector helpers built on top of `walk_structured_nodes`
// ============================================================================

/// Collect all `FieldNode`s from the tree.
pub fn collect_fields(nodes: &[StructuredNode]) -> Vec<FieldNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            out.push(f.clone());
        }
    });
    out
}

/// Collect all `FieldNode`s whose `input_type` is `Radio`.
pub fn collect_radio_fields(nodes: &[StructuredNode]) -> Vec<FieldNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            if matches!(f.input_type, FieldType::Radio { .. }) {
                out.push(f.clone());
            }
        }
    });
    out
}

/// Collect field labels (as plain text) from all `FieldNode`s in the tree.
pub fn collect_field_labels(nodes: &[StructuredNode]) -> Vec<String> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            if let Some(label) = &f.label {
                out.push(label.as_plain_text());
            }
        }
    });
    out
}

/// Collect field labels with trimming — skips fields with empty labels after trim.
pub fn collect_field_labels_trimmed(nodes: &[StructuredNode]) -> Vec<String> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            if let Some(label) = &f.label {
                let text = label.as_plain_text();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
        }
    });
    out
}

/// Collect field names (SOM path strings) from all `FieldNode`s in the tree.
pub fn collect_field_names(nodes: &[StructuredNode]) -> Vec<String> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            out.push(f.som_path_str().to_string());
        }
    });
    out
}

/// Collect all `HeadingNode`s as `(level, text)` pairs.
pub fn collect_headings(nodes: &[StructuredNode]) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Heading(h) = node {
            out.push((h.level.as_u8(), h.content.as_plain_text()));
        }
    });
    out
}

/// Collect all `ConditionalNode`s from the tree.
pub fn collect_conditionals(nodes: &[StructuredNode]) -> Vec<ConditionalNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Conditional(c) = node {
            out.push(c.clone());
        }
    });
    out
}

/// Count the number of `Conditional` nodes in the tree.
pub fn count_conditionals(nodes: &[StructuredNode]) -> usize {
    let mut count = 0;
    walk_structured_nodes(nodes, &mut |node| {
        if matches!(node, StructuredNode::Conditional(_)) {
            count += 1;
        }
    });
    count
}

/// Collect all `ListNode`s from the tree.
pub fn collect_lists(nodes: &[StructuredNode]) -> Vec<ListNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::List(l) = node {
            out.push(l.clone());
        }
    });
    out
}

/// Collect all `InlineNode`s from `Paragraph` nodes in the tree.
pub fn collect_inline_nodes(nodes: &[StructuredNode]) -> Vec<InlineNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Paragraph(p) = node {
            out.extend(p.content.0.clone());
        }
    });
    out
}

/// Find the `FieldId` of the first field whose SOM path ends with `suffix`.
pub fn find_field_id_by_suffix(nodes: &[StructuredNode], suffix: &str) -> Option<FieldId> {
    let fields = collect_fields(nodes);
    fields
        .iter()
        .find(|f| f.som_path_str().ends_with(suffix))
        .map(|f| f.name.clone())
}

/// Find the first field whose SOM path contains `name`.
pub fn find_field_by_name(nodes: &[StructuredNode], name: &str) -> Option<FieldNode> {
    collect_fields(nodes)
        .into_iter()
        .find(|f| f.som_path_str().contains(name))
}

// ============================================================================
// XFA loading and parsing helpers
// ============================================================================

/// Flatten XFA with script execution.
pub fn flatten_with_scripts(nodes: &mut [XfaNode]) -> Result<Flattened, String> {
    let script_result = ScriptExecutor::execute(nodes);
    ScriptExecutor::apply_presence_changes(nodes, &script_result.presence_changes);
    Flattened::merge_form_items_into_template(nodes);
    Flattened::merge_form_presence_into_template(nodes, &script_result.presence_changes);
    Flattened::from_xfa(nodes, &script_result.computed_values)
}

/// Extract XFA from a PDF file and parse it into `XfaNode`s.
/// Panics if the PDF cannot be read, contains no XFA, or parsing fails.
pub fn parse_xfa_from_pdf(path: impl AsRef<std::path::Path>) -> Vec<XfaNode> {
    let xfa_data = extract_xfa_from_pdf(path.as_ref()).expect("Failed to read PDF");
    let xfa_buffer = xfa_data.expect("PDF should contain XFA data");
    XfaNode::parse(&xfa_buffer).expect("Failed to parse XFA structure")
}

/// Extract XFA from a PDF file, parse it, and flatten with script execution.
/// Panics if any step fails.
pub fn flatten_from_pdf(path: impl AsRef<std::path::Path>) -> Flattened {
    let mut nodes = parse_xfa_from_pdf(path);
    flatten_with_scripts(&mut nodes).expect("Failed to flatten XFA with scripts")
}

/// Load the UBS profile (config.toml + XML templates) from `profiles/ubs/aem/`.
///
/// Returns `(AemProfile, templates)` ready for `AemConfig::from_profile`.
pub fn load_ubs_profile() -> (
    crate::aem::AemProfile,
    std::collections::HashMap<String, String>,
) {
    let dir_path = profiles_path("ubs/aem");
    let dir = std::path::Path::new(&dir_path);
    let toml_str = std::fs::read_to_string(dir.join("config.toml"))
        .expect("Failed to read profiles/ubs/aem/config.toml");
    let profile: crate::aem::AemProfile =
        toml::from_str(&toml_str).expect("Failed to parse UBS config.toml");

    let mut templates = std::collections::HashMap::new();
    for entry in std::fs::read_dir(dir).expect("Failed to read profiles/ubs/aem/") {
        let entry = entry.expect("Failed to read dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("xml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let content = std::fs::read_to_string(&path).expect("Failed to read template");
                templates.insert(stem.to_string(), content);
            }
        }
    }
    (profile, templates)
}

/// Assert that the given string is well-formed XML.
///
/// Parses the string with `quick_xml::Reader` and panics on any syntax error,
/// printing the error and a snippet of the surrounding XML.
pub fn assert_valid_xml(xml: &str) {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = true;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Err(e) => {
                let pos = reader.error_position() as usize;
                let start = pos.saturating_sub(200);
                let end = (pos + 200).min(xml.len());
                let snippet = &xml[start..end];
                panic!("XML syntax error at byte {pos}: {e}\n\nContext:\n{snippet}");
            }
            _ => {}
        }
    }

    // Also check for unescaped `&` in attribute values, which quick-xml
    // may silently accept. An `&` must be followed by a valid entity
    // reference (`amp;`, `lt;`, `gt;`, `quot;`, `apos;`) or a numeric
    // character reference (`#...;`).
    check_no_unescaped_ampersands(xml);

    // Check for unescaped `<` inside attribute values.
    check_no_unescaped_angle_brackets_in_attributes(xml);
}

/// Verify that every `&` in the XML is part of a valid entity or character
/// reference. Panics with context if an unescaped `&` is found.
fn check_no_unescaped_ampersands(xml: &str) {
    let bytes = xml.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'&' {
            continue;
        }
        // Look ahead for `;`
        let rest = &xml[i + 1..];
        if let Some(semi) = rest.find(';') {
            let entity = &rest[..semi];
            // Permit named entities and numeric references
            if matches!(entity, "amp" | "lt" | "gt" | "quot" | "apos") || entity.starts_with('#') {
                continue;
            }
        }
        // Unescaped `&`
        let start = i.saturating_sub(100);
        let end = (i + 100).min(xml.len());
        let snippet = &xml[start..end];
        panic!("Unescaped '&' at byte {i} in XML output.\n\nContext:\n{snippet}");
    }
}

/// Verify that no `<` or `>` appear inside XML attribute values.
/// Panics with context if a raw angle bracket is found in an attribute.
fn check_no_unescaped_angle_brackets_in_attributes(xml: &str) {
    // Simple state machine: track whether we are inside an opening tag
    // (between `<tagname` and `>`) and inside a quoted attribute value.
    let bytes = xml.as_bytes();
    let mut in_tag = false;
    let mut quote_char: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote_char {
            // Inside a quoted attribute value
            if b == q {
                quote_char = None;
            } else if b == b'<' || b == b'>' {
                let start = i.saturating_sub(100);
                let end = (i + 100).min(xml.len());
                let snippet = &xml[start..end];
                panic!(
                    "Unescaped '{}' inside XML attribute value at byte {i}.\n\nContext:\n{snippet}",
                    b as char
                );
            }
            i += 1;
            continue;
        }
        if in_tag {
            if b == b'"' || b == b'\'' {
                quote_char = Some(b);
            } else if b == b'>' || (b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>') {
                in_tag = false;
            }
        } else if b == b'<' {
            // Detect start of an opening/self-closing tag (skip comments, CDATA, PI)
            if i + 1 < bytes.len() && bytes[i + 1] != b'!' && bytes[i + 1] != b'?' {
                in_tag = true;
            }
        }
        i += 1;
    }
}
