#![allow(dead_code)] // Helpers may not all be used in current tests

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::sync::Once;

use crate::structured::{
    ConditionalNode, FieldId, FieldNode, FieldType, InlineNode, ListNode, StructuredNode,
};

/// Ensure UBS profile fonts are loaded into the global font manager.
///
/// This is safe to call from many tests — the loading happens exactly once.
pub fn ensure_ubs_fonts_loaded() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        crate::profiles::load_profile_fonts("ubs")
            .expect("Failed to load UBS profile fonts for tests");
    });
}

/// Build a path to a file in the `input/` test data directory.
///
/// Uses `CARGO_MANIFEST_DIR` so tests work regardless of the current working
/// directory (e.g. running from workspace root vs. package directory).
///
/// Also ensures UBS profile fonts are loaded (lazily, once).
pub fn input_path(filename: &str) -> String {
    ensure_ubs_fonts_loaded();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    format!("{}/input/{}", manifest_dir, filename)
}

/// Build a path to a directory or file inside the `profiles/` directory.
///
/// Profiles are at the workspace root, so we go up one level from CARGO_MANIFEST_DIR.
pub fn profiles_path(subpath: &str) -> String {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    format!("{}/../profiles/{}", manifest_dir, subpath)
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

/// Collect all `FieldNode`s whose `input_type` is `Textarea`.
pub fn collect_textarea_fields(nodes: &[StructuredNode]) -> Vec<FieldNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            if matches!(f.input_type, FieldType::Textarea { .. }) {
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

/// Collect all `FootnoteNode`s as `(marker, text)` pairs from the tree.
pub fn collect_footnotes(nodes: &[StructuredNode]) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Footnote(f) = node {
            out.push((f.marker.clone(), f.content.as_plain_text()));
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

/// Return `true` if any `Paragraph` or `Heading` reachable from `nodes`
/// **without** passing through a `Conditional` node contains `fragment` in its
/// plain text.
///
/// This is used to assert that text blocks which are common to all form states
/// are properly "factored out" of conditionals in the merged output.
pub fn has_text_outside_conditional(nodes: &[StructuredNode], fragment: &str) -> bool {
    for node in nodes {
        match node {
            StructuredNode::Paragraph(p) => {
                if p.content.as_plain_text().contains(fragment) {
                    return true;
                }
            }
            StructuredNode::Heading(h) => {
                if h.content.as_plain_text().contains(fragment) {
                    return true;
                }
            }
            StructuredNode::Group(g) => {
                if has_text_outside_conditional(&g.children, fragment) {
                    return true;
                }
            }
            StructuredNode::Repeatable(r) => {
                if has_text_outside_conditional(std::slice::from_ref(r.item.as_ref()), fragment) {
                    return true;
                }
            }
            StructuredNode::GridLayout(grid) => {
                for element in &grid.elements {
                    if has_text_outside_conditional(std::slice::from_ref(&element.node), fragment) {
                        return true;
                    }
                }
            }
            StructuredNode::Table(table) => {
                if let Some(header) = &table.header {
                    if has_text_outside_conditional(&header.cells, fragment) {
                        return true;
                    }
                }
                for row in &table.rows {
                    if has_text_outside_conditional(&row.cells, fragment) {
                        return true;
                    }
                }
            }
            // Intentionally do NOT recurse into Conditional nodes
            StructuredNode::Conditional(_) => {}
            _ => {}
        }
    }
    false
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

/// Collect all `TableNode`s from the tree.
pub fn collect_tables(nodes: &[StructuredNode]) -> Vec<crate::structured::TableNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Table(t) = node {
            out.push(t.clone());
        }
    });
    out
}

/// Collect all `InlineNode`s from `Paragraph` nodes in the tree.
pub fn collect_inline_nodes(nodes: &[StructuredNode]) -> Vec<InlineNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Paragraph(p) = node {
            if let Some(inline_text) = p.content.0.values().next() {
                out.extend(inline_text.0.clone());
            }
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

/// Load the UBS profile (config.toml + XML templates) from `profiles/ubs/aem/`.
///
/// Returns `(AemProfile, templates, custom_templates)` ready for `AemConfig::from_profile`.
pub fn load_ubs_profile() -> (
    crate::aem::AemProfile,
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, String>,
) {
    let dir_path = profiles_path("ubs/aem");
    let dir = std::path::Path::new(&dir_path);
    let toml_str = std::fs::read_to_string(dir.join("config.toml"))
        .expect("Failed to read profiles/ubs/aem/config.toml");
    // Disable custom element rules by default for tests so that existing tests
    // asserting on Fragment/Panel structure are not affected by the custom
    // element replacement pass.
    let mut profile: crate::aem::AemProfile =
        toml::from_str(&toml_str).expect("Failed to parse UBS config.toml");
    profile.custom_elements.clear();

    // Load translations/ directory (per-language TOML files)
    let translations_dir = dir.join("translations");
    if translations_dir.is_dir() {
        for entry in std::fs::read_dir(&translations_dir).expect("Failed to read translations/ dir")
        {
            let entry = entry.expect("Failed to read translations dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                if let Some(lang) = path.file_stem().and_then(|s| s.to_str()) {
                    let content =
                        std::fs::read_to_string(&path).expect("Failed to read translation TOML");
                    crate::profiles::parse_translation_toml(
                        &content,
                        lang,
                        &mut profile.default_translations,
                    )
                    .unwrap_or_else(|e| panic!("Failed to parse translations/{lang}.toml: {e}"));
                }
            }
        }
    }

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

    let mut custom_templates = std::collections::HashMap::new();
    let custom_dir = dir.join("custom");
    if custom_dir.is_dir() {
        for entry in std::fs::read_dir(&custom_dir).expect("Failed to read custom/ dir") {
            let entry = entry.expect("Failed to read custom dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("xml") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let content =
                        std::fs::read_to_string(&path).expect("Failed to read custom template");
                    custom_templates.insert(stem.to_string(), content);
                }
            }
        }
    }

    (profile, templates, custom_templates)
}

/// Like `load_ubs_profile`, but keeps custom_elements enabled so that tests
/// can verify custom element placement in the AEM tree.
pub fn load_ubs_profile_with_custom_elements() -> (
    crate::aem::AemProfile,
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, String>,
) {
    let (mut profile, templates, custom_templates) = load_ubs_profile();
    // Reload custom_elements from the config (load_ubs_profile clears them).
    let dir_path = profiles_path("ubs/aem");
    let dir = std::path::Path::new(&dir_path);
    let toml_str = std::fs::read_to_string(dir.join("config.toml"))
        .expect("Failed to read profiles/ubs/aem/config.toml");
    let full_profile: crate::aem::AemProfile =
        toml::from_str(&toml_str).expect("Failed to parse UBS config.toml");
    profile.custom_elements = full_profile.custom_elements;
    (profile, templates, custom_templates)
}

/// Build AEM node tree with custom elements enabled.
pub(super) fn build_aem_test_output_with_custom_elements(
    pdfs: &[(&str, &str)],
) -> crate::aem::AemNode {
    use crate::aem::{AemConfig, convert_to_aem};

    let envelopes: Vec<_> = pdfs
        .iter()
        .map(|(file, lang)| {
            crate::run_exhaustive_to_envelope(input_path(file), lang)
                .unwrap_or_else(|e| panic!("Failed to process {file}: {e}"))
        })
        .collect();

    let (ctx, content) = if envelopes.len() == 1 {
        let env = envelopes.into_iter().next().unwrap();
        (env.context, env.content)
    } else {
        let merged = crate::structured::merge_translations(envelopes, None)
            .expect("Failed to merge translations");
        (merged.context, merged.content)
    };

    let (profile, templates, custom_templates) = load_ubs_profile_with_custom_elements();
    let config = AemConfig::from_profile(&profile, templates, custom_templates, &ctx)
        .expect("Failed to create AemConfig");
    let config = crate::resolve_aem_languages(&content, &config);
    convert_to_aem(&content, &config)
}

/// Generate AEM XML from one or more PDF files and validate that it is
/// well-formed XML. Each entry is `(filename, language_code)`.
pub fn assert_aem_xml_valid_for(pdfs: &[(&str, &str)]) {
    let (_, root, config) = build_aem_test_output(pdfs);
    let xml = crate::aem::generate_aem_xml(&root, &config);

    assert_valid_aem_form_xml(&xml);
}

/// Generate an AEM package entirely in memory from one or more PDF files and
/// validate both the form and DAM `.content.xml` entries extracted from the ZIP.
pub fn assert_aem_package_valid_for(pdfs: &[(&str, &str)]) {
    let (content, root, config) = build_aem_test_output(pdfs);
    let zip_bytes = crate::aem::generate_aem_package(&root, &config, &content);
    let reader = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).expect("valid in-memory zip");

    let form_entry = format!(
        "jcr_root/content/forms/af/{}/{}/.content.xml",
        config.form_path,
        config.form_dir()
    );
    let dam_entry = format!(
        "jcr_root/content/dam/formsanddocuments/{}/{}/.content.xml",
        config.form_path,
        config.form_dir()
    );

    let mut form_xml = String::new();
    archive
        .by_name(&form_entry)
        .unwrap_or_else(|_| panic!("package missing form entry: {form_entry}"))
        .read_to_string(&mut form_xml)
        .expect("read form xml from zip");

    let mut dam_xml = String::new();
    archive
        .by_name(&dam_entry)
        .unwrap_or_else(|_| panic!("package missing DAM entry: {dam_entry}"))
        .read_to_string(&mut dam_xml)
        .expect("read dam xml from zip");

    assert_valid_aem_form_xml(&form_xml);
    assert_valid_aem_dam_xml(&dam_xml);
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

/// Assert that a generated AEM form `.content.xml` is both well-formed and
/// structurally valid for the JCR/CQ/FD/Sling conventions used by this project.
pub fn assert_valid_aem_form_xml(xml: &str) {
    assert_valid_xml(xml);
    let doc = parse_xml_document(xml);
    let mut violations = Vec::new();

    validate_namespaces(&doc, &mut violations);
    validate_form_structure(&doc, &mut violations);
    validate_jcr_primary_types(&doc, XmlKind::Form, &mut violations);
    validate_sling_resource_types(&doc, &mut violations);
    validate_fd_contract(&doc, &mut violations);
    validate_typed_values(&doc, &mut violations);
    validate_guide_node_classes(&doc, &mut violations);
    validate_repeatables(&doc, &mut violations);

    if !violations.is_empty() {
        panic!(
            "AEM form XML failed {} structural validation checks:\n- {}",
            violations.len(),
            violations.join("\n- ")
        );
    }
}

/// Assert that a generated AEM DAM `.content.xml` is both well-formed and
/// structurally valid for the JCR/DAM/Sling conventions used by this project.
pub fn assert_valid_aem_dam_xml(xml: &str) {
    assert_valid_xml(xml);
    let doc = parse_xml_document(xml);
    let mut violations = Vec::new();

    validate_namespaces(&doc, &mut violations);
    validate_dam_structure(&doc, &mut violations);
    validate_jcr_primary_types(&doc, XmlKind::Dam, &mut violations);
    validate_sling_resource_types(&doc, &mut violations);
    validate_typed_values(&doc, &mut violations);

    if !violations.is_empty() {
        panic!(
            "AEM DAM XML failed {} structural validation checks:\n- {}",
            violations.len(),
            violations.join("\n- ")
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XmlKind {
    Form,
    Dam,
}

#[derive(Debug)]
struct ParsedXmlDocument {
    nodes: Vec<ParsedXmlNode>,
}

#[derive(Debug)]
struct ParsedXmlNode {
    tag: String,
    path: String,
    attributes: BTreeMap<String, String>,
    parent: Option<usize>,
    children: Vec<usize>,
}

pub(super) fn build_aem_test_output(
    pdfs: &[(&str, &str)],
) -> (
    Vec<StructuredNode>,
    crate::aem::AemNode,
    crate::aem::AemConfig,
) {
    use crate::aem::{AemConfig, convert_to_aem};

    let envelopes: Vec<_> = pdfs
        .iter()
        .map(|(file, lang)| {
            crate::run_exhaustive_to_envelope(input_path(file), lang)
                .unwrap_or_else(|e| panic!("Failed to process {file}: {e}"))
        })
        .collect();

    let (ctx, content) = if envelopes.len() == 1 {
        let env = envelopes.into_iter().next().unwrap();
        (env.context, env.content)
    } else {
        let merged = crate::structured::merge_translations(envelopes, None)
            .expect("Failed to merge translations");
        (merged.context, merged.content)
    };

    let (profile, templates, custom_templates) = load_ubs_profile();
    let config = AemConfig::from_profile(&profile, templates, custom_templates, &ctx)
        .expect("Failed to create AemConfig");
    let config = crate::resolve_aem_languages(&content, &config);
    let root = convert_to_aem(&content, &config);

    (content, root, config)
}

fn parse_xml_document(xml: &str) -> ParsedXmlDocument {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = true;

    let mut nodes = Vec::<ParsedXmlNode>::new();
    let mut stack = Vec::<usize>::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let node_index = push_xml_node(&reader, &mut nodes, stack.last().copied(), event);
                stack.push(node_index);
            }
            Ok(Event::Empty(event)) => {
                push_xml_node(&reader, &mut nodes, stack.last().copied(), event);
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("Failed to parse XML for structural validation: {e}"),
            _ => {}
        }
    }

    ParsedXmlDocument { nodes }
}

fn push_xml_node(
    reader: &quick_xml::Reader<&[u8]>,
    nodes: &mut Vec<ParsedXmlNode>,
    parent: Option<usize>,
    event: quick_xml::events::BytesStart<'_>,
) -> usize {
    let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
    let mut attributes = BTreeMap::new();
    for attr in event.attributes().with_checks(false) {
        let attr = attr.unwrap_or_else(|e| panic!("Invalid XML attribute: {e}"));
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        let value = attr
            .decode_and_unescape_value(reader.decoder())
            .unwrap_or_else(|e| panic!("Failed to decode XML attribute value for {key}: {e}"))
            .into_owned();
        attributes.insert(key, value);
    }

    let path = match parent {
        Some(parent_index) => format!(
            "{}/{}",
            nodes[parent_index].path,
            node_descriptor(&tag, &attributes)
        ),
        None => node_descriptor(&tag, &attributes),
    };

    let index = nodes.len();
    nodes.push(ParsedXmlNode {
        tag,
        path,
        attributes,
        parent,
        children: Vec::new(),
    });

    if let Some(parent_index) = parent {
        nodes[parent_index].children.push(index);
    }

    index
}

fn node_descriptor(tag: &str, attributes: &BTreeMap<String, String>) -> String {
    if let Some(name) = attributes.get("name") {
        format!("{tag}[@name='{name}']")
    } else {
        tag.to_string()
    }
}

fn validate_namespaces(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    let Some(root) = doc.nodes.first() else {
        violations.push("XML document is empty".into());
        return;
    };

    let declared = root
        .attributes
        .iter()
        .filter_map(|(key, value)| key.strip_prefix("xmlns:").map(|prefix| (prefix, value)))
        .collect::<BTreeMap<_, _>>();

    let mut used_prefixes = BTreeSet::new();
    for node in &doc.nodes {
        if let Some(prefix) = xml_prefix(&node.tag) {
            used_prefixes.insert(prefix.to_string());
        }
        for key in node.attributes.keys() {
            if key.starts_with("xmlns:") {
                continue;
            }
            if let Some(prefix) = xml_prefix(key) {
                used_prefixes.insert(prefix.to_string());
            }
        }
    }

    for prefix in used_prefixes {
        if !declared.contains_key(prefix.as_str()) {
            violations.push(format!("root node is missing xmlns:{prefix} declaration"));
        }
    }

    for (prefix, uri) in canonical_namespaces() {
        if let Some(actual) = declared.get(prefix) {
            if *actual != uri {
                violations.push(format!("xmlns:{prefix} must be '{uri}', got '{actual}'"));
            }
        }
    }
}

fn validate_form_structure(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    let Some(root) = doc.nodes.first() else {
        return;
    };
    if root.tag != "jcr:root" {
        violations.push(format!(
            "form root element must be jcr:root, got {}",
            root.tag
        ));
        return;
    }

    let Some(content_index) = find_direct_child(doc, 0, "jcr:content") else {
        violations.push("jcr:root must have a direct jcr:content child".into());
        return;
    };
    let content = &doc.nodes[content_index];

    require_attr_value(content, "jcr:primaryType", "cq:PageContent", violations);
    require_non_empty_attr(content, "sling:resourceType", violations);
    require_non_empty_attr(content, "cq:template", violations);
    if let Some(template) = content.attributes.get("cq:template") {
        if !template.starts_with('/') {
            violations.push(format!(
                "{} has cq:template that is not an absolute JCR path: {}",
                content.path, template
            ));
        }
    }

    let Some(guide_index) = find_direct_child(doc, content_index, "guideContainer") else {
        violations.push(format!("{} must contain guideContainer", content.path));
        return;
    };
    let guide = &doc.nodes[guide_index];
    require_attr_value(guide, "jcr:primaryType", "nt:unstructured", violations);
    require_attr_value(guide, "guideNodeClass", "guideContainerNode", violations);
    require_non_empty_attr(guide, "sling:resourceType", violations);
    if let Some(resource_type) = guide.attributes.get("sling:resourceType") {
        if !resource_type.contains("guideContainer") {
            violations.push(format!(
                "{} must use a guideContainer sling:resourceType, got {}",
                guide.path, resource_type
            ));
        }
    }
    require_non_empty_attr(guide, "fd:version", violations);
    require_direct_child(doc, guide_index, "layout", violations);
    let Some(root_panel_index) = find_direct_child(doc, guide_index, "rootPanel") else {
        violations.push(format!("{} must contain rootPanel", guide.path));
        return;
    };

    let root_panel = &doc.nodes[root_panel_index];
    require_attr_value(root_panel, "jcr:primaryType", "nt:unstructured", violations);
    require_attr_value(root_panel, "guideNodeClass", "rootPanelNode", violations);
    require_non_empty_attr(root_panel, "sling:resourceType", violations);
    if let Some(resource_type) = root_panel.attributes.get("sling:resourceType") {
        if !resource_type.contains("panel") && !resource_type.contains("rootPanel") {
            violations.push(format!(
                "{} must use a panel/rootPanel sling:resourceType, got {}",
                root_panel.path, resource_type
            ));
        }
    }
    require_direct_child(doc, root_panel_index, "layout", violations);
    require_direct_child(doc, root_panel_index, "items", violations);
    require_direct_child(doc, root_panel_index, "toolbar", violations);

    validate_responsive_nodes(doc, violations);
}

fn validate_dam_structure(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    let Some(root) = doc.nodes.first() else {
        return;
    };
    if root.tag != "jcr:root" {
        violations.push(format!(
            "DAM root element must be jcr:root, got {}",
            root.tag
        ));
        return;
    }

    let Some(content_index) = find_direct_child(doc, 0, "jcr:content") else {
        violations.push("DAM jcr:root must have a direct jcr:content child".into());
        return;
    };
    let content = &doc.nodes[content_index];
    require_attr_value(content, "jcr:primaryType", "dam:AssetContent", violations);
    require_non_empty_attr(content, "sling:resourceType", violations);

    if find_direct_child(doc, content_index, "metadata").is_none() {
        violations.push(format!("{} must contain metadata child", content.path));
    }
}

fn validate_jcr_primary_types(
    doc: &ParsedXmlDocument,
    kind: XmlKind,
    violations: &mut Vec<String>,
) {
    let Some(root) = doc.nodes.first() else {
        return;
    };
    let expected_root_type = match kind {
        XmlKind::Form => "cq:Page",
        XmlKind::Dam => "dam:Asset",
    };
    require_attr_value(root, "jcr:primaryType", expected_root_type, violations);

    for node in &doc.nodes {
        match node.tag.as_str() {
            "layout" | "items" | "toolbar" | "guideContainer" | "rootPanel" | "cq:responsive"
            | "default" | "fd:rules" | "fd:scripts" | "metadata" => {
                require_attr_value(node, "jcr:primaryType", "nt:unstructured", violations);
            }
            _ => {}
        }

        if node.attributes.contains_key("guideNodeClass") {
            require_attr_value(node, "jcr:primaryType", "nt:unstructured", violations);
        }
    }
}

fn validate_sling_resource_types(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    for node in &doc.nodes {
        if node.attributes.contains_key("guideNodeClass") {
            require_non_empty_attr(node, "sling:resourceType", violations);
        }

        if matches!(node.tag.as_str(), "layout" | "items") && has_container_parent(doc, node) {
            require_non_empty_attr(node, "sling:resourceType", violations);
        }

        if let Some(resource_type) = node.attributes.get("sling:resourceType") {
            if resource_type.trim().is_empty() {
                violations.push(format!("{} has empty sling:resourceType", node.path));
            }
        }
    }
}

fn validate_fd_contract(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    for node in &doc.nodes {
        if node.tag == "fd:scripts" {
            require_attr_value(node, "jcr:primaryType", "nt:unstructured", violations);
            for key in [
                "fd:click",
                "fd:init",
                "fd:calculate",
                "fd:validate",
                "fd:valueCommit",
                "fd:navigationChange",
            ] {
                if let Some(value) = node.attributes.get(key) {
                    let trimmed = value.trim();
                    if trimmed.is_empty() {
                        violations
                            .push(format!("{} attribute {} must not be empty", node.path, key));
                    }
                    let looks_script_payload = (trimmed.starts_with('{') && trimmed.ends_with('}'))
                        || (trimmed.starts_with('[') && trimmed.ends_with(']'));
                    if !looks_script_payload || !trimmed.contains('{') || !trimmed.contains('}') {
                        violations.push(format!(
                            "{} attribute {} must look like an fd script payload, got {}",
                            node.path, key, value
                        ));
                    }
                }
            }
        }

        if node.tag == "fd:rules" {
            require_attr_value(node, "jcr:primaryType", "nt:unstructured", violations);
        }
    }
}

fn validate_typed_values(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    for node in &doc.nodes {
        for (key, value) in &node.attributes {
            if let Some(suffix) = value.strip_prefix("{Boolean}") {
                if suffix != "true" && suffix != "false" {
                    violations.push(format!(
                        "{} attribute {} has invalid boolean typed value {}",
                        node.path, key, value
                    ));
                }
            }
            if let Some(suffix) = value.strip_prefix("{Long}") {
                if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
                    violations.push(format!(
                        "{} attribute {} has invalid long typed value {}",
                        node.path, key, value
                    ));
                }
            }
            if let Some(suffix) = value.strip_prefix("{Date}") {
                if !looks_like_iso_datetime(suffix) {
                    violations.push(format!(
                        "{} attribute {} has invalid date typed value {}",
                        node.path, key, value
                    ));
                }
            }
            if value.starts_with('[') && !value.ends_with(']') {
                violations.push(format!(
                    "{} attribute {} has unbalanced array-like value {}",
                    node.path, key, value
                ));
            }
        }

        for key in ["visible", "enabled", "validateOnStepCompletion"] {
            if let Some(value) = node.attributes.get(key) {
                let is_valid = matches!(
                    value.as_str(),
                    "{Boolean}true" | "{Boolean}false" | "true" | "false"
                );
                if !is_valid {
                    violations.push(format!(
                        "{} attribute {} must be a boolean-like value, got {}",
                        node.path, key, value
                    ));
                }
            }
        }

        if let Some(value) = node.attributes.get("mandatory") {
            if value != "true" && value != "false" {
                violations.push(format!(
                    "{} attribute mandatory must be 'true' or 'false', got {}",
                    node.path, value
                ));
            }
        }
    }
}

fn validate_guide_node_classes(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    let valid_classes = [
        "guideContainerNode",
        "rootPanelNode",
        "guidePanel",
        "guideTextBox",
        "guideTextDraw",
        "guideCheckBox",
        "guideRadioButton",
        "guideDropDownList",
        "guideDatePicker",
        "guideButton",
        "guideNumericBox",
        "guideToolbar",
    ];

    for node in &doc.nodes {
        if let Some(class_name) = node.attributes.get("guideNodeClass") {
            if !valid_classes.contains(&class_name.as_str()) {
                violations.push(format!(
                    "{} has unknown guideNodeClass {}",
                    node.path, class_name
                ));
            }
        }
    }
}

fn validate_repeatables(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    for node in &doc.nodes {
        if node.attributes.contains_key("minOccur") || node.attributes.contains_key("maxOccur") {
            require_attr_value(node, "jcr:primaryType", "nt:unstructured", violations);
            require_non_empty_attr(node, "sling:resourceType", violations);

            let min = node
                .attributes
                .get("minOccur")
                .and_then(|value| value.parse::<u32>().ok());
            let max = node
                .attributes
                .get("maxOccur")
                .and_then(|value| value.parse::<u32>().ok());
            if min.is_none() {
                violations.push(format!("{} has invalid minOccur", node.path));
            }
            if max.is_none() {
                violations.push(format!("{} has invalid maxOccur", node.path));
            }
            if let (Some(min), Some(max)) = (min, max) {
                if min > max {
                    violations.push(format!(
                        "{} has minOccur {} greater than maxOccur {}",
                        node.path, min, max
                    ));
                }
            }

            let Some(items_index) = find_direct_child_by_tag(doc, node, "items") else {
                violations.push(format!("{} must contain items child", node.path));
                continue;
            };

            let remove_button = doc.nodes[items_index]
                .children
                .iter()
                .map(|index| &doc.nodes[*index])
                .find(|child| {
                    child
                        .attributes
                        .get("name")
                        .is_some_and(|name| name == "BT_Remove")
                });
            match remove_button {
                Some(button) => validate_repeatable_button_child(
                    doc,
                    button,
                    "fd:click",
                    "removeInstance",
                    violations,
                ),
                None => violations.push(format!(
                    "{} repeatable items must contain BT_Remove button",
                    node.path
                )),
            }

            let add_button = doc.nodes[node.parent.unwrap_or_default()]
                .children
                .iter()
                .map(|index| &doc.nodes[*index])
                .find(|child| {
                    child
                        .attributes
                        .get("name")
                        .is_some_and(|name| name == "BT_Add")
                });
            match add_button {
                Some(button) => {
                    validate_repeatable_button_child(
                        doc,
                        button,
                        "fd:click",
                        "addInstance",
                        violations,
                    );
                    let Some(scripts) = find_direct_child_by_tag(doc, button, "fd:scripts") else {
                        violations.push(format!("{} must contain fd:scripts child", button.path));
                        continue;
                    };
                    if !doc.nodes[scripts].attributes.contains_key("fd:init") {
                        violations.push(format!(
                            "{} must contain fd:init script",
                            doc.nodes[scripts].path
                        ));
                    }
                }
                None => violations.push(format!(
                    "{} repeatable parent must contain BT_Add button sibling",
                    node.path
                )),
            }
        }
    }
}

fn validate_responsive_nodes(doc: &ParsedXmlDocument, violations: &mut Vec<String>) {
    for node in &doc.nodes {
        if node.tag == "cq:responsive" {
            let Some(default_index) = find_direct_child_by_tag(doc, node, "default") else {
                violations.push(format!("{} must contain default child", node.path));
                continue;
            };
            let default_node = &doc.nodes[default_index];
            require_non_empty_attr(default_node, "offset", violations);
            require_non_empty_attr(default_node, "width", violations);
        }
    }
}

fn validate_repeatable_button_child(
    doc: &ParsedXmlDocument,
    button: &ParsedXmlNode,
    script_attr: &str,
    needle: &str,
    violations: &mut Vec<String>,
) {
    let Some(scripts_index) = find_direct_child_by_tag(doc, button, "fd:scripts") else {
        violations.push(format!("{} must contain fd:scripts child", button.path));
        return;
    };
    let scripts = &doc.nodes[scripts_index];
    require_attr_value(scripts, "jcr:primaryType", "nt:unstructured", violations);
    let Some(payload) = scripts.attributes.get(script_attr) else {
        violations.push(format!("{} must contain {}", scripts.path, script_attr));
        return;
    };
    if !payload.contains(needle) {
        violations.push(format!(
            "{} {} must reference {}",
            scripts.path, script_attr, needle
        ));
    }
}

fn require_direct_child(
    doc: &ParsedXmlDocument,
    parent_index: usize,
    tag: &str,
    violations: &mut Vec<String>,
) {
    if find_direct_child(doc, parent_index, tag).is_none() {
        violations.push(format!(
            "{} must contain direct child {}",
            doc.nodes[parent_index].path, tag
        ));
    }
}

fn require_non_empty_attr(node: &ParsedXmlNode, attr: &str, violations: &mut Vec<String>) {
    match node.attributes.get(attr) {
        Some(value) if !value.trim().is_empty() => {}
        Some(_) => violations.push(format!("{} has empty {}", node.path, attr)),
        None => violations.push(format!("{} is missing {}", node.path, attr)),
    }
}

fn require_attr_value(
    node: &ParsedXmlNode,
    attr: &str,
    expected: &str,
    violations: &mut Vec<String>,
) {
    match node.attributes.get(attr) {
        Some(value) if value == expected => {}
        Some(value) => violations.push(format!(
            "{} must have {}='{}', got '{}'",
            node.path, attr, expected, value
        )),
        None => violations.push(format!("{} is missing {}", node.path, attr)),
    }
}

fn find_direct_child(doc: &ParsedXmlDocument, parent_index: usize, tag: &str) -> Option<usize> {
    doc.nodes[parent_index]
        .children
        .iter()
        .copied()
        .find(|child| doc.nodes[*child].tag == tag)
}

fn find_direct_child_by_tag(
    doc: &ParsedXmlDocument,
    parent: &ParsedXmlNode,
    tag: &str,
) -> Option<usize> {
    let parent_index = doc
        .nodes
        .iter()
        .position(|node| std::ptr::eq(node, parent))
        .expect("parent node should exist in parsed document");
    find_direct_child(doc, parent_index, tag)
}

fn has_container_parent(doc: &ParsedXmlDocument, node: &ParsedXmlNode) -> bool {
    node.parent
        .map(|parent_index| {
            doc.nodes[parent_index]
                .attributes
                .contains_key("guideNodeClass")
                || matches!(
                    doc.nodes[parent_index].tag.as_str(),
                    "guideContainer" | "rootPanel"
                )
        })
        .unwrap_or(false)
}

fn xml_prefix(value: &str) -> Option<&str> {
    value.split_once(':').map(|(prefix, _)| prefix)
}

fn canonical_namespaces() -> [(&'static str, &'static str); 6] {
    [
        ("jcr", "http://www.jcp.org/jcr/1.0"),
        ("sling", "http://sling.apache.org/jcr/sling/1.0"),
        ("cq", "http://www.day.com/jcr/cq/1.0"),
        ("nt", "http://www.jcp.org/jcr/nt/1.0"),
        ("fd", "http://www.adobe.com/aemfd/fd/1.0"),
        ("dam", "http://www.day.com/dam/1.0"),
    ]
}

fn looks_like_iso_datetime(value: &str) -> bool {
    value.contains('T')
        && value.chars().take(4).all(|c| c.is_ascii_digit())
        && (value.ends_with('Z') || value.contains('+') || value.rfind('-').is_some())
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
            } else if b == b'<' {
                let start = i.saturating_sub(100);
                let end = (i + 100).min(xml.len());
                let snippet = &xml[start..end];
                panic!(
                    "Unescaped '<' inside XML attribute value at byte {i}.\n\nContext:\n{snippet}",
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

/// Load the UBS XSD config from `profiles/ubs/xsd/`, including registered types.
pub fn load_ubs_xsd_config() -> crate::xsd::XsdConfig {
    let dir_path = profiles_path("ubs/xsd");
    let dir = std::path::Path::new(&dir_path);
    crate::xsd::load_xsd_config_from_dir(dir)
        .unwrap_or_else(|e| panic!("Failed to load UBS XSD config: {e}"))
}

/// Recursively walk an AemNode tree, calling `callback` on every node.
pub fn walk_aem_nodes(node: &crate::aem::AemNode, callback: &mut impl FnMut(&crate::aem::AemNode)) {
    callback(node);
    match node {
        crate::aem::AemNode::Root { children, .. }
        | crate::aem::AemNode::Panel { children, .. }
        | crate::aem::AemNode::Repeatable { children, .. } => {
            for child in children {
                walk_aem_nodes(child, callback);
            }
        }
        _ => {}
    }
}

/// Count `AemNode::Fragment` nodes in the tree and collect their details.
pub fn collect_aem_fragment_refs(root: &crate::aem::AemNode) -> Vec<(String, Option<String>)> {
    let mut fragments = Vec::new();
    walk_aem_nodes(root, &mut |node| {
        if let crate::aem::AemNode::Fragment {
            frag_ref, bind_ref, ..
        } = node
        {
            fragments.push((frag_ref.clone(), bind_ref.clone()));
        }
    });
    fragments
}
