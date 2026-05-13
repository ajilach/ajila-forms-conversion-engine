//! Converter from `StructuredNode` trees to `AemNode` trees.
//!
//! The conversion is stateless apart from a small `ConversionContext` that
//! tracks UUID generation and naming counters.

use std::collections::HashMap;

use uuid::Uuid;

use crate::structured::{
    ConditionalNode, FieldId, FieldNode, FieldType, FootnoteNode, GridLayout, GroupNode,
    HeadingLevel, HeadingNode, ImageNode, InlineNode, InlineText, InputValue, ListNode, NameValue,
    ParagraphNode, RepeatableNode, StructuredNode, TableNode, TranslatableString,
};

use super::fragment_parser::ParsedFragment;
use super::{AemConfig, AemNode, AemOption, ConditionRule, OptionAlignment};

// ============================================================================
// Conversion context
// ============================================================================

/// Namespace UUID used for deterministic UUID v5 generation.
const NAMESPACE_AEM: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// Maximum number of characters for the CamelCase identifier portion of a
/// generated name (before prefix and short-UUID suffix are added).
const MAX_CAMEL_CASE_LEN: usize = 30;

/// Number of hex characters to take from the UUID for the short suffix.
const SHORT_UUID_LEN: usize = 8;

/// A collected conditional: maps a trigger field’s `FieldId` to a conditional
/// panel name and the value that should make it visible.
struct CollectedCondition {
    /// The `FieldId` of the field whose value triggers visibility.
    field_id: FieldId,
    /// AEM `name` of the conditional panel.
    panel_name: String,
    /// The value that makes the panel visible.
    value: InputValue,
}

/// Internal state carried through the conversion.
struct ConversionContext {
    /// Counter for generating unique seeds (avoids UUID collisions when two
    /// elements share the same source text).
    counter: u32,
    /// Whether to produce deterministic UUIDs.
    deterministic: bool,
    /// The language to prefer when extracting translatable strings.
    language: String,
    /// Conditions collected during the first pass.
    collected_conditions: Vec<CollectedCondition>,
    /// Pre-computed XSD bind-ref paths, populated when `bind_to_xsd` or `use_fragments` is true.
    bind_refs: Option<crate::xsd::BindRefMaps>,
    /// Map from field ID to human-readable label.
    field_labels: HashMap<FieldId, String>,
}

impl ConversionContext {
    fn new(config: &AemConfig) -> Self {
        Self {
            counter: 0,
            deterministic: config.deterministic_uuids,
            language: config.master_language.clone(),
            collected_conditions: Vec::new(),
            bind_refs: None,
            field_labels: HashMap::new(),
        }
    }

    /// Generate a UUID — deterministic from `seed` or random.
    fn uuid(&mut self, seed: &str) -> Uuid {
        if self.deterministic {
            Uuid::new_v5(&NAMESPACE_AEM, seed.as_bytes())
        } else {
            Uuid::new_v4()
        }
    }

    /// Build a meaningful name from a `prefix` and human-readable `source_text`.
    ///
    /// The result has the form `PREFIX_CamelCaseIdentifier_abcd1234` where:
    /// - `PREFIX` is the component-type prefix (e.g. `TXT`, `PN`, `ST`, …)
    /// - `CamelCaseIdentifier` is derived from `source_text`, truncated to
    ///   [`MAX_CAMEL_CASE_LEN`] characters at a word boundary.
    /// - `abcd1234` is the first [`SHORT_UUID_LEN`] hex characters of a
    ///   deterministic UUID v5 (or random v4).
    ///
    /// If `source_text` yields an empty CamelCase part the name degrades to
    /// `PREFIX_abcd1234`.
    fn make_name(&mut self, prefix: &str, source_text: &str) -> String {
        self.counter += 1;
        let camel = to_camel_case(source_text);
        // Seed includes counter so that identical labels still produce
        // distinct UUIDs.
        let seed = format!("{}_{}_{}", prefix, camel, self.counter);
        let uuid = self.uuid(&seed);
        let short = short_uuid(&uuid);
        if camel.is_empty() {
            format!("{}_{}", prefix, short)
        } else {
            format!("{}_{}_{}", prefix, camel, short)
        }
    }
}

// ============================================================================
// Naming helpers
// ============================================================================

/// Convert an arbitrary human-readable string to UpperCamelCase, truncated to
/// [`MAX_CAMEL_CASE_LEN`] characters at a word boundary.
///
/// The function:
/// 1. Strips HTML tags.
/// 2. Replaces any non-alphanumeric / non-whitespace character with a space.
/// 3. Splits on whitespace, capitalises each word's first letter.
/// 4. Joins without separator.
/// 5. Truncates to `MAX_CAMEL_CASE_LEN` at a word boundary (i.e. at an
///    uppercase letter that starts a new word).
fn to_camel_case(input: &str) -> String {
    // 1. Strip HTML tags
    let no_tags = strip_html_tags(input);

    // 2. Replace non-ASCII and non-alphanumeric chars with space
    //    (JCR/AEM node names must be ASCII-safe)
    let cleaned: String = no_tags
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || (c.is_ascii_whitespace()) {
                c
            } else {
                ' '
            }
        })
        .collect();

    // 3. Split on whitespace, capitalise first letter of each word
    let words: Vec<String> = cleaned
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    let rest: String = chars.collect();
                    format!("{}{}", upper, rest.to_lowercase())
                }
                None => String::new(),
            }
        })
        .collect();

    // 4. Join
    let joined = words.join("");

    // 5. Truncate at MAX_CAMEL_CASE_LEN on a word boundary
    truncate_camel_case(&joined, MAX_CAMEL_CASE_LEN)
}

/// Truncate a CamelCase string to at most `max_len` characters, cutting at a
/// word boundary (uppercase letter) when possible.
fn truncate_camel_case(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Walk backwards from max_len to find an uppercase letter boundary
    let bytes = s.as_bytes();
    for i in (1..max_len).rev() {
        if bytes[i].is_ascii_uppercase() {
            return s[..i].to_string();
        }
    }
    // No good boundary found — hard truncate
    s[..max_len].to_string()
}

/// Strip HTML tags from a string (simple regex-free approach).
fn strip_html_tags(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut inside_tag = false;
    for ch in input.chars() {
        if ch == '<' {
            inside_tag = true;
        } else if ch == '>' {
            inside_tag = false;
            result.push(' '); // replace tag with space
        } else if !inside_tag {
            result.push(ch);
        }
    }
    result
}

/// Return the first [`SHORT_UUID_LEN`] hex characters of a UUID.
fn short_uuid(uuid: &Uuid) -> String {
    let hex = uuid.as_simple().to_string();
    hex[..SHORT_UUID_LEN].to_string()
}

// ============================================================================
// Public API
// ============================================================================

/// Convert a slice of `StructuredNode`s into an `AemNode::Root`.
///
/// The returned root node wraps all converted children and carries the form
/// title from the supplied configuration.
///
/// H2 headings are used to split the top-level children into sub-panels:
/// each H2 starts a new panel whose title is the heading text, and all
/// following nodes (until the next H2) become that panel's children.
/// Nodes before the first H2 remain directly under the root.
pub fn convert_to_aem(nodes: &[StructuredNode], config: &AemConfig) -> AemNode {
    let mut ctx = ConversionContext::new(config);

    // Collect field labels for display in conditionals
    collect_field_labels(nodes, &config.master_language, &mut ctx.field_labels);

    // Pre-compute XSD bind-ref paths when either bind_to_xsd (so fields/panels
    // receive `bindRef` attributes) or use_fragments (so fragment matching can
    // work based on bind-ref leaf names) is enabled.
    if config.bind_to_xsd || config.use_fragments {
        if let Some(ref xsd_config) = config.xsd_config {
            ctx.bind_refs = Some(crate::xsd::compute_bind_refs(nodes, xsd_config));
        }
    }

    // Extract H1 heading text to use as the display title (guideformtitle _value).
    // Falls back to form_title (form code) if no H1 is present.
    let form_display_title = nodes
        .iter()
        .find_map(|n| {
            if let StructuredNode::Heading(h) = n {
                if matches!(h.level, HeadingLevel::H1) {
                    return Some(h.content.plain_text_in(&ctx.language).trim().to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| config.form_title.clone());

    // First pass: separate footnotes from normal nodes, then split by H2.
    // Each section is (Option<heading_text>, nodes_in_section).
    let mut sections: Vec<(Option<String>, Vec<&StructuredNode>)> = Vec::new();
    let mut footnotes: Vec<&FootnoteNode> = Vec::new();
    collect_all_footnotes(nodes, &mut footnotes);

    for node in nodes {
        if matches!(node, StructuredNode::Footnote(_)) {
            continue;
        }
        if let StructuredNode::Heading(h) = node {
            if matches!(h.level, HeadingLevel::H2) {
                let title = h.content.plain_text_in(&ctx.language).trim().to_string();
                sections.push((Some(title), vec![]));
                continue;
            }
        }
        // Append to the current section, or create a preamble section
        if let Some(last) = sections.last_mut() {
            last.1.push(node);
        } else {
            sections.push((None, vec![node]));
        }
    }

    // Second pass: convert each section into AemNodes.
    // Preamble content (before first H2) is merged into the first H2 section page,
    // not wrapped in its own page. This matches the reference structure where
    // preface/intro content appears at the start of the first content page.
    let mut children: Vec<AemNode> = Vec::new();
    let mut preamble_nodes: Vec<AemNode> = Vec::new();

    for (title, section_nodes) in &sections {
        let converted: Vec<AemNode> = section_nodes
            .iter()
            .filter_map(|n| convert_node(n, config, &mut ctx, config.grid_columns, None))
            .collect();

        if let Some(title) = title {
            // H2 section → wrap in a Panel; look up XSD bindRef if enabled.
            let bind_ref = ctx
                .bind_refs
                .as_ref()
                .and_then(|br| br.sections.get(title.as_str()))
                .cloned();
            let name = ctx.make_name("PN", title);
            let uuid = ctx.uuid(&name);

            // Prepend preamble content to the first H2 section
            let section_children = if children.is_empty() && !preamble_nodes.is_empty() {
                let mut merged = std::mem::take(&mut preamble_nodes);
                merged.extend(converted);
                merged
            } else {
                converted
            };

            children.push(AemNode::Panel {
                uuid,
                name,
                title: title.clone(),
                children: section_children,
                is_page: true,
                dor_exclude: false,
                visible: true,
                is_conditional: false,
                dor_num_cols: None,
                colspan: config.grid_columns,
                dor_colspan: None,
                bind_ref,
            });
        } else {
            // Preamble (before first H2) → collect nodes, don't create page.
            // These will be prepended to the first H2 section.
            preamble_nodes = converted;
        }
    }

    // If there were only preamble nodes and no H2 sections, wrap them in a page.
    if children.is_empty() && !preamble_nodes.is_empty() {
        let name = ctx.make_name("PN", "");
        let uuid = ctx.uuid(&name);
        children.push(AemNode::Panel {
            uuid,
            name,
            title: String::new(),
            children: preamble_nodes,
            is_page: true,
            dor_exclude: false,
            visible: true,
            is_conditional: false,
            dor_num_cols: None,
            colspan: config.grid_columns,
            dor_colspan: None,
            bind_ref: None,
        });
    }

    // Place footnotes on the page panels where they are referenced.
    // For each footnote with a marker, scan each page's structured nodes for
    // superscript text matching that marker. Place on the matching page,
    // or fall back to the last page.
    if !footnotes.is_empty() {
        place_footnotes_on_pages(&mut children, &footnotes, &sections, config, &mut ctx);
    }

    inject_page_edge_templates(&mut children, config, &mut ctx);

    // --- Second pass: wire conditions onto trigger fields ---
    let conditions = std::mem::take(&mut ctx.collected_conditions);
    if !conditions.is_empty() {
        wire_conditions(&mut children, &conditions);
    }

    // --- Third pass: replace panels with fragment references ---
    if config.use_fragments && !config.fragments.is_empty() {
        let xsd_config = config.xsd_config.as_ref();
        replace_with_fragments(&mut children, &config.fragments, xsd_config, &mut ctx);
    }

    // --- Fourth pass: propagate bind_ref from section panels to repeatables ---
    // In reference forms, the repeatable inner panel (with maxOccur) carries the
    // bindRef, not the wrapping section panel.  Move the section panel's bind_ref
    // to its child Repeatable when applicable.
    propagate_bind_ref_to_repeatables(&mut children);

    // When bind_to_xsd is disabled, strip bind_ref from all non-Fragment nodes
    // (bind_refs were only needed internally for fragment matching).
    if !config.bind_to_xsd {
        strip_bind_refs(&mut children);
    }

    AemNode::Root {
        title: form_display_title,
        children,
    }
}

fn inject_page_edge_templates(
    children: &mut [AemNode],
    config: &AemConfig,
    ctx: &mut ConversionContext,
) {
    let insert_preface = config.component_templates.contains_key("preface");
    let insert_appendix = config.component_templates.contains_key("appendix");
    if !insert_preface && !insert_appendix {
        return;
    }

    let page_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| match node {
            AemNode::Panel { is_page: true, .. } => Some(idx),
            _ => None,
        })
        .collect();

    let Some(first_page_idx) = page_indices.first().copied() else {
        return;
    };
    let last_page_idx = page_indices.last().copied().unwrap_or(first_page_idx);

    if insert_preface {
        let name = ctx.make_name("PRF", "Preface");
        let uuid = ctx.uuid(&name);
        let preface = AemNode::Preface { uuid, name };
        if let AemNode::Panel {
            children: page_children,
            ..
        } = &mut children[first_page_idx]
        {
            page_children.insert(0, preface);
        }
    }

    if insert_appendix {
        let name = ctx.make_name("APX", "Appendix");
        let uuid = ctx.uuid(&name);
        let appendix = AemNode::Appendix { uuid, name };
        if let AemNode::Panel {
            children: page_children,
            ..
        } = &mut children[last_page_idx]
        {
            page_children.push(appendix);
        }
    }
}

// ============================================================================
// Footnote placement
// ============================================================================

/// Recursively collect all footnote nodes from the structured tree.
fn collect_all_footnotes<'a>(nodes: &'a [StructuredNode], out: &mut Vec<&'a FootnoteNode>) {
    for node in nodes {
        match node {
            StructuredNode::Footnote(f) => out.push(f),
            StructuredNode::Group(g) => collect_all_footnotes(&g.children, out),
            StructuredNode::Conditional(c) => {
                collect_all_footnotes(std::slice::from_ref(c.content.as_ref()), out);
            }
            _ => {}
        }
    }
}

/// Place footnotes on the AEM page panels where they are referenced.
///
/// For each footnote with a marker (e.g. "1"), searches the structured nodes
/// in each H2-page section for a `Superscript` inline node containing that
/// marker text. The footnote is placed at the end of the matching page panel.
/// Footnotes without a match are placed on the last page.
fn place_footnotes_on_pages(
    children: &mut [AemNode],
    footnotes: &[&FootnoteNode],
    sections: &[(Option<String>, Vec<&StructuredNode>)],
    config: &AemConfig,
    ctx: &mut ConversionContext,
) {
    if footnotes.is_empty() {
        return;
    }

    // Build page indices (panels with is_page: true)
    let page_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| match node {
            AemNode::Panel { is_page: true, .. } => Some(idx),
            _ => None,
        })
        .collect();

    if page_indices.is_empty() {
        return;
    }

    let last_page_idx = *page_indices.last().unwrap();

    // Map sections to page indices. Sections with a title (H2) map 1:1 to pages.
    // The preamble (no title) is merged into the first page.
    let mut section_to_page: Vec<usize> = Vec::new();
    let mut page_counter = 0usize;
    for (title, _) in sections {
        if title.is_some() {
            if page_counter < page_indices.len() {
                section_to_page.push(page_indices[page_counter]);
                page_counter += 1;
            }
        } else {
            // Preamble → first page
            section_to_page.push(page_indices[0]);
        }
    }

    for footnote in footnotes {
        let target_page = if let Some(marker) = &footnote.marker {
            // Search each section's nodes for a superscript reference with this marker
            let mut found_page = None;
            for (section_idx, (_, section_nodes)) in sections.iter().enumerate() {
                if section_nodes
                    .iter()
                    .any(|n| node_contains_superscript_marker(n, marker))
                {
                    if let Some(&page_idx) = section_to_page.get(section_idx) {
                        found_page = Some(page_idx);
                        break;
                    }
                }
            }
            found_page.unwrap_or(last_page_idx)
        } else {
            last_page_idx
        };

        // Convert the footnote to an AemNode
        let plain = inline_text_to_html(&footnote.content, &ctx.language);
        let content = format!("<p>{plain}</p>");
        let source_text = footnote.content.plain_text_in(&ctx.language);
        let name = ctx.make_name("FN", &source_text);
        let uuid = ctx.uuid(&name);
        let aem_footnote = AemNode::Footnote {
            uuid,
            name,
            content,
            colspan: config.grid_columns,
            dor_colspan: None,
        };

        // Place on the target page panel
        if let AemNode::Panel {
            children: page_children,
            ..
        } = &mut children[target_page]
        {
            page_children.push(aem_footnote);
        }
    }
}

/// Check if a `StructuredNode` (or its descendants) contains a `Superscript`
/// inline node whose text matches the given marker.
fn node_contains_superscript_marker(node: &StructuredNode, marker: &str) -> bool {
    match node {
        StructuredNode::Heading(h) => inline_text_has_superscript_marker(&h.content, marker),
        StructuredNode::Paragraph(p) => inline_text_has_superscript_marker(&p.content, marker),
        StructuredNode::Group(g) => g
            .children
            .iter()
            .any(|c| node_contains_superscript_marker(c, marker)),
        StructuredNode::Conditional(c) => node_contains_superscript_marker(&c.content, marker),
        StructuredNode::GridLayout(gl) => gl
            .elements
            .iter()
            .any(|e| node_contains_superscript_marker(&e.node, marker)),
        StructuredNode::Repeatable(r) => node_contains_superscript_marker(&r.item, marker),
        StructuredNode::Table(t) => {
            let header_match = t
                .header
                .as_ref()
                .map(|h| {
                    h.cells
                        .iter()
                        .any(|c| node_contains_superscript_marker(c, marker))
                })
                .unwrap_or(false);
            header_match
                || t.rows.iter().any(|r| {
                    r.cells
                        .iter()
                        .any(|c| node_contains_superscript_marker(c, marker))
                })
        }
        StructuredNode::List(l) => l
            .items
            .iter()
            .any(|item| inline_text_has_superscript_marker(&item.content, marker)),
        StructuredNode::Field(f) => f
            .label
            .as_ref()
            .map(|l| inline_text_has_superscript_marker(l, marker))
            .unwrap_or(false),
        _ => false,
    }
}

/// Check if an `InlineText` contains a `Superscript` node whose plain text matches `marker`.
fn inline_text_has_superscript_marker(text: &InlineText, marker: &str) -> bool {
    text.0
        .iter()
        .any(|node| inline_node_has_superscript_marker(node, marker))
}

/// Recursively check if an `InlineNode` is or contains a `Superscript` whose text matches `marker`.
fn inline_node_has_superscript_marker(node: &InlineNode, marker: &str) -> bool {
    match node {
        InlineNode::Superscript(inner) => {
            // Check if the inner text matches the marker
            if let Some(text) = inner.leading_text() {
                text.trim() == marker
            } else {
                false
            }
        }
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            inline_node_has_superscript_marker(inner, marker)
        }
        InlineNode::Link(link) => inline_text_has_superscript_marker(&link.content, marker),
        _ => false,
    }
}

// ============================================================================
// Node dispatch
// ============================================================================

/// Convert a single `StructuredNode` into an `AemNode` (or `None` for empty /
/// skipped nodes).
fn convert_node(
    node: &StructuredNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> Option<AemNode> {
    match node {
        StructuredNode::Heading(h) if matches!(h.level, HeadingLevel::H1 | HeadingLevel::H2) => {
            // H1 is used as the form title; H2 is used for section panel titles.
            // Neither should produce an inline TextDraw.
            None
        }
        StructuredNode::Heading(h) => Some(convert_heading(h, config, ctx, colspan, dor_colspan)),
        StructuredNode::Paragraph(p) => {
            Some(convert_paragraph(p, config, ctx, colspan, dor_colspan))
        }
        StructuredNode::Image(img) => Some(convert_image(img, config, ctx, colspan, dor_colspan)),
        StructuredNode::Table(t) => Some(convert_table(t, config, ctx, colspan, dor_colspan)),
        StructuredNode::Field(f) => Some(convert_field(f, config, ctx, colspan, dor_colspan)),
        StructuredNode::Repeatable(r) => {
            Some(convert_repeatable(r, config, ctx, colspan, dor_colspan))
        }
        StructuredNode::Group(g) => Some(convert_group(g, config, ctx, colspan, dor_colspan)),
        StructuredNode::Conditional(c) => {
            Some(convert_conditional(c, config, ctx, colspan, dor_colspan))
        }
        StructuredNode::GridLayout(gl) => {
            Some(convert_grid_layout(gl, config, ctx, colspan, dor_colspan))
        }
        StructuredNode::List(l) => Some(convert_list(l, config, ctx, colspan, dor_colspan)),
        StructuredNode::Empty => None,
        StructuredNode::Footnote(_) => None,
    }
}

// ============================================================================
// Per-variant converters
// ============================================================================

fn convert_heading(
    h: &HeadingNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let plain = inline_text_to_html(&h.content, &ctx.language);
    let content = format!("<p>{plain}</p>");
    let source_text = h.content.plain_text_in(&ctx.language);
    let name = ctx.make_name("TTL", &source_text);
    let uuid = ctx.uuid(&name);
    AemNode::TitleDraw {
        uuid,
        name,
        content,
        heading_level: 4,
        colspan,
        dor_colspan,
    }
}

fn convert_paragraph(
    p: &ParagraphNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let html = inline_text_to_html(&p.content, &ctx.language);
    let content = format!("<p>{html}</p>");
    let source_text = p.content.plain_text_in(&ctx.language);
    let name = ctx.make_name("ST", &source_text);
    let uuid = ctx.uuid(&name);
    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: false,
        colspan,
        dor_colspan,
    }
}

pub(crate) fn render_list_html(list: &ListNode, lang: &str) -> String {
    let tag = if list.list_style.is_ordered() {
        "ol"
    } else {
        "ul"
    };
    let style_attr = if list.list_style.needs_css() {
        format!(
            " style=\"list-style-type: {};\"",
            list.list_style.css_value()
        )
    } else {
        String::new()
    };
    let items_html: String = list
        .items
        .iter()
        .map(|item| {
            let html = inline_text_to_html(&item.content, lang);
            let sub_html = item
                .sublist
                .as_ref()
                .map(|sub| render_list_html(sub, lang))
                .unwrap_or_default();
            format!("<li>{html}{sub_html}</li>")
        })
        .collect();
    format!("<{tag}{style_attr}>{items_html}</{tag}>")
}

fn convert_list(
    list: &ListNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let content = render_list_html(list, &ctx.language);
    let first_item_text = list
        .items
        .first()
        .map(|i| i.content.plain_text_in(&ctx.language))
        .unwrap_or_default();
    let name = ctx.make_name("ST", &first_item_text);
    let uuid = ctx.uuid(&name);
    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: false,
        colspan,
        dor_colspan,
    }
}

fn convert_image(
    img: &ImageNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let alt = img.alt_text.as_deref().unwrap_or("image");
    let content = if img.content.is_empty() {
        format!("<p>[Image: {alt}]</p>")
    } else {
        let b64 = base64_encode(&img.content);
        format!("<p><img src=\"data:image/png;base64,{b64}\" alt=\"{alt}\" /></p>")
    };
    let name = ctx.make_name("IMG", alt);
    let uuid = ctx.uuid(&name);
    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: false,
        colspan,
        dor_colspan,
    }
}

fn convert_table(
    table: &TableNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    _colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    // Convert table to a simple panel with all cells as direct children (paragraphs).
    // AEM doesn't support tables, so we just output cells linearly.
    let caption_text = table
        .caption
        .as_ref()
        .map(|c| c.plain_text_in(&ctx.language))
        .unwrap_or_default();
    let name = ctx.make_name("TBL", &caption_text);
    let uuid = ctx.uuid(&name);
    let title = table
        .caption
        .as_ref()
        .map(|c| inline_text_to_html(c, &ctx.language))
        .unwrap_or_default();

    let mut children = Vec::new();

    // Header cells
    if let Some(header) = &table.header {
        for cell in &header.cells {
            if let Some(node) = convert_node(cell, config, ctx, config.grid_columns, None) {
                children.push(node);
            }
        }
    }

    // Body cells
    for row in &table.rows {
        for cell in &row.cells {
            if let Some(node) = convert_node(cell, config, ctx, config.grid_columns, None) {
                children.push(node);
            }
        }
    }

    AemNode::Panel {
        uuid,
        name,
        title,
        children,
        is_page: false,
        dor_exclude: false,
        visible: true,
        is_conditional: false,
        dor_num_cols: None,
        colspan: config.grid_columns,
        dor_colspan,
        bind_ref: None,
    }
}

fn convert_field(
    f: &FieldNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let label = f
        .label
        .as_ref()
        .map(|l| inline_text_to_html(l, &ctx.language))
        .unwrap_or_default();

    // Extract source text for the name heuristic:
    // 1. Field label (if available)
    // 2. Last SOM path segment (if available)
    // 3. Empty string (fallback)
    let source_text = f
        .label
        .as_ref()
        .map(|l| l.plain_text_in(&ctx.language))
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            f.som_path
                .as_ref()
                .and_then(|p| p.as_str().rsplit('.').next())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    // Look up the XSD bindRef path for this field when bind_to_xsd is enabled.
    let bind_ref: Option<String> = ctx
        .bind_refs
        .as_ref()
        .and_then(|br| br.fields.get(&f.name))
        .cloned();

    match &f.input_type {
        FieldType::Text { max_length, .. } => {
            let name = ctx.make_name("TXT", &source_text);
            let uuid = ctx.uuid(&name);
            AemNode::TextField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                max_chars: *max_length,
                colspan,
                dor_colspan,
                bind_ref,
            }
        }

        FieldType::Textarea { max_length } => {
            let name = ctx.make_name("TXT", &source_text);
            let uuid = ctx.uuid(&name);
            AemNode::TextField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                max_chars: *max_length,
                colspan,
                dor_colspan,
                bind_ref,
            }
        }

        FieldType::Number { .. } => {
            let name = ctx.make_name("NB", &source_text);
            let uuid = ctx.uuid(&name);
            AemNode::NumberField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                colspan,
                dor_colspan,
                bind_ref,
            }
        }

        FieldType::Date => {
            let name = ctx.make_name("DATE", &source_text);
            let uuid = ctx.uuid(&name);
            AemNode::DatePicker {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                colspan,
                dor_colspan,
                bind_ref,
            }
        }

        FieldType::Email => {
            let name = ctx.make_name("EML", &source_text);
            let uuid = ctx.uuid(&name);
            AemNode::TextField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                max_chars: None,
                colspan,
                dor_colspan,
                bind_ref,
            }
        }

        FieldType::Tel => {
            let name = ctx.make_name("TEL", &source_text);
            let uuid = ctx.uuid(&name);
            AemNode::TextField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                max_chars: None,
                colspan,
                dor_colspan,
                bind_ref,
            }
        }

        FieldType::Bool => {
            let name = ctx.make_name("CB", &source_text);
            let uuid = ctx.uuid(&name);
            let option_label = label.clone();
            AemNode::Checkbox {
                uuid,
                name,
                options: vec![AemOption {
                    label: option_label,
                    value: "true".into(),
                }],
                alignment: OptionAlignment::Horizontal,
                visible: true,
                colspan,
                dor_colspan,
                field_id: Some(f.name.clone()),
                conditions: Vec::new(),
                bind_ref,
            }
        }

        FieldType::Radio { options } => {
            let name = ctx.make_name("RB", &source_text);
            let uuid = ctx.uuid(&name);
            let aem_options = convert_name_values(options, &ctx.language);
            AemNode::RadioButton {
                uuid,
                name,
                label,
                options: aem_options,
                alignment: OptionAlignment::Vertical,
                mandatory: true,
                visible: true,
                colspan,
                dor_colspan,
                field_id: Some(f.name.clone()),
                conditions: Vec::new(),
                bind_ref,
            }
        }

        FieldType::Select { options } => {
            let name = ctx.make_name("DD", &source_text);
            let uuid = ctx.uuid(&name);
            let aem_options = convert_name_values(options, &ctx.language);
            AemNode::Dropdown {
                uuid,
                name,
                label,
                options: aem_options,
                mandatory: false,
                visible: true,
                colspan,
                dor_colspan,
                field_id: Some(f.name.clone()),
                conditions: Vec::new(),
                bind_ref,
            }
        }
    }
}

fn convert_repeatable(
    r: &RepeatableNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    _colspan: u32,
    _dor_colspan: Option<u32>,
) -> AemNode {
    let name = ctx.make_name("RP", "");
    let uuid = ctx.uuid(&name);
    let inner = convert_node(&r.item, config, ctx, config.grid_columns, None);
    let children = inner.into_iter().collect();
    AemNode::Repeatable {
        uuid,
        name: name.clone(),
        title: name,
        children,
        min_occur: r.min_occurrences,
        max_occur: r.max_occurrences.unwrap_or(config.repeatable_max_occur),
        bind_ref: None,
    }
}

fn convert_group(
    g: &GroupNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let name = ctx.make_name("PN", "");
    let uuid = ctx.uuid(&name);
    let children: Vec<AemNode> = g
        .children
        .iter()
        .filter_map(|n| convert_node(n, config, ctx, config.grid_columns, None))
        .collect();
    AemNode::Panel {
        uuid,
        name,
        title: String::new(),
        children,
        is_page: false,
        dor_exclude: false,
        visible: true,
        is_conditional: false,
        dor_num_cols: None,
        colspan,
        dor_colspan,
        bind_ref: None,
    }
}

fn convert_conditional(
    c: &ConditionalNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let name = ctx.make_name("PN", "");
    let uuid = ctx.uuid(&name);
    let inner = convert_node(&c.content, config, ctx, config.grid_columns, None);
    let children: Vec<AemNode> = inner.into_iter().collect();

    // Look up the field label, falling back to the UUID
    let field_label = ctx
        .field_labels
        .get(&c.condition.field_name)
        .cloned()
        .unwrap_or_else(|| c.condition.field_name.to_string());
    let title = format!(
        "Condition: {} = {}",
        field_label,
        format_input_value(&c.condition.value)
    );

    // Record the condition so the second pass can wire it to the trigger field.
    ctx.collected_conditions.push(CollectedCondition {
        field_id: c.condition.field_name.clone(),
        panel_name: name.clone(),
        value: c.condition.value.clone(),
    });

    // Conditional panels start hidden; the trigger field’s valueCommit script
    // will show them when the value matches.
    AemNode::Panel {
        uuid,
        name,
        title,
        children,
        is_page: false,
        dor_exclude: true,
        visible: false,
        is_conditional: true,
        dor_num_cols: None,
        colspan,
        dor_colspan,
        bind_ref: None,
    }
}

fn convert_grid_layout(
    gl: &GridLayout,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
    dor_colspan: Option<u32>,
) -> AemNode {
    let name = ctx.make_name("PN", "");
    let uuid = ctx.uuid(&name);
    let total = gl.columns as u32;
    let children: Vec<AemNode> = gl
        .elements
        .iter()
        .filter_map(|elem| {
            let col_span = (elem.span as u32 * config.grid_columns) / total;
            let dor_col_span = Some(elem.span as u32);
            convert_node(&elem.node, config, ctx, col_span.max(1), dor_col_span)
        })
        .collect();
    AemNode::Panel {
        uuid,
        name,
        title: String::new(),
        children,
        is_page: false,
        dor_exclude: false,
        visible: true,
        is_conditional: false,
        dor_num_cols: Some(gl.columns as u32),
        colspan,
        dor_colspan,
        bind_ref: None,
    }
}

// ============================================================================
// Two-pass condition wiring
// ============================================================================

/// Wire collected conditions onto their trigger field nodes.
///
/// Groups conditions by `FieldId`, then walks the tree to find the trigger
/// field (RadioButton, Checkbox, or Dropdown) whose `field_id` matches.
/// Each matching condition becomes one `ConditionRule` on the trigger.
fn wire_conditions(nodes: &mut [AemNode], conditions: &[CollectedCondition]) {
    use std::collections::HashMap;

    // Group conditions by trigger FieldId.
    let mut by_field: HashMap<&FieldId, Vec<&CollectedCondition>> = HashMap::new();
    for cond in conditions {
        by_field.entry(&cond.field_id).or_default().push(cond);
    }

    if by_field.is_empty() {
        return;
    }

    // Walk the tree and annotate matching trigger fields.
    for node in nodes.iter_mut() {
        wire_conditions_recursive(node, &by_field);
    }
}

fn wire_conditions_recursive(
    node: &mut AemNode,
    by_field: &std::collections::HashMap<&FieldId, Vec<&CollectedCondition>>,
) {
    match node {
        AemNode::RadioButton {
            field_id,
            conditions,
            ..
        }
        | AemNode::Checkbox {
            field_id,
            conditions,
            ..
        }
        | AemNode::Dropdown {
            field_id,
            conditions,
            ..
        } => {
            if let Some(fid) = field_id {
                if let Some(conds) = by_field.get(fid) {
                    for c in conds {
                        conditions.push(ConditionRule {
                            target_panel_name: c.panel_name.clone(),
                            value: c.value.clone(),
                            show: true,
                        });
                    }
                }
            }
        }
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => {
            for child in children.iter_mut() {
                wire_conditions_recursive(child, by_field);
            }
        }
        _ => {}
    }
}

// ============================================================================
// Fragment replacement
// ============================================================================

/// Collect leaf element names from `bindRef` values of a node's children.
///
/// For each child that has a `bindRef`, extract the last path segment
/// (the element name). E.g. `/form/section/Street` → `"Street"`.
fn collect_child_bind_ref_leaves(children: &[AemNode]) -> Vec<String> {
    let mut leaves = Vec::new();
    for child in children {
        let bind_ref = match child {
            AemNode::TextField { bind_ref, .. }
            | AemNode::NumberField { bind_ref, .. }
            | AemNode::DatePicker { bind_ref, .. }
            | AemNode::Dropdown { bind_ref, .. }
            | AemNode::Checkbox { bind_ref, .. }
            | AemNode::RadioButton { bind_ref, .. } => bind_ref.as_deref(),
            AemNode::Panel {
                bind_ref,
                children: sub_children,
                ..
            } => {
                // Recurse into sub-panels to collect their leaves too
                leaves.extend(collect_child_bind_ref_leaves(sub_children));
                bind_ref.as_deref()
            }
            AemNode::Repeatable {
                children: sub_children,
                ..
            } => {
                // Recurse into repeatable items to collect their leaves too
                leaves.extend(collect_child_bind_ref_leaves(sub_children));
                None
            }
            _ => None,
        };
        if let Some(br) = bind_ref {
            if let Some(leaf) = br.rsplit('/').next() {
                if !leaf.is_empty() {
                    leaves.push(leaf.to_string());
                }
            }
        }
    }
    leaves
}

/// Check whether all panel leaf element names are contained in the
/// fragment's bound elements (i.e. panel leaves ⊆ fragment elements).
fn panel_leaves_subset_of_fragment(fragment: &ParsedFragment, panel_leaves: &[String]) -> bool {
    panel_leaves
        .iter()
        .all(|leaf| fragment.bound_elements.iter().any(|elem| elem == leaf))
}

/// Find the best matching fragment for a panel, given its children's bind_ref
/// leaf element names and the XSD config's registered types.
///
/// Matching logic:
/// 1. Collect the panel children's bind_ref leaf element names.
/// 2. For each registered XSD type, check if **all** panel leaves are
///    contained in the type's elements (subset check).
/// 3. Among matching fragments, verify that all panel leaves are also
///    contained in the fragment's bound elements, then pick the most
///    specific fragment.
fn find_best_fragment<'a>(
    panel_leaves: &[String],
    fragments: &'a [ParsedFragment],
    xsd_config: Option<&crate::xsd::XsdConfig>,
) -> Option<&'a ParsedFragment> {
    find_best_fragment_inner(panel_leaves, fragments, xsd_config, true)
}

/// Like [`find_best_fragment`], but when `strict` is false, the second check
/// (panel leaves ⊆ fragment bound_elements) is skipped.  This is used in
/// the multi-instance path where the panel is kept and only some of its
/// children are replaced; unmatched leaves remain as regular children.
fn find_best_fragment_inner<'a>(
    panel_leaves: &[String],
    fragments: &'a [ParsedFragment],
    xsd_config: Option<&crate::xsd::XsdConfig>,
    strict: bool,
) -> Option<&'a ParsedFragment> {
    let xsd_config = xsd_config?;

    // Determine which registered XSD types match this panel's leaf elements.
    // All panel leaves must be contained in the type's elements (subset check).
    let mut matching_types: Vec<&str> = Vec::new();
    for (type_name, reg_type) in &xsd_config.registered_types {
        let type_elements: Vec<&str> = reg_type.elements.iter().map(|e| e.name.as_str()).collect();
        let all_present = panel_leaves
            .iter()
            .all(|l| type_elements.contains(&l.as_str()));
        if all_present {
            matching_types.push(type_name);
        }
    }

    if matching_types.is_empty() {
        return None;
    }

    // Find fragments whose xsd_type_name is in the matching types.
    // In strict mode, also require all panel leaves ⊆ fragment bound_elements.
    let mut best: Option<&ParsedFragment> = None;
    for fragment in fragments {
        if !matching_types.contains(&fragment.xsd_type_name.as_str()) {
            continue;
        }
        if strict && !panel_leaves_subset_of_fragment(fragment, panel_leaves) {
            continue;
        }
        // Prefer the fragment with the most bound elements (most specific).
        if let Some(prev) = best {
            if fragment.bound_elements.len() > prev.bound_elements.len() {
                best = Some(fragment);
            }
        } else {
            best = Some(fragment);
        }
    }

    best
}

/// Collect full `bindRef` paths from a node's children (recursively).
///
/// Unlike `collect_child_bind_ref_leaves` which returns only leaf names,
/// this returns the complete bind_ref strings.
fn collect_child_bind_ref_full_paths(children: &[AemNode]) -> Vec<String> {
    let mut paths = Vec::new();
    for child in children {
        match child {
            AemNode::TextField { bind_ref, .. }
            | AemNode::NumberField { bind_ref, .. }
            | AemNode::DatePicker { bind_ref, .. }
            | AemNode::Dropdown { bind_ref, .. }
            | AemNode::Checkbox { bind_ref, .. }
            | AemNode::RadioButton { bind_ref, .. } => {
                if let Some(br) = bind_ref {
                    paths.push(br.clone());
                }
            }
            AemNode::Panel {
                bind_ref,
                children: sub_children,
                ..
            } => {
                paths.extend(collect_child_bind_ref_full_paths(sub_children));
                if let Some(br) = bind_ref {
                    paths.push(br.clone());
                }
            }
            AemNode::Repeatable {
                children: sub_children,
                ..
            } => {
                paths.extend(collect_child_bind_ref_full_paths(sub_children));
            }
            _ => {}
        }
    }
    paths
}

/// Recursively walk the `AemNode` tree and replace panels whose child fields
/// match a known fragment's XSD type with `AemNode::Fragment` nodes.
///
/// Handles two cases:
/// 1. **Direct match**: A panel's immediate children directly correspond to a
///    single XSD type (bind_ref paths are one level deep relative to the panel).
///    The entire panel is replaced with a Fragment.
/// 2. **Multi-instance match**: A panel's children's bind_ref paths have
///    intermediate segments (e.g. `kunde/Name` and `ubs_europe_se/Name`),
///    indicating multiple type instances. Each child that contributes fields
///    for a matched type is individually replaced.
///
/// Recurses depth-first so inner nodes are processed before parents.
/// Rewrite a bind_ref path from the form-specific root to the fragment
/// prefix when the XSD config provides a distinct `fragmentBindRefPrefix`.
///
/// For example, if the form root is `UBSAF_BBRR` and the fragment prefix is
/// `UBSAF`, then `/UBSAF_BBRR/AccountHolder` becomes `/UBSAF/AccountHolder`.
fn to_fragment_bind_ref(bind_ref: &str, xsd_config: Option<&crate::xsd::XsdConfig>) -> String {
    let Some(cfg) = xsd_config else {
        return bind_ref.to_string();
    };
    let form_root = cfg.root_element_name();
    let frag_prefix = cfg.fragment_bind_ref_prefix();
    if form_root == frag_prefix {
        return bind_ref.to_string();
    }
    let form_root_prefix = format!("/{}/", form_root);
    if let Some(rest) = bind_ref.strip_prefix(&form_root_prefix) {
        format!("/{}/{}", frag_prefix, rest)
    } else if bind_ref == format!("/{}", form_root) {
        format!("/{}", frag_prefix)
    } else {
        bind_ref.to_string()
    }
}

/// Returns true if any node in the slice (or its descendants) is a conditional panel.
fn contains_conditional(nodes: &[AemNode]) -> bool {
    for node in nodes {
        match node {
            AemNode::Panel {
                is_conditional: true,
                ..
            } => return true,
            AemNode::Panel { children, .. }
            | AemNode::Root { children, .. }
            | AemNode::Repeatable { children, .. } => {
                if contains_conditional(children) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn replace_with_fragments(
    nodes: &mut [AemNode],
    fragments: &[ParsedFragment],
    xsd_config: Option<&crate::xsd::XsdConfig>,
    ctx: &mut ConversionContext,
) {
    let mut i = 0;
    while i < nodes.len() {
        // First, recurse into children of container nodes
        match &mut nodes[i] {
            AemNode::Root { children, .. }
            | AemNode::Panel { children, .. }
            | AemNode::Repeatable { children, .. } => {
                replace_with_fragments(children, fragments, xsd_config, ctx);
            }
            _ => {}
        }

        // Only check panels with bind_ref
        if let AemNode::Panel {
            children,
            bind_ref: Some(br),
            is_conditional,
            ..
        } = &nodes[i]
        {
            let is_conditional = *is_conditional;
            let full_paths = collect_child_bind_ref_full_paths(children);
            let br_prefix = format!("{}/", br);

            // Compute relative paths from this panel's bind_ref
            let relative_paths: Vec<&str> = full_paths
                .iter()
                .filter_map(|p| p.strip_prefix(&br_prefix))
                .collect();

            // Check if any relative path has an intermediate segment
            // (e.g. "kunde/Name" has depth 2, "Name" has depth 1)
            let has_intermediates = relative_paths.iter().any(|p| p.contains('/'));

            if !has_intermediates && !is_conditional && !contains_conditional(children) {
                // Direct match: all paths are single-segment → try to replace
                // the whole panel with a fragment.
                let leaves = collect_child_bind_ref_leaves(children);
                if !leaves.is_empty() {
                    if let Some(fragment) = find_best_fragment(&leaves, fragments, xsd_config) {
                        let fragment = fragment.clone();
                        let bind_ref = Some(to_fragment_bind_ref(br, xsd_config));
                        let name = ctx.make_name("PN_affrg", &fragment.dir_name);
                        let uuid = ctx.uuid(&name);
                        nodes[i] = AemNode::Fragment {
                            uuid,
                            name,
                            frag_ref: fragment.frag_ref,
                            bind_ref,
                        };
                    }
                }
            } else if !has_intermediates && !is_conditional && contains_conditional(children) {
                // Direct match but the panel contains conditional children.
                // Replace only the non-conditional children with a fragment,
                // keeping conditional panels in place.
                let leaves = collect_child_bind_ref_leaves(children);
                if !leaves.is_empty() {
                    if let Some(fragment) = find_best_fragment(&leaves, fragments, xsd_config) {
                        let fragment = fragment.clone();
                        if let AemNode::Panel {
                            children,
                            bind_ref: Some(br),
                            ..
                        } = &mut nodes[i]
                        {
                            let bind_ref = Some(to_fragment_bind_ref(br, xsd_config));
                            let frag_name = ctx.make_name("PN_affrg", &fragment.dir_name);
                            let uuid = ctx.uuid(&frag_name);

                            // Keep only conditional panels; replace the rest
                            // with a single fragment node.
                            let conditionals: Vec<AemNode> = children
                                .drain(..)
                                .filter(|c| {
                                    matches!(c, AemNode::Panel { is_conditional: true, .. })
                                        || contains_conditional(std::slice::from_ref(c))
                                })
                                .collect();

                            children.push(AemNode::Fragment {
                                uuid,
                                name: frag_name,
                                frag_ref: fragment.frag_ref,
                                bind_ref,
                            });
                            children.extend(conditionals);
                        }
                    }
                }
            } else {
                // Multi-instance: group children's bind_ref paths by the
                // intermediate parent path (everything between the panel's
                // bind_ref and the leaf element name).  For each group that
                // matches a fragment, remove the children whose fields all
                // belong to that group and insert a Fragment node instead.
                let mut matched =
                    compute_intermediate_matches(&full_paths, &br_prefix, fragments, xsd_config);

                if !matched.is_empty() {
                    // Sort for deterministic output order
                    matched.sort_by(|a, b| a.0.cmp(&b.0));

                    if let AemNode::Panel {
                        children,
                        bind_ref: Some(br),
                        ..
                    } = &mut nodes[i]
                    {
                        // For each matched group, determine which children are
                        // fully covered (all their bind_ref paths belong to
                        // matched groups) and collect Fragment nodes to insert.
                        let matched_prefixes: Vec<String> = matched
                            .iter()
                            .map(|(int_path, _)| format!("{}{}", br_prefix, int_path))
                            .collect();

                        // Identify children to remove: a child is removed when
                        // every one of its bind_ref paths falls under a matched
                        // intermediate prefix (exact match or starts_with prefix + "/").
                        // Never remove conditional panels or panels containing
                        // conditionals — their names are referenced by
                        // visibility scripts.
                        let mut keep = Vec::new();
                        for child in children.drain(..) {
                            let is_cond = matches!(
                                &child,
                                AemNode::Panel {
                                    is_conditional: true,
                                    ..
                                }
                            ) || contains_conditional(std::slice::from_ref(&child));
                            let child_paths =
                                collect_child_bind_ref_full_paths(std::slice::from_ref(&child));
                            let all_covered = !is_cond
                                && !child_paths.is_empty()
                                && child_paths.iter().all(|cp| {
                                    matched_prefixes
                                        .iter()
                                        .any(|mp| cp == mp || cp.starts_with(&format!("{}/", mp)))
                                });
                            if !all_covered {
                                keep.push(child);
                            }
                        }

                        // Insert Fragment nodes for each matched group
                        let mut frag_nodes: Vec<AemNode> = Vec::new();
                        for (int_path, fragment) in &matched {
                            let name = ctx.make_name("PN_affrg", &fragment.dir_name);
                            let uuid = ctx.uuid(&name);
                            let full_path = format!("{}/{}", br, int_path);
                            frag_nodes.push(AemNode::Fragment {
                                uuid,
                                name,
                                frag_ref: fragment.frag_ref.clone(),
                                bind_ref: Some(to_fragment_bind_ref(&full_path, xsd_config)),
                            });
                        }

                        // Rebuild children: kept nodes + new fragment nodes
                        *children = keep;
                        children.extend(frag_nodes);
                    }
                }
            }
        }

        i += 1;
    }
}

/// For a panel whose children's bind_ref paths have intermediate segments,
/// group by the full intermediate parent path (all segments between the
/// panel's bind_ref and the leaf) and find matching fragments for each group.
///
/// Returns a vec of `(intermediate_path, fragment)` for each matched group.
fn compute_intermediate_matches(
    full_paths: &[String],
    br_prefix: &str,
    fragments: &[ParsedFragment],
    xsd_config: Option<&crate::xsd::XsdConfig>,
) -> Vec<(String, ParsedFragment)> {
    // Group leaf element names by their full intermediate path.
    // E.g. for relative path "authorized_representative_s/IndividualBasic/LastName",
    // the intermediate path is "authorized_representative_s/IndividualBasic"
    // and the leaf is "LastName".
    let mut groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for fp in full_paths {
        if let Some(relative) = fp.strip_prefix(br_prefix) {
            if let Some((parent, leaf)) = relative.rsplit_once('/') {
                // Only consider paths with at least one intermediate segment
                if !parent.is_empty() {
                    groups
                        .entry(parent.to_string())
                        .or_default()
                        .push(leaf.to_string());
                }
            }
        }
    }

    // Try to match each group's leaves to a fragment (non-strict: the
    // fragment doesn't need to cover all leaves, just match the XSD type)
    let mut matched = Vec::new();
    for (int_path, leaves) in &groups {
        if let Some(fragment) = find_best_fragment_inner(leaves, fragments, xsd_config, false) {
            matched.push((int_path.clone(), fragment.clone()));
        }
    }

    matched
}

/// Move `bind_ref` from section Panels to their child Repeatable nodes.
///
/// In reference AEM forms, the repeatable inner panel (with `maxOccur`)
/// carries the `bindRef` — not the wrapping section panel.  This function
/// walks the tree and, for any Panel that has a `bind_ref` and contains a
/// `Repeatable` child, moves the bind_ref to the Repeatable.
fn propagate_bind_ref_to_repeatables(nodes: &mut [AemNode]) {
    for node in nodes.iter_mut() {
        match node {
            AemNode::Panel {
                children, bind_ref, ..
            } => {
                // First recurse into children
                propagate_bind_ref_to_repeatables(children);

                // If this panel has a bind_ref and contains a Repeatable child,
                // move the bind_ref to the Repeatable (the section panel itself
                // should not emit bindRef — the repeatable inner panel owns it).
                if bind_ref.is_some() {
                    let has_repeatable = children
                        .iter()
                        .any(|c| matches!(c, AemNode::Repeatable { .. }));
                    if has_repeatable {
                        let br = bind_ref.take().unwrap();
                        for child in children.iter_mut() {
                            if let AemNode::Repeatable {
                                bind_ref: rep_br, ..
                            } = child
                            {
                                if rep_br.is_none() {
                                    *rep_br = Some(br.clone());
                                }
                            }
                        }
                    }
                }
            }
            AemNode::Root { children, .. } | AemNode::Repeatable { children, .. } => {
                propagate_bind_ref_to_repeatables(children);
            }
            _ => {}
        }
    }
}

/// Recursively clear `bind_ref` on all nodes except `Fragment` nodes.
///
/// Used when `use_fragments` is enabled but `bind_to_xsd` is disabled:
/// bind-refs were computed internally for fragment matching but should not
/// appear in the final AEM output.
fn strip_bind_refs(nodes: &mut [AemNode]) {
    for node in nodes.iter_mut() {
        match node {
            AemNode::Fragment { .. } => {
                // Keep bind_ref on fragments — it is the data bind path.
            }
            AemNode::Root { children, .. } => {
                strip_bind_refs(children);
            }
            AemNode::Repeatable {
                children, bind_ref, ..
            } => {
                *bind_ref = None;
                strip_bind_refs(children);
            }
            AemNode::Panel {
                children, bind_ref, ..
            } => {
                *bind_ref = None;
                strip_bind_refs(children);
            }
            AemNode::TextField { bind_ref, .. }
            | AemNode::NumberField { bind_ref, .. }
            | AemNode::DatePicker { bind_ref, .. }
            | AemNode::Dropdown { bind_ref, .. }
            | AemNode::Checkbox { bind_ref, .. }
            | AemNode::RadioButton { bind_ref, .. } => {
                *bind_ref = None;
            }
            AemNode::TextDraw { .. }
            | AemNode::TitleDraw { .. }
            | AemNode::Preface { .. }
            | AemNode::Appendix { .. }
            | AemNode::Footnote { .. } => {}
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Convert `InlineText` to a simple HTML string.
pub(crate) fn inline_text_to_html(text: &InlineText, language: &str) -> String {
    let mut out = String::new();
    for node in &text.0 {
        inline_node_to_html(node, language, &mut out);
    }
    out
}

fn inline_node_to_html(node: &InlineNode, language: &str, out: &mut String) {
    match node {
        InlineNode::Text(s) => {
            out.push_str(&escape_html(s));
        }
        InlineNode::TranslatedText(map) => {
            let text = map
                .get(language)
                .and_then(|o| o.as_deref())
                .or_else(|| map.values().find_map(|o| o.as_deref()))
                .unwrap_or("");
            out.push_str(&escape_html(text));
        }
        InlineNode::Link(link) => {
            out.push_str("<a href=\"");
            out.push_str(&escape_html(&link.href));
            out.push_str("\">");
            for child in &link.content.0 {
                inline_node_to_html(child, language, out);
            }
            out.push_str("</a>");
        }
        InlineNode::Strong(inner) => {
            out.push_str("<b>");
            inline_node_to_html(inner, language, out);
            out.push_str("</b>");
        }
        InlineNode::Emphasis(inner) => {
            out.push_str("<i>");
            inline_node_to_html(inner, language, out);
            out.push_str("</i>");
        }
        InlineNode::Superscript(inner) => {
            out.push_str("<sup>");
            inline_node_to_html(inner, language, out);
            out.push_str("</sup>");
        }
    }
}

use crate::util::{base64_encode, escape_html};

fn convert_name_values(options: &[NameValue], language: &str) -> Vec<AemOption> {
    options
        .iter()
        .map(|nv| {
            let label = match &nv.name {
                TranslatableString::Plain(s) => s.clone(),
                TranslatableString::Translated(map) => map
                    .get(language)
                    .and_then(|o| o.clone())
                    .or_else(|| map.values().find_map(|o| o.clone()))
                    .unwrap_or_default(),
            };
            let value = format_input_value(&nv.value);
            AemOption { label, value }
        })
        .collect()
}

/// Collect field labels from the structured nodes into a map.
fn collect_field_labels(
    nodes: &[StructuredNode],
    language: &str,
    labels: &mut HashMap<FieldId, String>,
) {
    for node in nodes {
        match node {
            StructuredNode::Field(f) => {
                if let Some(label) = &f.label {
                    let label_text = label.plain_text_in(language);
                    if !label_text.is_empty() {
                        labels.insert(f.name.clone(), label_text.trim().to_string());
                    }
                }
            }
            StructuredNode::Group(g) => collect_field_labels(&g.children, language, labels),
            StructuredNode::Table(t) => {
                if let Some(header) = &t.header {
                    collect_field_labels(&header.cells, language, labels);
                }
                for row in &t.rows {
                    collect_field_labels(&row.cells, language, labels);
                }
            }
            StructuredNode::Repeatable(r) => {
                collect_field_labels(&[(*r.item).clone()], language, labels);
            }
            StructuredNode::Conditional(c) => {
                collect_field_labels(&[(*c.content).clone()], language, labels);
            }
            StructuredNode::GridLayout(g) => {
                for elem in &g.elements {
                    collect_field_labels(std::slice::from_ref(&elem.node), language, labels);
                }
            }
            _ => {}
        }
    }
}

fn format_input_value(v: &InputValue) -> String {
    match v {
        InputValue::Text(s) => s.clone(),
        InputValue::Number(n) => n.to_string(),
        InputValue::Bool(b) => b.to_string(),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::*;

    fn default_config() -> AemConfig {
        let mut config = AemConfig::test_default("TEST");
        config.deterministic_uuids = true;
        config
    }

    /// Assert that an AEM name matches the pattern `PREFIX_CamelCase_hexhexhex`
    /// or `PREFIX_hexhexhex` (when no CamelCase part is available).
    fn assert_name_pattern(name: &str, expected_prefix: &str, expected_camel_contains: &str) {
        assert!(
            name.starts_with(&format!("{}_", expected_prefix)),
            "Name '{}' should start with '{}_'",
            name,
            expected_prefix
        );
        // Last segment (after final _) must be an 8-char hex string
        let last_underscore = name.rfind('_').expect("Name should contain _");
        let suffix = &name[last_underscore + 1..];
        assert_eq!(
            suffix.len(),
            8,
            "Short UUID suffix '{}' should be 8 chars in '{}'",
            suffix,
            name
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "Suffix '{}' should be hex digits in '{}'",
            suffix,
            name
        );
        if !expected_camel_contains.is_empty() {
            assert!(
                name.contains(expected_camel_contains),
                "Name '{}' should contain '{}'",
                name,
                expected_camel_contains
            );
        }
    }

    /// Helper: extract the children from the single preamble panel under Root.
    /// When there are no H2 headings, all content is wrapped in one preamble panel.
    fn unwrap_preamble(root: &AemNode) -> &[AemNode] {
        match root {
            AemNode::Root { children, .. } => {
                assert_eq!(children.len(), 1, "Expected single preamble panel");
                match &children[0] {
                    AemNode::Panel {
                        children, title, ..
                    } => {
                        assert!(title.is_empty(), "Preamble panel should have empty title");
                        children
                    }
                    other => panic!("Expected Panel, got {:?}", other),
                }
            }
            other => panic!("Expected Root, got {:?}", other),
        }
    }

    #[test]
    fn convert_heading_produces_titledraw() {
        // H3 headings are NOT used for sectioning — they become titledraws
        let nodes = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H3,
            content: InlineText::plain("Sub Title"),
            som_path: None,
            source_name: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::TitleDraw {
                content,
                heading_level,
                name,
                ..
            } => {
                assert!(content.contains("<p>"), "content should use <p> not <h3>");
                assert!(!content.contains("<h3>"), "content must not contain <h3>");
                assert!(content.contains("Sub Title"));
                assert_eq!(*heading_level, 4);
                assert!(name.starts_with("TTL_"), "name should start with TTL_");
            }
            other => panic!("Expected TitleDraw, got {:?}", other),
        }
    }

    #[test]
    fn convert_unordered_list_produces_textdraw() {
        let nodes = vec![StructuredNode::List(ListNode {
            list_style: crate::document::ListStyleType::Disc,
            items: vec![
                ListItem::simple(InlineText::plain("First item")),
                ListItem::simple(InlineText::plain("Second item")),
            ],
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::TextDraw { content, .. } => {
                assert!(content.contains("<ul>"));
                assert!(content.contains("</ul>"));
                assert!(content.contains("<li>First item</li>"));
                assert!(content.contains("<li>Second item</li>"));
                assert!(!content.contains("<ol>"));
            }
            other => panic!("Expected TextDraw, got {:?}", other),
        }
    }

    #[test]
    fn convert_ordered_list_produces_textdraw() {
        let nodes = vec![StructuredNode::List(ListNode {
            list_style: crate::document::ListStyleType::Decimal,
            items: vec![
                ListItem::simple(InlineText::plain("Step one")),
                ListItem::simple(InlineText::plain("Step two")),
            ],
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::TextDraw { content, .. } => {
                assert!(content.contains("<ol>"));
                assert!(content.contains("</ol>"));
                assert!(content.contains("<li>Step one</li>"));
                assert!(content.contains("<li>Step two</li>"));
                assert!(!content.contains("<ul>"));
            }
            other => panic!("Expected TextDraw, got {:?}", other),
        }
    }

    #[test]
    fn h2_headings_create_section_panels() {
        // H2 headings split the top-level children into sub-panels.
        // Content before the first H2 is also wrapped in a panel.
        let nodes = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Preamble"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H2,
                content: InlineText::plain("Section A"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Field(FieldNode {
                name: "fieldA".into(),
                som_path: None,
                label: Some(InlineText::plain("Field A")),
                input_type: FieldType::Text {
                    regex: None,
                    max_length: None,
                    min_length: None,
                },
                value: None,
                placeholder: None,
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H2,
                content: InlineText::plain("Section B"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Field(FieldNode {
                name: "fieldB".into(),
                som_path: None,
                label: Some(InlineText::plain("Field B")),
                input_type: FieldType::Text {
                    regex: None,
                    max_length: None,
                    min_length: None,
                },
                value: None,
                placeholder: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Footer text"),
                som_path: None,
                source_name: None,
            }),
        ];
        let root = convert_to_aem(&nodes, &default_config());
        match &root {
            AemNode::Root { children, .. } => {
                // Preamble merged into first section: Panel "Section A" + Panel "Section B"
                assert_eq!(
                    children.len(),
                    2,
                    "Expected 2 root children: 2 section panels (preamble merged into first)"
                );

                // First child: Panel for Section A (preamble TextDraw + fieldA)
                match &children[0] {
                    AemNode::Panel {
                        title,
                        children: panel_children,
                        ..
                    } => {
                        assert_eq!(title, "Section A");
                        // preamble paragraph + fieldA
                        assert_eq!(panel_children.len(), 2);
                        assert!(matches!(&panel_children[0], AemNode::TextDraw { .. }));
                        assert!(matches!(&panel_children[1], AemNode::TextField { .. }));
                    }
                    other => panic!("Expected Panel for Section A, got {:?}", other),
                }

                // Second child: Panel for Section B
                match &children[1] {
                    AemNode::Panel {
                        title,
                        children: panel_children,
                        ..
                    } => {
                        assert_eq!(title, "Section B");
                        // fieldB + footer paragraph (H2 heading is NOT converted to TextDraw)
                        assert_eq!(panel_children.len(), 2);
                        assert!(matches!(&panel_children[0], AemNode::TextField { .. }));
                        assert!(matches!(&panel_children[1], AemNode::TextDraw { .. }));
                    }
                    other => panic!("Expected Panel for Section B, got {:?}", other),
                }
            }
            other => panic!("Expected Root, got {:?}", other),
        }
    }

    #[test]
    fn convert_paragraph_produces_textdraw() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Hello world"),
            som_path: None,
            source_name: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::TextDraw { content, .. } => {
                assert!(content.contains("<p>"));
                assert!(content.contains("Hello world"));
            }
            other => panic!("Expected TextDraw, got {:?}", other),
        }
    }

    #[test]
    fn inserts_preface_and_appendix_into_first_and_last_page_panels() {
        let mut config = default_config();
        config
            .component_templates
            .insert("preface".into(), "<preface/>".into());
        config
            .component_templates
            .insert("appendix".into(), "<appendix/>".into());

        let nodes = vec![
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H2,
                content: InlineText::plain("Page One"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("First page content"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H2,
                content: InlineText::plain("Page Two"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Second page content"),
                som_path: None,
                source_name: None,
            }),
        ];

        let root = convert_to_aem(&nodes, &config);
        match root {
            AemNode::Root { children, .. } => {
                assert_eq!(children.len(), 2, "expected two page panels");

                match &children[0] {
                    AemNode::Panel {
                        children: page_children,
                        ..
                    } => {
                        assert!(matches!(
                            page_children.first(),
                            Some(AemNode::Preface { .. })
                        ));
                    }
                    other => panic!("Expected first child to be a panel, got {:?}", other),
                }

                match &children[1] {
                    AemNode::Panel {
                        children: page_children,
                        ..
                    } => {
                        assert!(matches!(
                            page_children.last(),
                            Some(AemNode::Appendix { .. })
                        ));
                    }
                    other => panic!("Expected second child to be a panel, got {:?}", other),
                }
            }
            other => panic!("Expected Root, got {:?}", other),
        }
    }

    #[test]
    fn convert_text_field() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "firstName".into(),
            som_path: None,
            label: Some(InlineText::plain("First Name")),
            input_type: FieldType::Text {
                regex: None,
                max_length: Some(50),
                min_length: None,
            },
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::TextField {
                name,
                label,
                max_chars,
                ..
            } => {
                assert_name_pattern(name, "TXT", "FirstName");
                assert_eq!(label, "First Name");
                assert_eq!(*max_chars, Some(50));
            }
            other => panic!("Expected TextField, got {:?}", other),
        }
    }

    #[test]
    fn convert_radio_field() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "gender".into(),
            som_path: None,
            label: Some(InlineText::plain("Gender")),
            input_type: FieldType::Radio {
                options: vec![
                    NameValue {
                        name: "Male".into(),
                        value: InputValue::Text("M".into()),
                    },
                    NameValue {
                        name: "Female".into(),
                        value: InputValue::Text("F".into()),
                    },
                ],
            },
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::RadioButton { name, options, .. } => {
                assert_name_pattern(name, "RB", "Gender");
                assert_eq!(options.len(), 2);
                assert_eq!(options[0].label, "Male");
                assert_eq!(options[0].value, "M");
            }
            other => panic!("Expected RadioButton, got {:?}", other),
        }
    }

    #[test]
    fn convert_select_field() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "country".into(),
            som_path: None,
            label: Some(InlineText::plain("Country")),
            input_type: FieldType::Select {
                options: vec![
                    NameValue {
                        name: "Switzerland".into(),
                        value: InputValue::Text("CH".into()),
                    },
                    NameValue {
                        name: "Germany".into(),
                        value: InputValue::Text("DE".into()),
                    },
                ],
            },
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::Dropdown { name, options, .. } => {
                assert_name_pattern(name, "DD", "Country");
                assert_eq!(options.len(), 2);
            }
            other => panic!("Expected Dropdown, got {:?}", other),
        }
    }

    #[test]
    fn convert_checkbox_from_bool() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "agreeTerms".into(),
            som_path: None,
            label: Some(InlineText::plain("I agree to the terms")),
            input_type: FieldType::Bool,
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::Checkbox { name, options, .. } => {
                assert_name_pattern(name, "CB", "IAgreeToTheTerms");
                assert_eq!(options.len(), 1);
                assert_eq!(options[0].value, "true");
            }
            other => panic!("Expected Checkbox, got {:?}", other),
        }
    }

    #[test]
    fn convert_date_field() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "birthDate".into(),
            som_path: None,
            label: Some(InlineText::plain("Date of Birth")),
            input_type: FieldType::Date,
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::DatePicker { name, label, .. } => {
                assert_name_pattern(name, "DATE", "DateOfBirth");
                assert_eq!(label, "Date of Birth");
            }
            other => panic!("Expected DatePicker, got {:?}", other),
        }
    }

    #[test]
    fn convert_repeatable() {
        let nodes = vec![StructuredNode::Repeatable(RepeatableNode {
            item: Box::new(StructuredNode::Field(FieldNode {
                name: "phone".into(),
                som_path: None,
                label: Some(InlineText::plain("Phone")),
                input_type: FieldType::Text {
                    regex: None,
                    max_length: None,
                    min_length: None,
                },
                value: None,
                placeholder: None,
            })),
            min_occurrences: 1,
            max_occurrences: Some(5),
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::Repeatable {
                min_occur,
                max_occur,
                children,
                ..
            } => {
                assert_eq!(*min_occur, 1);
                assert_eq!(*max_occur, 5);
                assert_eq!(children.len(), 1);
            }
            other => panic!("Expected Repeatable, got {:?}", other),
        }
    }

    #[test]
    fn convert_group_produces_panel() {
        let nodes = vec![StructuredNode::Group(GroupNode {
            children: vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Info"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "x".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
            ],
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::Panel { children, .. } => {
                assert_eq!(children.len(), 2);
            }
            other => panic!("Expected Panel, got {:?}", other),
        }
    }

    #[test]
    fn empty_nodes_are_skipped() {
        let nodes = vec![
            StructuredNode::Empty,
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("visible"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Empty,
        ];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn deterministic_uuids_are_reproducible() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("test"),
            som_path: None,
            source_name: None,
        })];
        let config = default_config();
        let root1 = convert_to_aem(&nodes, &config);
        let root2 = convert_to_aem(&nodes, &config);

        let uuid1 = match &unwrap_preamble(&root1)[0] {
            AemNode::TextDraw { uuid, .. } => *uuid,
            _ => panic!(),
        };
        let uuid2 = match &unwrap_preamble(&root2)[0] {
            AemNode::TextDraw { uuid, .. } => *uuid,
            _ => panic!(),
        };
        assert_eq!(uuid1, uuid2);
    }

    #[test]
    fn grid_layout_distributes_colspan() {
        let nodes = vec![StructuredNode::GridLayout(GridLayout {
            columns: 3,
            elements: vec![
                GridLayoutElement {
                    span: 1,
                    node: StructuredNode::Field(FieldNode {
                        name: "a".into(),
                        som_path: None,
                        label: None,
                        input_type: FieldType::Text {
                            regex: None,
                            max_length: None,
                            min_length: None,
                        },
                        value: None,
                        placeholder: None,
                    }),
                },
                GridLayoutElement {
                    span: 2,
                    node: StructuredNode::Field(FieldNode {
                        name: "b".into(),
                        som_path: None,
                        label: None,
                        input_type: FieldType::Text {
                            regex: None,
                            max_length: None,
                            min_length: None,
                        },
                        value: None,
                        placeholder: None,
                    }),
                },
            ],
        })];
        let config = default_config();
        let root = convert_to_aem(&nodes, &config);
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::Panel {
                children,
                dor_num_cols,
                dor_colspan,
                ..
            } => {
                // Grid with 3 columns → dorNumCols = 3
                assert_eq!(*dor_num_cols, Some(3));
                // The panel itself has no dor_colspan (top-level)
                assert_eq!(*dor_colspan, None);
                assert_eq!(children.len(), 2);
                // span=1 of 3 columns → 12/3*1 = 4, dorColspan = 1
                match &children[0] {
                    AemNode::TextField {
                        colspan,
                        dor_colspan,
                        ..
                    } => {
                        assert_eq!(*colspan, 4);
                        assert_eq!(*dor_colspan, Some(1));
                    }
                    other => panic!("Expected TextField, got {:?}", other),
                }
                // span=2 of 3 columns → 12/3*2 = 8, dorColspan = 2
                match &children[1] {
                    AemNode::TextField {
                        colspan,
                        dor_colspan,
                        ..
                    } => {
                        assert_eq!(*colspan, 8);
                        assert_eq!(*dor_colspan, Some(2));
                    }
                    other => panic!("Expected TextField, got {:?}", other),
                }
            }
            other => panic!("Expected Panel, got {:?}", other),
        }
    }

    // ========================================================================
    // Naming heuristic tests
    // ========================================================================

    #[test]
    fn to_camel_case_basic() {
        assert_eq!(to_camel_case("First Name"), "FirstName");
        assert_eq!(to_camel_case("Date of Birth"), "DateOfBirth");
        assert_eq!(to_camel_case("hello world"), "HelloWorld");
    }

    #[test]
    fn to_camel_case_strips_html() {
        assert_eq!(to_camel_case("<b>bold text</b>"), "BoldText");
        assert_eq!(to_camel_case("<h3>Sub Title</h3>"), "SubTitle");
    }

    #[test]
    fn to_camel_case_strips_special_chars() {
        assert_eq!(
            to_camel_case("I agree to the terms & conditions!"),
            "IAgreeToTheTermsConditions"
        );
        assert_eq!(to_camel_case("field-name_here"), "FieldNameHere");
        assert_eq!(to_camel_case("hello/world"), "HelloWorld");
    }

    #[test]
    fn to_camel_case_truncates_at_word_boundary() {
        // "ThisIsAVeryLongFieldLabelThatExceedsTheMaximumLength" > 30 chars
        let long = "this is a very long field label that exceeds the maximum length";
        let result = to_camel_case(long);
        assert!(
            result.len() <= MAX_CAMEL_CASE_LEN,
            "CamelCase '{}' (len {}) should be <= {}",
            result,
            result.len(),
            MAX_CAMEL_CASE_LEN
        );
        // Should start with the first words
        assert!(result.starts_with("ThisIsAVeryLong"));
    }

    #[test]
    fn to_camel_case_empty_input() {
        assert_eq!(to_camel_case(""), "");
        assert_eq!(to_camel_case("   "), "");
        assert_eq!(to_camel_case("!!!"), "");
    }

    #[test]
    fn to_camel_case_numeric_input() {
        assert_eq!(to_camel_case("123 abc"), "123Abc");
        assert_eq!(to_camel_case("42"), "42");
    }

    #[test]
    fn short_uuid_returns_8_hex_chars() {
        let uuid = Uuid::new_v5(&NAMESPACE_AEM, b"test");
        let short = short_uuid(&uuid);
        assert_eq!(short.len(), 8);
        assert!(short.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn make_name_with_text_produces_prefix_camel_uuid() {
        let config = default_config();
        let mut ctx = ConversionContext::new(&config);
        let name = ctx.make_name("TXT", "First Name");
        assert_name_pattern(&name, "TXT", "FirstName");
    }

    #[test]
    fn make_name_without_text_produces_prefix_uuid() {
        let config = default_config();
        let mut ctx = ConversionContext::new(&config);
        let name = ctx.make_name("PN", "");
        assert_name_pattern(&name, "PN", "");
        // Should be PREFIX_shortUuid (no CamelCase middle part)
        let parts: Vec<&str> = name.splitn(2, '_').collect();
        assert_eq!(parts[0], "PN");
        assert_eq!(parts[1].len(), 8, "Should be just the short UUID");
    }

    #[test]
    fn make_name_unique_for_identical_labels() {
        let config = default_config();
        let mut ctx = ConversionContext::new(&config);
        let name1 = ctx.make_name("TXT", "Same Label");
        let name2 = ctx.make_name("TXT", "Same Label");
        assert_ne!(
            name1, name2,
            "Identical labels should produce different names"
        );
        // Both should follow the correct pattern
        assert_name_pattern(&name1, "TXT", "SameLabel");
        assert_name_pattern(&name2, "TXT", "SameLabel");
    }

    #[test]
    fn make_name_deterministic_across_runs() {
        let config = default_config();
        let mut ctx1 = ConversionContext::new(&config);
        let mut ctx2 = ConversionContext::new(&config);
        let name1 = ctx1.make_name("ST", "Hello World");
        let name2 = ctx2.make_name("ST", "Hello World");
        assert_eq!(name1, name2, "Deterministic mode should produce same names");
    }

    #[test]
    fn field_name_uses_label_text() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "some_uuid_path".into(),
            som_path: None,
            label: Some(InlineText::plain("Account Number")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::TextField { name, .. } => {
                assert_name_pattern(name, "TXT", "AccountNumber");
            }
            other => panic!("Expected TextField, got {:?}", other),
        }
    }

    #[test]
    fn field_name_falls_back_to_som_path() {
        use crate::xfa::scripting::SomPath;
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "some_path".into(),
            som_path: Some(SomPath::new("UBSForms.Page.Details.TF_FamilyName")),
            label: None,
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::TextField { name, .. } => {
                // Should use the last SOM path segment
                assert_name_pattern(name, "TXT", "TfFamilyname");
            }
            other => panic!("Expected TextField, got {:?}", other),
        }
    }

    #[test]
    fn email_field_uses_eml_prefix() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "email".into(),
            som_path: None,
            label: Some(InlineText::plain("Email Address")),
            input_type: FieldType::Email,
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::TextField { name, .. } => {
                assert_name_pattern(name, "EML", "EmailAddress");
            }
            other => panic!("Expected TextField, got {:?}", other),
        }
    }

    #[test]
    fn tel_field_uses_tel_prefix() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "phone".into(),
            som_path: None,
            label: Some(InlineText::plain("Mobile Phone")),
            input_type: FieldType::Tel,
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::TextField { name, .. } => {
                assert_name_pattern(name, "TEL", "MobilePhone");
            }
            other => panic!("Expected TextField, got {:?}", other),
        }
    }

    #[test]
    fn number_field_uses_nb_prefix() {
        let nodes = vec![StructuredNode::Field(FieldNode {
            name: "amount".into(),
            som_path: None,
            label: Some(InlineText::plain("Amount")),
            input_type: FieldType::Number {
                min: None,
                max: None,
                step: None,
            },
            value: None,
            placeholder: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::NumberField { name, .. } => {
                assert_name_pattern(name, "NB", "Amount");
            }
            other => panic!("Expected NumberField, got {:?}", other),
        }
    }

    #[test]
    fn heading_name_uses_text_content() {
        let nodes = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H3,
            content: InlineText::plain("Client Details"),
            som_path: None,
            source_name: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::TitleDraw { name, .. } => {
                assert_name_pattern(name, "TTL", "ClientDetails");
            }
            other => panic!("Expected TitleDraw, got {:?}", other),
        }
    }

    #[test]
    fn paragraph_name_uses_text_content() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Please fill in the form below."),
            som_path: None,
            source_name: None,
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        match &children[0] {
            AemNode::TextDraw { name, .. } => {
                assert_name_pattern(name, "ST", "PleaseFillInTheFormBelow");
            }
            other => panic!("Expected TextDraw, got {:?}", other),
        }
    }

    #[test]
    fn h2_section_panel_name_uses_title() {
        let nodes = vec![
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H2,
                content: InlineText::plain("Personal Information"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("details"),
                som_path: None,
                source_name: None,
            }),
        ];
        let root = convert_to_aem(&nodes, &default_config());
        match &root {
            AemNode::Root { children, .. } => {
                assert_eq!(children.len(), 1);
                match &children[0] {
                    AemNode::Panel { name, title, .. } => {
                        assert_name_pattern(name, "PN", "PersonalInformation");
                        assert_eq!(title, "Personal Information");
                    }
                    other => panic!("Expected Panel, got {:?}", other),
                }
            }
            other => panic!("Expected Root, got {:?}", other),
        }
    }

    // ========================================================================
    // Conditional visibility tests
    // ========================================================================

    #[test]
    fn conditional_produces_hidden_panel_and_wires_trigger() {
        // A radio field + conditional node should produce:
        // 1. The radio button with conditions wired
        // 2. A hidden panel wrapping the conditional content
        let field_id: FieldId = "form.page.radioField".into();

        let nodes = vec![
            StructuredNode::Field(FieldNode {
                name: field_id.clone(),
                som_path: None,
                label: Some(InlineText::plain("Pick")),
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Yes".into()),
                            value: InputValue::Text("yes".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("No".into()),
                            value: InputValue::Text("no".into()),
                        },
                    ],
                },
                value: None,
                placeholder: None,
            }),
            StructuredNode::Conditional(ConditionalNode {
                condition: FieldCondition {
                    field_name: field_id.clone(),
                    value: InputValue::Text("yes".into()),
                },
                content: Box::new(StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Shown when yes"),
                    som_path: None,
                    source_name: None,
                })),
            }),
        ];

        let config = default_config();
        let root = convert_to_aem(&nodes, &config);

        // Unwrap root → single preamble panel → children
        let children = unwrap_preamble(&root);
        assert_eq!(
            children.len(),
            2,
            "Should have radio button and conditional panel"
        );

        // The first child should be a RadioButton with conditions wired
        match &children[0] {
            AemNode::RadioButton {
                conditions,
                field_id,
                ..
            } => {
                assert!(field_id.is_some(), "RadioButton should have a field_id set");
                assert_eq!(
                    conditions.len(),
                    1,
                    "RadioButton should have exactly one condition rule"
                );
                assert!(conditions[0].show, "Condition rule should have show=true");
                assert_eq!(
                    conditions[0].value,
                    InputValue::Text("yes".into()),
                    "Condition should trigger on 'yes'"
                );
            }
            other => panic!("Expected RadioButton, got {:?}", other),
        }

        // The second child should be a hidden Panel
        match &children[1] {
            AemNode::Panel {
                visible,
                dor_exclude,
                title,
                is_conditional,
                ..
            } => {
                assert!(!visible, "Conditional panel should start hidden");
                assert!(*dor_exclude, "Conditional panel should exclude from DOR");
                assert!(
                    title.contains("Condition"),
                    "Conditional panel title should mention 'Condition'. Got: {}",
                    title
                );
                assert!(is_conditional, "Panel should be marked as conditional");
            }
            other => panic!("Expected Panel, got {:?}", other),
        }
    }

    #[test]
    fn multiple_conditions_same_trigger_grouped() {
        // Two conditionals referencing the same field should both wire to that field
        let field_id: FieldId = "form.page.dropdown".into();

        let nodes = vec![
            StructuredNode::Field(FieldNode {
                name: field_id.clone(),
                som_path: None,
                label: Some(InlineText::plain("Type")),
                input_type: FieldType::Select {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("A".into()),
                            value: InputValue::Text("a".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("B".into()),
                            value: InputValue::Text("b".into()),
                        },
                    ],
                },
                value: None,
                placeholder: None,
            }),
            StructuredNode::Conditional(ConditionalNode {
                condition: FieldCondition {
                    field_name: field_id.clone(),
                    value: InputValue::Text("a".into()),
                },
                content: Box::new(StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Content A"),
                    som_path: None,
                    source_name: None,
                })),
            }),
            StructuredNode::Conditional(ConditionalNode {
                condition: FieldCondition {
                    field_name: field_id.clone(),
                    value: InputValue::Text("b".into()),
                },
                content: Box::new(StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Content B"),
                    som_path: None,
                    source_name: None,
                })),
            }),
        ];

        let config = default_config();
        let root = convert_to_aem(&nodes, &config);
        let children = unwrap_preamble(&root);

        assert_eq!(
            children.len(),
            3,
            "Should have dropdown + 2 conditional panels"
        );

        // Dropdown should have 2 condition rules
        match &children[0] {
            AemNode::Dropdown { conditions, .. } => {
                assert_eq!(
                    conditions.len(),
                    2,
                    "Dropdown should have 2 condition rules"
                );
            }
            other => panic!("Expected Dropdown, got {:?}", other),
        }

        // Both panels should be hidden
        for i in 1..3 {
            match &children[i] {
                AemNode::Panel { visible, .. } => {
                    assert!(!visible, "Conditional panel {} should be hidden", i);
                }
                other => panic!("Expected Panel at index {}, got {:?}", i, other),
            }
        }
    }

    #[test]
    fn root_title_uses_h1_heading_text() {
        let nodes = vec![
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H1,
                content: InlineText::plain("My Form Display Title"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Some text"),
                som_path: None,
                source_name: None,
            }),
        ];
        let config = default_config(); // form_title == "TEST" (form code)
        let root = convert_to_aem(&nodes, &config);
        match &root {
            AemNode::Root { title, .. } => {
                assert_eq!(
                    title, "My Form Display Title",
                    "Root title should come from H1 heading text"
                );
            }
            other => panic!("Expected Root, got {:?}", other),
        }
    }

    #[test]
    fn root_title_falls_back_to_form_code_without_h1() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("No heading here"),
            som_path: None,
            source_name: None,
        })];
        let config = default_config();
        let root = convert_to_aem(&nodes, &config);
        match &root {
            AemNode::Root { title, .. } => {
                assert_eq!(
                    title, "TEST",
                    "Root title should fall back to form_title (form code) when no H1"
                );
            }
            other => panic!("Expected Root, got {:?}", other),
        }
    }

    #[test]
    fn panel_leaves_subset_of_fragment_requires_all_leaves() {
        let fragment = ParsedFragment {
            dir_name: "frag1".to_string(),
            frag_ref: "/content/dam/formsanddocuments/frag1".to_string(),
            name: "Frag1".to_string(),
            xsd_type_name: "SomeType".to_string(),
            bound_elements: vec!["IBAN".to_string(), "Name".to_string(), "Date".to_string()],
        };

        // Subset: all leaves exist in fragment → true
        let leaves = vec!["IBAN".to_string(), "Name".to_string()];
        assert!(panel_leaves_subset_of_fragment(&fragment, &leaves));

        // Exact match → true
        let leaves = vec!["IBAN".to_string(), "Name".to_string(), "Date".to_string()];
        assert!(panel_leaves_subset_of_fragment(&fragment, &leaves));

        // Not a subset: "Company" is NOT in fragment → false
        let leaves = vec!["IBAN".to_string(), "Company".to_string()];
        assert!(!panel_leaves_subset_of_fragment(&fragment, &leaves));

        // Empty leaves → trivially true (empty set is subset of any set)
        let leaves: Vec<String> = vec![];
        assert!(panel_leaves_subset_of_fragment(&fragment, &leaves));
    }
}
