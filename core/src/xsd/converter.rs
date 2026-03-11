//! XSD schema generation from structured nodes.
//!
//! Traverses the `StructuredNode` tree and emits an XSD schema string.
//! Headings create nested `xs:complexType` hierarchies, fields become
//! `xs:element` declarations, conditionals become `xs:choice`, and
//! repeatables use `minOccurs`/`maxOccurs`.

use crate::structured::{
    FieldId, FieldNode, FieldType, GroupNode, HeadingNode, RepeatableNode, StructuredNode,
};

use super::{XsdConfig, resolve_complex_type, resolve_element, to_snake_case};

// ============================================================================
// Public API
// ============================================================================

/// Generate a complete XSD schema string from structured nodes.
///
/// The output wraps all generated content in an `<xs:schema>` root element,
/// including any predefined type definitions from the config.
pub fn generate_xsd(nodes: &[StructuredNode], config: &XsdConfig) -> String {
    let mut ctx = GeneratorContext::new();

    // Build the heading-based hierarchy first, then generate XSD.
    let sections = build_heading_hierarchy(nodes);
    for section in &sections {
        generate_section(section, config, &mut ctx, 2);
    }

    // Assemble the full schema
    let mut output = String::new();
    output.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    output.push_str("<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\n");

    // Emit xs:include directives only for types actually used in the schema,
    // deduplicated by path (multiple entries may point to the same file).
    let include_paths: std::collections::BTreeSet<&str> = config
        .profile
        .includes
        .iter()
        .filter(|(name, _)| ctx.used_type_refs.contains(*name))
        .map(|(_, inc)| inc.path.as_str())
        .collect();
    for path in &include_paths {
        output.push_str(&format!("  <xs:include schemaLocation=\"{}\"/>\n", path));
    }
    if !include_paths.is_empty() {
        output.push('\n');
    }

    // Include predefined type definitions
    for type_fragment in &config.predefined_types {
        for line in type_fragment.lines() {
            output.push_str("  ");
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }

    // Root element wrapping all generated content
    output.push_str("  <xs:element name=\"form\">\n");
    output.push_str("    <xs:complexType>\n");
    output.push_str("      <xs:sequence>\n");
    output.push_str(&ctx.body);
    output.push_str("      </xs:sequence>\n");
    output.push_str("    </xs:complexType>\n");
    output.push_str("  </xs:element>\n");

    output.push_str("</xs:schema>\n");
    output
}

// ============================================================================
// Generator context
// ============================================================================

/// Mutable state carried through generation.
struct GeneratorContext {
    /// The accumulated XSD body content (inside the root element's sequence).
    body: String,
    /// Type names referenced via `type="..."` attributes during generation.
    /// Used to determine which xs:include directives are actually needed.
    used_type_refs: std::collections::HashSet<String>,
}

impl GeneratorContext {
    fn new() -> Self {
        Self {
            body: String::new(),
            used_type_refs: std::collections::HashSet::new(),
        }
    }

    /// Record a type reference. Builtin xs: types are ignored since they
    /// never require an xs:include directive.
    fn record_type_ref(&mut self, type_ref: &str) {
        if !type_ref.starts_with("xs:") {
            self.used_type_refs.insert(type_ref.to_string());
        }
    }
}

// ============================================================================
// Heading hierarchy
// ============================================================================

/// A section created by splitting nodes at heading boundaries.
///
/// Each section has a heading (except possibly the root-level preamble)
/// and a list of child items: either nested sub-sections or leaf nodes.
#[derive(Debug)]
struct Section {
    /// The heading that starts this section (None for top-level preamble).
    heading: Option<HeadingNode>,
    /// Direct child nodes that are not sub-sections (fields, etc.).
    children: Vec<SectionItem>,
}

/// An item within a section — either a nested sub-section or a leaf node.
#[derive(Debug)]
enum SectionItem {
    SubSection(Section),
    Node(StructuredNode),
}

/// Build a heading-based hierarchy from a flat list of structured nodes.
///
/// Headings split the list into nested sections where heading level determines
/// nesting depth: H1 is the outermost, H6 the innermost. Non-heading nodes
/// become children of the most recently opened section.
fn build_heading_hierarchy(nodes: &[StructuredNode]) -> Vec<Section> {
    let mut stack: Vec<(u8, Section)> = Vec::new();
    let mut result: Vec<Section> = Vec::new();

    // Track the top-level items before any heading
    let mut preamble_items: Vec<SectionItem> = Vec::new();
    let mut seen_heading = false;

    for node in nodes {
        match node {
            StructuredNode::Heading(heading) => {
                let level = heading.level.as_u8();
                seen_heading = true;

                // Pop sections from the stack that are at the same or deeper level
                while let Some((stack_level, _)) = stack.last() {
                    if *stack_level >= level {
                        let (_, completed) = stack.pop().unwrap();
                        // Add completed section as a sub-section of the new parent
                        if let Some((_, parent)) = stack.last_mut() {
                            parent.children.push(SectionItem::SubSection(completed));
                        } else {
                            result.push(completed);
                        }
                    } else {
                        break;
                    }
                }

                // Push a new section for this heading
                stack.push((
                    level,
                    Section {
                        heading: Some(heading.clone()),
                        children: Vec::new(),
                    },
                ));
            }

            StructuredNode::Group(group) => {
                // Recurse into groups to find headings inside them
                let sub_items = flatten_group_for_hierarchy(group);
                for item in sub_items {
                    match item {
                        FlatItem::Heading(heading) => {
                            let level = heading.level.as_u8();
                            seen_heading = true;

                            while let Some((stack_level, _)) = stack.last() {
                                if *stack_level >= level {
                                    let (_, completed) = stack.pop().unwrap();
                                    if let Some((_, parent)) = stack.last_mut() {
                                        parent.children.push(SectionItem::SubSection(completed));
                                    } else {
                                        result.push(completed);
                                    }
                                } else {
                                    break;
                                }
                            }

                            stack.push((
                                level,
                                Section {
                                    heading: Some(heading),
                                    children: Vec::new(),
                                },
                            ));
                        }
                        FlatItem::Node(n) => {
                            if let Some((_, section)) = stack.last_mut() {
                                section.children.push(SectionItem::Node(n));
                            } else if !seen_heading {
                                preamble_items.push(SectionItem::Node(n));
                            } else {
                                preamble_items.push(SectionItem::Node(n));
                            }
                        }
                    }
                }
            }

            other => {
                if let Some((_, section)) = stack.last_mut() {
                    section.children.push(SectionItem::Node(other.clone()));
                } else {
                    preamble_items.push(SectionItem::Node(other.clone()));
                }
            }
        }
    }

    // Drain remaining stack
    while let Some((_, completed)) = stack.pop() {
        if let Some((_, parent)) = stack.last_mut() {
            parent.children.push(SectionItem::SubSection(completed));
        } else {
            result.push(completed);
        }
    }

    // If there were nodes before any heading, wrap them in a preamble section
    if !preamble_items.is_empty() {
        let mut all = vec![Section {
            heading: None,
            children: preamble_items,
        }];
        all.extend(result);
        result = all;
    }

    result
}

/// Items produced when flattening a GroupNode for hierarchy building.
enum FlatItem {
    Heading(HeadingNode),
    Node(StructuredNode),
}

/// Recursively flatten a GroupNode, extracting headings and other nodes.
fn flatten_group_for_hierarchy(group: &GroupNode) -> Vec<FlatItem> {
    let mut items = Vec::new();
    for child in &group.children {
        match child {
            StructuredNode::Heading(h) => items.push(FlatItem::Heading(h.clone())),
            StructuredNode::Group(g) => {
                items.extend(flatten_group_for_hierarchy(g));
            }
            other => items.push(FlatItem::Node(other.clone())),
        }
    }
    items
}

// ============================================================================
// Section → XSD generation
// ============================================================================

/// Generate XSD content for a section (heading + children).
fn generate_section(
    section: &Section,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
) {
    let indent_str = " ".repeat(indent);

    match &section.heading {
        Some(heading) => {
            let label = heading.content.as_plain_text();

            // Try to resolve against complexTypes config
            let resolved = resolve_complex_type(&label, &config.profile);

            match resolved {
                Some(ref res) if should_use_complex_type(res, section, config) => {
                    // Config match with child validation passed
                    if let Some(ref type_ref) = res.type_ref {
                        // Reference a predefined complexType
                        ctx.record_type_ref(type_ref);
                        ctx.body.push_str(&format!(
                            "{}<xs:element name=\"{}\" type=\"{}\"/>\n",
                            indent_str, res.name, type_ref
                        ));
                    } else {
                        // Inline complexType with children
                        ctx.body.push_str(&format!(
                            "{}<xs:element name=\"{}\">\n",
                            indent_str, res.name
                        ));
                        ctx.body
                            .push_str(&format!("{}  <xs:complexType>\n", indent_str));
                        ctx.body
                            .push_str(&format!("{}    <xs:sequence>\n", indent_str));
                        generate_section_children(section, config, ctx, indent + 6);
                        ctx.body
                            .push_str(&format!("{}    </xs:sequence>\n", indent_str));
                        ctx.body
                            .push_str(&format!("{}  </xs:complexType>\n", indent_str));
                        ctx.body.push_str(&format!("{}</xs:element>\n", indent_str));
                    }
                }
                _ => {
                    // No config match or child validation failed → inline complexType
                    // with snake_case name derived from the heading label
                    let name = to_snake_case(&label);
                    ctx.body
                        .push_str(&format!("{}<xs:element name=\"{}\">\n", indent_str, name));
                    ctx.body
                        .push_str(&format!("{}  <xs:complexType>\n", indent_str));
                    ctx.body
                        .push_str(&format!("{}    <xs:sequence>\n", indent_str));
                    generate_section_children(section, config, ctx, indent + 6);
                    ctx.body
                        .push_str(&format!("{}    </xs:sequence>\n", indent_str));
                    ctx.body
                        .push_str(&format!("{}  </xs:complexType>\n", indent_str));
                    ctx.body.push_str(&format!("{}</xs:element>\n", indent_str));
                }
            }
        }
        None => {
            // Preamble section (no heading) — just emit children directly
            generate_section_children(section, config, ctx, indent);
        }
    }
}

/// Check whether a resolved complexType config should be used, considering
/// the `required_children` / `optional_children` constraints.
fn should_use_complex_type(
    resolved: &super::ResolvedComplexType,
    section: &Section,
    config: &XsdConfig,
) -> bool {
    let mapping = &resolved.mapping;

    // If no child constraints are set, always match on synonym alone
    if mapping.required_children.is_none() && mapping.optional_children.is_none() {
        return true;
    }

    // Collect canonical names of direct children
    let child_names = collect_child_canonical_names(section, config);

    // Check required: every required child must be present
    if let Some(ref required) = mapping.required_children {
        for req in required {
            if !child_names.contains(req) {
                return false;
            }
        }
    }

    // Check strict subset: every child must be in required ∪ optional
    let allowed: std::collections::HashSet<&str> = {
        let mut set = std::collections::HashSet::new();
        if let Some(ref required) = mapping.required_children {
            for r in required {
                set.insert(r.as_str());
            }
        }
        if let Some(ref optional) = mapping.optional_children {
            for o in optional {
                set.insert(o.as_str());
            }
        }
        set
    };

    for child_name in &child_names {
        if !allowed.contains(child_name.as_str()) {
            return false;
        }
    }

    true
}

/// Collect the resolved canonical names of direct children in a section.
///
/// For fields, resolves against `[elements]` config; for sub-sections,
/// resolves against `[complexTypes]` config. Unmatched items use camelCase
/// from the label.
fn collect_child_canonical_names(section: &Section, config: &XsdConfig) -> Vec<String> {
    let mut names = Vec::new();

    for item in &section.children {
        match item {
            SectionItem::SubSection(sub) => {
                if let Some(ref heading) = sub.heading {
                    let label = heading.content.as_plain_text();
                    let name = match resolve_complex_type(&label, &config.profile) {
                        Some(res) => res.name,
                        None => to_snake_case(&label),
                    };
                    names.push(name);
                }
            }
            SectionItem::Node(node) => match node {
                StructuredNode::Field(field) => {
                    let label = field
                        .label
                        .as_ref()
                        .map(|l| l.as_plain_text())
                        .unwrap_or_default();
                    let name = match resolve_element(&label, &config.profile) {
                        Some(res) => res.name,
                        None => to_snake_case(&label),
                    };
                    names.push(name);
                }
                StructuredNode::Repeatable(rep) => {
                    // If the repeated item is a field, resolve it
                    if let StructuredNode::Field(field) = rep.item.as_ref() {
                        let label = field
                            .label
                            .as_ref()
                            .map(|l| l.as_plain_text())
                            .unwrap_or_default();
                        let name = match resolve_element(&label, &config.profile) {
                            Some(res) => res.name,
                            None => to_snake_case(&label),
                        };
                        names.push(name);
                    }
                }
                StructuredNode::Group(group) => {
                    // Recurse into groups to find fields
                    collect_group_child_names(group, config, &mut names);
                }
                StructuredNode::Conditional(_) => {
                    // Conditionals contribute their children's names
                    // but we don't add the conditional itself as a name
                }
                _ => {
                    // Presentational nodes (Paragraph, Image, Table, List, etc.)
                    // don't contribute to the schema
                }
            },
        }
    }

    names
}

/// Recursively collect canonical names from a GroupNode's children.
fn collect_group_child_names(group: &GroupNode, config: &XsdConfig, names: &mut Vec<String>) {
    for child in &group.children {
        match child {
            StructuredNode::Field(field) => {
                let label = field
                    .label
                    .as_ref()
                    .map(|l| l.as_plain_text())
                    .unwrap_or_default();
                let name = match resolve_element(&label, &config.profile) {
                    Some(res) => res.name,
                    None => to_snake_case(&label),
                };
                names.push(name);
            }
            StructuredNode::Group(g) => {
                collect_group_child_names(g, config, names);
            }
            _ => {}
        }
    }
}

/// Generate XSD for the children of a section.
fn generate_section_children(
    section: &Section,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
) {
    let items = &section.children;
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            SectionItem::SubSection(sub) => {
                generate_section(sub, config, ctx, indent);
                i += 1;
            }
            SectionItem::Node(node) => {
                // Check for conditional grouping
                if let StructuredNode::Conditional(cond) = node {
                    // Group adjacent conditionals with the same field_name
                    let field_name = &cond.condition.field_name;
                    let start = i;
                    let mut end = i + 1;
                    while end < items.len() {
                        if let SectionItem::Node(StructuredNode::Conditional(next_cond)) =
                            &items[end]
                        {
                            if &next_cond.condition.field_name == field_name {
                                end += 1;
                                continue;
                            }
                        }
                        break;
                    }

                    if end - start > 1 {
                        // Multiple conditionals → xs:choice
                        generate_choice(&items[start..end], config, ctx, indent);
                    } else {
                        // Single conditional — still wrap in xs:choice for schema correctness
                        generate_choice(&items[start..end], config, ctx, indent);
                    }
                    i = end;
                } else {
                    generate_node(node, config, ctx, indent, None, None);
                    i += 1;
                }
            }
        }
    }
}

// ============================================================================
// Node → XSD generation
// ============================================================================

/// Generate XSD for a single structured node.
fn generate_node(
    node: &StructuredNode,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
    min_occurs: Option<u32>,
    max_occurs: Option<Option<u32>>,
) {
    match node {
        StructuredNode::Field(field) => {
            generate_field(field, config, ctx, indent, min_occurs, max_occurs);
        }
        StructuredNode::Repeatable(rep) => {
            generate_repeatable(rep, config, ctx, indent);
        }
        StructuredNode::Group(group) => {
            generate_group(group, config, ctx, indent);
        }
        StructuredNode::Conditional(cond) => {
            // Single conditional not caught by the grouping logic
            generate_choice(
                &[SectionItem::Node(StructuredNode::Conditional(cond.clone()))],
                config,
                ctx,
                indent,
            );
        }
        StructuredNode::GridLayout(grid) => {
            // Recurse into grid elements
            for elem in &grid.elements {
                generate_node(&elem.node, config, ctx, indent, None, None);
            }
        }
        // Presentational nodes — skip
        StructuredNode::Heading(_)
        | StructuredNode::Paragraph(_)
        | StructuredNode::Image(_)
        | StructuredNode::Table(_)
        | StructuredNode::List(_)
        | StructuredNode::Empty => {}
    }
}

/// Generate XSD for a field node.
fn generate_field(
    field: &FieldNode,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
    min_occurs: Option<u32>,
    max_occurs: Option<Option<u32>>,
) {
    let indent_str = " ".repeat(indent);

    let label = field
        .label
        .as_ref()
        .map(|l| l.as_plain_text())
        .unwrap_or_default();

    // Resolve name and type
    let (name, type_ref) = match resolve_element(&label, &config.profile) {
        Some(res) => (res.name, res.type_ref),
        None => (to_snake_case(&label), "xs:string".to_string()),
    };
    ctx.record_type_ref(&type_ref);

    // Build occurrence attributes
    let occur_attrs = build_occurrence_attrs(min_occurs, max_occurs);

    // Check if we need restrictions (enumeration, pattern, etc.)
    let restrictions = collect_restrictions(field);

    if restrictions.is_empty() {
        // Simple element with type attribute
        ctx.body.push_str(&format!(
            "{}<xs:element name=\"{}\" type=\"{}\"{}/>\n",
            indent_str, name, type_ref, occur_attrs
        ));
    } else {
        // Element with inline simpleType restriction
        ctx.body.push_str(&format!(
            "{}<xs:element name=\"{}\"{}>\n",
            indent_str, name, occur_attrs
        ));
        ctx.body
            .push_str(&format!("{}  <xs:simpleType>\n", indent_str));
        ctx.body.push_str(&format!(
            "{}    <xs:restriction base=\"{}\">\n",
            indent_str, type_ref
        ));
        for restriction in &restrictions {
            ctx.body
                .push_str(&format!("{}      {}\n", indent_str, restriction));
        }
        ctx.body
            .push_str(&format!("{}    </xs:restriction>\n", indent_str));
        ctx.body
            .push_str(&format!("{}  </xs:simpleType>\n", indent_str));
        ctx.body.push_str(&format!("{}</xs:element>\n", indent_str));
    }
}

/// Collect XSD restriction facets for a field based on its FieldType.
fn collect_restrictions(field: &FieldNode) -> Vec<String> {
    let mut restrictions = Vec::new();

    match &field.input_type {
        FieldType::Text {
            regex,
            max_length,
            min_length,
        } => {
            if let Some(pattern) = regex {
                restrictions.push(format!("<xs:pattern value=\"{}\"/>", xml_escape(pattern)));
            }
            if let Some(min) = min_length {
                restrictions.push(format!("<xs:minLength value=\"{}\"/>", min));
            }
            if let Some(max) = max_length {
                restrictions.push(format!("<xs:maxLength value=\"{}\"/>", max));
            }
        }
        FieldType::Number { min, max, .. } => {
            if let Some(min_val) = min {
                restrictions.push(format!("<xs:minInclusive value=\"{}\"/>", min_val));
            }
            if let Some(max_val) = max {
                restrictions.push(format!("<xs:maxInclusive value=\"{}\"/>", max_val));
            }
        }
        FieldType::Radio { options } | FieldType::Select { options } => {
            for opt in options {
                let value_str = match &opt.value {
                    crate::structured::InputValue::Text(s) => s.clone(),
                    crate::structured::InputValue::Number(n) => n.to_string(),
                    crate::structured::InputValue::Bool(b) => b.to_string(),
                };
                restrictions.push(format!(
                    "<xs:enumeration value=\"{}\"/>",
                    xml_escape(&value_str)
                ));
            }
        }
        _ => {}
    }

    restrictions
}

/// Generate XSD for a repeatable node.
fn generate_repeatable(
    rep: &RepeatableNode,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
) {
    generate_node(
        &rep.item,
        config,
        ctx,
        indent,
        Some(rep.min_occurrences),
        Some(rep.max_occurrences),
    );
}

/// Generate XSD for a group node (recurse into children).
fn generate_group(
    group: &GroupNode,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
) {
    let mut i = 0;
    let children = &group.children;

    while i < children.len() {
        // Check for conditional grouping within the group
        if let StructuredNode::Conditional(cond) = &children[i] {
            let field_name = &cond.condition.field_name;
            let start = i;
            let mut end = i + 1;
            while end < children.len() {
                if let StructuredNode::Conditional(next_cond) = &children[end] {
                    if &next_cond.condition.field_name == field_name {
                        end += 1;
                        continue;
                    }
                }
                break;
            }

            // Convert to SectionItems for generate_choice
            let section_items: Vec<SectionItem> = children[start..end]
                .iter()
                .map(|n| SectionItem::Node(n.clone()))
                .collect();
            generate_choice(&section_items, config, ctx, indent);
            i = end;
        } else {
            generate_node(&children[i], config, ctx, indent, None, None);
            i += 1;
        }
    }
}

/// Generate an `xs:choice` from a slice of conditional node items.
fn generate_choice(
    items: &[SectionItem],
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
) {
    let indent_str = " ".repeat(indent);

    ctx.body.push_str(&format!("{}<xs:choice>\n", indent_str));

    for item in items {
        if let SectionItem::Node(StructuredNode::Conditional(cond)) = item {
            ctx.body
                .push_str(&format!("{}  <xs:sequence>\n", indent_str));
            generate_conditional_content(&cond.content, config, ctx, indent + 4);
            ctx.body
                .push_str(&format!("{}  </xs:sequence>\n", indent_str));
        }
    }

    ctx.body.push_str(&format!("{}</xs:choice>\n", indent_str));
}

/// Generate XSD content for the inner content of a conditional branch.
fn generate_conditional_content(
    node: &StructuredNode,
    config: &XsdConfig,
    ctx: &mut GeneratorContext,
    indent: usize,
) {
    match node {
        StructuredNode::Group(group) => {
            // Recurse into all group children
            for child in &group.children {
                generate_conditional_content(child, config, ctx, indent);
            }
        }
        _ => {
            generate_node(node, config, ctx, indent, None, None);
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Build the `minOccurs`/`maxOccurs` attribute string for an element.
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

/// Escape special XML characters in attribute values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ============================================================================
// BindRef computation (for AEM XSD binding)
// ============================================================================

/// Maps produced by [`compute_bind_refs`] and consumed by the AEM converter
/// to attach `bindRef` attributes to form components.
pub struct BindRefMaps {
    /// Maps a [`FieldId`] to its absolute XSD path (e.g. `/form/section/field_name`).
    pub fields: std::collections::HashMap<FieldId, String>,
    /// Maps a heading's plain-text label to the XSD path of the corresponding
    /// complex type element (e.g. `"Personal Data"` → `/form/personal_data`).
    /// Used to look up the bind path for AEM section panels.
    pub sections: std::collections::HashMap<String, String>,
}

/// Compute XSD bind-ref paths for all fields and heading sections in `nodes`.
///
/// Reuses the same name-resolution logic as [`generate_xsd`] so that the
/// resulting paths exactly match the elements produced by the XSD generator.
pub fn compute_bind_refs(nodes: &[StructuredNode], config: &super::XsdConfig) -> BindRefMaps {
    let mut maps = BindRefMaps {
        fields: std::collections::HashMap::new(),
        sections: std::collections::HashMap::new(),
    };
    let sections = build_heading_hierarchy(nodes);
    for section in &sections {
        collect_section_bind_refs(section, "/form", config, &mut maps);
    }
    maps
}

/// Recursively accumulate bind-ref paths for a single [`Section`].
fn collect_section_bind_refs(
    section: &Section,
    parent_path: &str,
    config: &super::XsdConfig,
    maps: &mut BindRefMaps,
) {
    let current_path = match &section.heading {
        Some(heading) => {
            let label = heading.content.as_plain_text();
            let resolved = resolve_complex_type(&label, &config.profile);
            let name = match resolved {
                Some(ref res) if should_use_complex_type(res, section, config) => res.name.clone(),
                _ => to_snake_case(&label),
            };
            let path = format!("{}/{}", parent_path, name);
            // Key by trimmed plain-text so it matches AEM panel titles.
            maps.sections.insert(label.trim().to_string(), path.clone());
            path
        }
        None => {
            // Preamble section (no heading): inherit parent path without adding a segment.
            parent_path.to_string()
        }
    };

    collect_section_items_bind_refs(&section.children, &current_path, config, maps);
}

/// Accumulate bind-ref paths for the direct children of a section.
fn collect_section_items_bind_refs(
    items: &[SectionItem],
    current_path: &str,
    config: &super::XsdConfig,
    maps: &mut BindRefMaps,
) {
    for item in items {
        match item {
            SectionItem::SubSection(sub) => {
                collect_section_bind_refs(sub, current_path, config, maps);
            }
            SectionItem::Node(node) => {
                collect_node_bind_refs(node, current_path, config, maps);
            }
        }
    }
}

/// Accumulate bind-ref paths for a single [`StructuredNode`].
fn collect_node_bind_refs(
    node: &StructuredNode,
    current_path: &str,
    config: &super::XsdConfig,
    maps: &mut BindRefMaps,
) {
    match node {
        StructuredNode::Field(field) => {
            let label = field
                .label
                .as_ref()
                .map(|l| l.as_plain_text())
                .unwrap_or_default();
            let name = match resolve_element(&label, &config.profile) {
                Some(res) => res.name,
                None => to_snake_case(&label),
            };
            // Skip empty names (fields with no label and no resolved name).
            if !name.is_empty() && name != "unknown" {
                let path = format!("{}/{}", current_path, name);
                maps.fields.insert(field.name.clone(), path);
            }
        }
        StructuredNode::Repeatable(rep) => {
            // XSD uses minOccurs/maxOccurs on the element itself — the inner
            // item gets the same path level as a non-repeatable field.
            collect_node_bind_refs(&rep.item, current_path, config, maps);
        }
        StructuredNode::Group(group) => {
            // Groups are transparent: recurse into children at the same path.
            for child in &group.children {
                collect_node_bind_refs(child, current_path, config, maps);
            }
        }
        StructuredNode::Conditional(cond) => {
            // Conditional content sits at the same path level (xs:choice doesn't
            // add a new named element).
            collect_conditional_node_bind_refs(&cond.content, current_path, config, maps);
        }
        StructuredNode::GridLayout(grid) => {
            for elem in &grid.elements {
                collect_node_bind_refs(&elem.node, current_path, config, maps);
            }
        }
        // Presentational / structural nodes — no XSD elements produced.
        StructuredNode::Heading(_)
        | StructuredNode::Paragraph(_)
        | StructuredNode::Image(_)
        | StructuredNode::Table(_)
        | StructuredNode::List(_)
        | StructuredNode::Empty => {}
    }
}

/// Accumulate bind-ref paths for the content of a conditional branch.
///
/// Mirrors [`generate_conditional_content`]: groups are transparent, all other
/// nodes delegate to [`collect_node_bind_refs`].
fn collect_conditional_node_bind_refs(
    node: &StructuredNode,
    current_path: &str,
    config: &super::XsdConfig,
    maps: &mut BindRefMaps,
) {
    match node {
        StructuredNode::Group(group) => {
            for child in &group.children {
                collect_conditional_node_bind_refs(child, current_path, config, maps);
            }
        }
        _ => {
            collect_node_bind_refs(node, current_path, config, maps);
        }
    }
}
