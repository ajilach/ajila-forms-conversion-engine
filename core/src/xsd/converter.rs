//! XSD schema generation from structured nodes.
//!
//! Traverses the `StructuredNode` tree and builds an `XsdSchema` tree
//! (intermediate representation), which can then be serialized to XML.
//! Headings create nested `xs:complexType` hierarchies, fields become
//! `xs:element` declarations, conditionals become `xs:choice`, and
//! repeatables use `minOccurs`/`maxOccurs`.

use crate::structured::{FieldId, FieldNode, FieldType, GroupNode, HeadingNode, StructuredNode};

use super::{
    XsdConfig, XsdNode, XsdRestriction, XsdSchema, find_matching_types, resolve_element,
    resolve_section_name_with_heading, to_pascal_case,
};

// ============================================================================
// Public API
// ============================================================================

/// Generate a complete XSD schema tree from structured nodes.
///
/// Returns an `XsdSchema` with includes (deduplicated, only for used types)
/// and a root `<xs:element name="form">` wrapping all generated content.
pub fn generate_xsd_schema(nodes: &[StructuredNode], config: &XsdConfig) -> XsdSchema {
    let sections = build_heading_hierarchy(nodes);
    let mut body = Vec::new();
    for section in &sections {
        build_section(section, config, &mut body);
    }

    // Collect all non-builtin type refs used in the tree
    let mut used_refs = std::collections::HashSet::new();
    for node in &body {
        collect_type_refs(node, &mut used_refs);
    }

    // Build deduplicated, sorted include list
    let include_paths: std::collections::BTreeSet<&str> = config
        .type_to_file
        .iter()
        .filter(|(name, _)| used_refs.contains(name.as_str()))
        .map(|(_, path)| path.as_str())
        .collect();

    let root = XsdNode::Element {
        name: config.root_element_name(),
        type_ref: None,
        min_occurs: None,
        max_occurs: None,
        content: Some(Box::new(XsdNode::ComplexType {
            name: None,
            sequence: body,
        })),
    };

    XsdSchema {
        includes: include_paths.into_iter().map(|s| s.to_string()).collect(),
        root,
    }
}

/// Generate a complete XSD schema XML string from structured nodes.
pub fn generate_xsd(nodes: &[StructuredNode], config: &XsdConfig) -> String {
    generate_xsd_schema(nodes, config).to_xml()
}

/// Recursively collect non-builtin type references from an XsdNode tree.
fn collect_type_refs<'a>(node: &'a XsdNode, refs: &mut std::collections::HashSet<&'a str>) {
    match node {
        XsdNode::Element {
            type_ref, content, ..
        } => {
            if let Some(tr) = type_ref {
                if !tr.starts_with("xs:") {
                    refs.insert(tr.as_str());
                }
            }
            if let Some(child) = content {
                collect_type_refs(child, refs);
            }
        }
        XsdNode::Ref { ref_name, .. } => {
            // A ref= references a global element; look up its type in the
            // include index so the corresponding file is included.
            refs.insert(ref_name.as_str());
        }
        XsdNode::ComplexType { sequence, .. } => {
            for child in sequence {
                collect_type_refs(child, refs);
            }
        }
        XsdNode::SimpleType { .. } => {}
        XsdNode::Choice { options } => {
            for branch in options {
                for child in branch {
                    collect_type_refs(child, refs);
                }
            }
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

    for node in nodes {
        match node {
            StructuredNode::Heading(heading) => {
                let level = heading.level.as_u8();

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
// Section text extraction (for keyword/regex matching)
// ============================================================================

/// Collect all plain text from a section (heading + all children) into a
/// single string suitable for keyword/regex matching.
fn section_full_text(section: &Section, config: &XsdConfig) -> String {
    let mut parts = Vec::new();
    if let Some(heading) = &section.heading {
        parts.push(config.label_text(&heading.content));
    }
    for item in &section.children {
        collect_item_text(item, config, &mut parts);
    }
    parts.join(" ")
}

/// Recursively collect text from a SectionItem.
fn collect_item_text(item: &SectionItem, config: &XsdConfig, parts: &mut Vec<String>) {
    match item {
        SectionItem::SubSection(sub) => {
            if let Some(heading) = &sub.heading {
                parts.push(config.label_text(&heading.content));
            }
            for child in &sub.children {
                collect_item_text(child, config, parts);
            }
        }
        SectionItem::Node(node) => {
            collect_node_text(node, config, parts);
        }
    }
}

/// Recursively collect text from a StructuredNode.
fn collect_node_text(node: &StructuredNode, config: &XsdConfig, parts: &mut Vec<String>) {
    match node {
        StructuredNode::Heading(h) => {
            parts.push(config.label_text(&h.content));
        }
        StructuredNode::Paragraph(p) => {
            parts.push(config.label_text(&p.content));
        }
        StructuredNode::Field(f) => {
            if let Some(label) = &f.label {
                parts.push(config.label_text(label));
            }
        }
        StructuredNode::Group(g) => {
            for child in &g.children {
                collect_node_text(child, config, parts);
            }
        }
        StructuredNode::Repeatable(r) => {
            collect_node_text(&r.item, config, parts);
        }
        StructuredNode::Conditional(c) => {
            collect_node_text(&c.content, config, parts);
        }
        StructuredNode::GridLayout(grid) => {
            for elem in &grid.elements {
                collect_node_text(&elem.node, config, parts);
            }
        }
        StructuredNode::List(list) => {
            for item in &list.items {
                parts.push(config.label_text(&item.content));
            }
        }
        StructuredNode::Table(table) => {
            if let Some(caption) = &table.caption {
                parts.push(config.label_text(caption));
            }
        }
        StructuredNode::Footnote(n) => {
            parts.push(config.label_text(&n.content));
        }
        StructuredNode::Image(_) | StructuredNode::Empty => {}
    }
}

// ============================================================================
// Section → XsdNode building
// ============================================================================

/// Build XsdNode(s) for a section (heading + children).
fn build_section(section: &Section, config: &XsdConfig, out: &mut Vec<XsdNode>) {
    match &section.heading {
        Some(heading) => {
            let label = config.label_text(&heading.content);
            let heading_text = config.label_text(&heading.content);
            let name = resolve_section_name_with_heading(
                &section_full_text(section, config),
                Some(&heading_text),
                &config.profile,
            )
            .unwrap_or_else(|| to_pascal_case(&label));

            // Collect child (name, type) pairs and try auto-matching
            let child_pairs = collect_child_name_type_pairs(section, config);
            let matched = find_matching_types(&child_pairs, &config.registered_types);

            if matched.len() == 1 {
                // Single type covers all children.
                out.push(XsdNode::Element {
                    name,
                    type_ref: Some(matched[0].name.clone()),
                    min_occurs: None,
                    max_occurs: None,
                    content: None,
                });
            } else if matched.len() > 1 {
                // Multiple disjoint types cover all children
                let sequence: Vec<XsdNode> = matched
                    .iter()
                    .map(|rt| {
                        let elem_name = config
                            .type_to_element_name
                            .get(&rt.name)
                            .cloned()
                            .unwrap_or_else(|| rt.name.trim_end_matches("Type").to_string());
                        XsdNode::Element {
                            name: elem_name,
                            type_ref: Some(rt.name.clone()),
                            min_occurs: None,
                            max_occurs: None,
                            content: None,
                        }
                    })
                    .collect();

                out.push(XsdNode::Element {
                    name,
                    type_ref: None,
                    min_occurs: None,
                    max_occurs: None,
                    content: Some(Box::new(XsdNode::ComplexType {
                        name: None,
                        sequence,
                    })),
                });
            } else {
                // No match → inline complexType
                let mut children = Vec::new();
                build_section_children(section, config, &mut children);
                out.push(XsdNode::Element {
                    name,
                    type_ref: None,
                    min_occurs: None,
                    max_occurs: None,
                    content: Some(Box::new(XsdNode::ComplexType {
                        name: None,
                        sequence: children,
                    })),
                });
            }
        }
        None => {
            // Preamble section (no heading) — emit children directly
            build_section_children(section, config, out);
        }
    }
}

/// Collect resolved (name, type) pairs for the direct children of a section.
///
/// For fields, resolves against `[elements]` config. Unmatched items use
/// snake_case from the label with `xs:string` as the default type.
fn collect_child_name_type_pairs(section: &Section, config: &XsdConfig) -> Vec<(String, String)> {
    let mut pairs = Vec::new();

    for item in &section.children {
        match item {
            SectionItem::SubSection(_) => {
                // Sub-sections are headings, not simple elements — skip for matching
            }
            SectionItem::Node(node) => {
                collect_node_name_type_pairs(node, config, &mut pairs);
            }
        }
    }

    pairs
}

/// Collect (name, type) pairs from a single node.
fn collect_node_name_type_pairs(
    node: &StructuredNode,
    config: &XsdConfig,
    pairs: &mut Vec<(String, String)>,
) {
    match node {
        StructuredNode::Field(field) => {
            let label = field
                .label
                .as_ref()
                .map(|l| config.label_text(l))
                .unwrap_or_default();
            let (name, type_ref) = match resolve_element(&label, &config.profile) {
                Some(res) => (res.name, res.type_ref),
                None => (to_pascal_case(&label), "xs:string".to_string()),
            };
            pairs.push((name, type_ref));
        }
        StructuredNode::Repeatable(rep) => {
            collect_node_name_type_pairs(&rep.item, config, pairs);
        }
        StructuredNode::Group(group) => {
            for child in &group.children {
                collect_node_name_type_pairs(child, config, pairs);
            }
        }
        StructuredNode::GridLayout(grid) => {
            for elem in &grid.elements {
                collect_node_name_type_pairs(&elem.node, config, pairs);
            }
        }
        _ => {}
    }
}

/// Build XsdNode(s) for the children of a section.
fn build_section_children(section: &Section, config: &XsdConfig, out: &mut Vec<XsdNode>) {
    let items = &section.children;
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            SectionItem::SubSection(sub) => {
                build_section(sub, config, out);
                i += 1;
            }
            SectionItem::Node(node) => {
                if let StructuredNode::Conditional(cond) = node {
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
                    out.push(build_choice(&items[start..end], config));
                    i = end;
                } else {
                    build_node(node, config, out, None, None);
                    i += 1;
                }
            }
        }
    }
}

// ============================================================================
// Node → XsdNode building
// ============================================================================

/// Build XsdNode(s) for a single structured node.
fn build_node(
    node: &StructuredNode,
    config: &XsdConfig,
    out: &mut Vec<XsdNode>,
    min_occurs: Option<u32>,
    max_occurs: Option<Option<u32>>,
) {
    match node {
        StructuredNode::Field(field) => {
            out.push(build_field(field, config, min_occurs, max_occurs));
        }
        StructuredNode::Repeatable(rep) => {
            build_node(
                &rep.item,
                config,
                out,
                Some(rep.min_occurrences),
                Some(rep.max_occurrences),
            );
        }
        StructuredNode::Group(group) => {
            build_group(group, config, out);
        }
        StructuredNode::Conditional(cond) => {
            out.push(build_choice(
                &[SectionItem::Node(StructuredNode::Conditional(cond.clone()))],
                config,
            ));
        }
        StructuredNode::GridLayout(grid) => {
            for elem in &grid.elements {
                build_node(&elem.node, config, out, None, None);
            }
        }
        // Presentational nodes — skip
        StructuredNode::Heading(_)
        | StructuredNode::Paragraph(_)
        | StructuredNode::Image(_)
        | StructuredNode::Table(_)
        | StructuredNode::List(_)
        | StructuredNode::Footnote(_)
        | StructuredNode::Empty => {}
    }
}

/// Build an XsdNode for a field.
fn build_field(
    field: &FieldNode,
    config: &XsdConfig,
    min_occurs: Option<u32>,
    max_occurs: Option<Option<u32>>,
) -> XsdNode {
    let label = field
        .label
        .as_ref()
        .map(|l| config.label_text(l))
        .unwrap_or_default();

    let (name, type_ref) = match resolve_element(&label, &config.profile) {
        Some(res) => (res.name, res.type_ref),
        None => (to_pascal_case(&label), "xs:string".to_string()),
    };

    let restrictions = collect_restrictions(field);

    if restrictions.is_empty() {
        XsdNode::Element {
            name,
            type_ref: Some(type_ref),
            min_occurs,
            max_occurs,
            content: None,
        }
    } else {
        XsdNode::Element {
            name,
            type_ref: None,
            min_occurs,
            max_occurs,
            content: Some(Box::new(XsdNode::SimpleType {
                base: type_ref,
                restrictions,
            })),
        }
    }
}

/// Collect XSD restriction facets for a field based on its FieldType.
fn collect_restrictions(field: &FieldNode) -> Vec<XsdRestriction> {
    let mut restrictions = Vec::new();

    match &field.input_type {
        FieldType::Text {
            regex,
            max_length,
            min_length,
        } => {
            if let Some(pattern) = regex {
                restrictions.push(XsdRestriction::Pattern(pattern.clone()));
            }
            if let Some(min) = min_length {
                restrictions.push(XsdRestriction::MinLength(*min));
            }
            if let Some(max) = max_length {
                restrictions.push(XsdRestriction::MaxLength(*max));
            }
        }
        FieldType::Textarea { max_length } => {
            if let Some(max) = max_length {
                restrictions.push(XsdRestriction::MaxLength(*max));
            }
        }
        FieldType::Number { min, max, .. } => {
            if let Some(min_val) = min {
                restrictions.push(XsdRestriction::MinInclusive(min_val.to_string()));
            }
            if let Some(max_val) = max {
                restrictions.push(XsdRestriction::MaxInclusive(max_val.to_string()));
            }
        }
        FieldType::Radio { options } | FieldType::Select { options } => {
            for opt in options {
                let value_str = match &opt.value {
                    crate::structured::InputValue::Text(s) => s.clone(),
                    crate::structured::InputValue::Number(n) => n.to_string(),
                    crate::structured::InputValue::Bool(b) => b.to_string(),
                };
                restrictions.push(XsdRestriction::Enumeration(value_str));
            }
        }
        _ => {}
    }

    restrictions
}

/// Build XsdNode(s) for a group node (recurse into children).
fn build_group(group: &GroupNode, config: &XsdConfig, out: &mut Vec<XsdNode>) {
    let mut i = 0;
    let children = &group.children;

    while i < children.len() {
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
            let section_items: Vec<SectionItem> = children[start..end]
                .iter()
                .map(|n| SectionItem::Node(n.clone()))
                .collect();
            out.push(build_choice(&section_items, config));
            i = end;
        } else {
            build_node(&children[i], config, out, None, None);
            i += 1;
        }
    }
}

/// Build an `XsdNode::Choice` from a slice of conditional node items.
fn build_choice(items: &[SectionItem], config: &XsdConfig) -> XsdNode {
    let mut options = Vec::new();

    for item in items {
        if let SectionItem::Node(StructuredNode::Conditional(cond)) = item {
            let mut branch = Vec::new();
            build_conditional_content(&cond.content, config, &mut branch);
            options.push(branch);
        }
    }

    XsdNode::Choice { options }
}

/// Build XsdNode(s) for the content of a conditional branch.
fn build_conditional_content(node: &StructuredNode, config: &XsdConfig, out: &mut Vec<XsdNode>) {
    match node {
        StructuredNode::Group(group) => {
            for child in &group.children {
                build_conditional_content(child, config, out);
            }
        }
        _ => {
            build_node(node, config, out, None, None);
        }
    }
}

// ============================================================================
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
    let root_path = format!("/{}", config.root_element_name());
    let sections = build_heading_hierarchy(nodes);
    for section in &sections {
        collect_section_bind_refs(section, &root_path, config, &mut maps);
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
            let label = config.label_text(&heading.content);
            let heading_text = config.label_text(&heading.content);
            let name = resolve_section_name_with_heading(
                &section_full_text(section, config),
                Some(&heading_text),
                &config.profile,
            )
            .unwrap_or_else(|| to_pascal_case(&label));
            let path = format!("{}/{}", parent_path, name);
            // Use or_insert so that the first (shallowest) heading with a
            // given label wins.  The AEM converter only creates panels for
            // top-level H2 headings, so deeper sub-headings with the same
            // text must not overwrite the earlier entry.
            maps.sections
                .entry(label.trim().to_string())
                .or_insert(path.clone());
            path
        }
        None => parent_path.to_string(),
    };

    // Mirror the type-matching logic from build_section: when a section is
    // matched to multiple disjoint types, the XSD generator wraps each type's
    // fields under a wrapper element, adding an extra path level.
    let wrapper_paths = if section.heading.is_some() {
        let child_pairs = collect_child_name_type_pairs(section, config);
        let matched = find_matching_types(&child_pairs, &config.registered_types);
        if matched.len() > 1 {
            Some(build_multi_type_wrapper_paths(
                &matched,
                config,
                &current_path,
            ))
        } else {
            None
        }
    } else {
        None
    };

    collect_section_items_bind_refs(
        &section.children,
        &current_path,
        config,
        maps,
        wrapper_paths.as_ref(),
    );
}

type WrapperPaths = std::collections::HashMap<(String, String), String>;

/// For each matched type, compute the wrapper element path that the XSD
/// generator would produce, and map every child element of that type to it.
fn build_multi_type_wrapper_paths(
    matched: &[&super::RegisteredComplexType],
    config: &super::XsdConfig,
    section_path: &str,
) -> WrapperPaths {
    let mut map = WrapperPaths::new();
    for rt in matched {
        let elem_name = config
            .type_to_element_name
            .get(&rt.name)
            .cloned()
            .unwrap_or_else(|| rt.name.trim_end_matches("Type").to_string());
        let wrapper_path = format!("{}/{}", section_path, elem_name);
        for child in &rt.elements {
            map.insert(
                (child.name.clone(), child.type_ref.clone()),
                wrapper_path.clone(),
            );
        }
    }
    map
}

/// Accumulate bind-ref paths for the direct children of a section.
///
/// When `wrappers` is `Some` (multi-type matched section), field paths are
/// routed through the appropriate wrapper element.
fn collect_section_items_bind_refs(
    items: &[SectionItem],
    current_path: &str,
    config: &super::XsdConfig,
    maps: &mut BindRefMaps,
    wrappers: Option<&WrapperPaths>,
) {
    for item in items {
        match item {
            SectionItem::SubSection(sub) => {
                collect_section_bind_refs(sub, current_path, config, maps);
            }
            SectionItem::Node(node) => {
                collect_node_bind_refs(node, current_path, config, maps, wrappers);
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
    wrappers: Option<&WrapperPaths>,
) {
    match node {
        StructuredNode::Field(field) => {
            let label = field
                .label
                .as_ref()
                .map(|l| config.label_text(l))
                .unwrap_or_default();
            let (name, type_ref) = match resolve_element(&label, &config.profile) {
                Some(res) => (res.name, res.type_ref),
                None => (to_pascal_case(&label), "xs:string".to_string()),
            };
            if !name.is_empty() && name != "Unknown" {
                let base_path = wrappers
                    .and_then(|wp| wp.get(&(name.clone(), type_ref)))
                    .map(|s| s.as_str())
                    .unwrap_or(current_path);
                let path = format!("{}/{}", base_path, name);
                maps.fields.insert(field.name.clone(), path);
            }
        }
        StructuredNode::Repeatable(rep) => {
            collect_node_bind_refs(&rep.item, current_path, config, maps, wrappers);
        }
        StructuredNode::Group(group) => {
            for child in &group.children {
                collect_node_bind_refs(child, current_path, config, maps, wrappers);
            }
        }
        StructuredNode::Conditional(cond) => {
            collect_conditional_node_bind_refs(&cond.content, current_path, config, maps, wrappers);
        }
        StructuredNode::GridLayout(grid) => {
            for elem in &grid.elements {
                collect_node_bind_refs(&elem.node, current_path, config, maps, wrappers);
            }
        }
        StructuredNode::Heading(_)
        | StructuredNode::Paragraph(_)
        | StructuredNode::Image(_)
        | StructuredNode::Table(_)
        | StructuredNode::List(_)
        | StructuredNode::Footnote(_)
        | StructuredNode::Empty => {}
    }
}

/// Accumulate bind-ref paths for the content of a conditional branch.
///
/// Groups are transparent, all other nodes delegate to [`collect_node_bind_refs`].
fn collect_conditional_node_bind_refs(
    node: &StructuredNode,
    current_path: &str,
    config: &super::XsdConfig,
    maps: &mut BindRefMaps,
    wrappers: Option<&WrapperPaths>,
) {
    match node {
        StructuredNode::Group(group) => {
            for child in &group.children {
                collect_conditional_node_bind_refs(child, current_path, config, maps, wrappers);
            }
        }
        _ => {
            collect_node_bind_refs(node, current_path, config, maps, wrappers);
        }
    }
}
