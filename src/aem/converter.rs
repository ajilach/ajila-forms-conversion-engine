//! Converter from `StructuredNode` trees to `AemNode` trees.
//!
//! The conversion is stateless apart from a small `ConversionContext` that
//! tracks UUID generation and naming counters.

use uuid::Uuid;

use crate::structured::{
    ConditionalNode, FieldNode, FieldType, GridLayout, GroupNode, HeadingLevel, HeadingNode,
    ImageNode, InlineNode, InlineText, InputValue, NameValue, ParagraphNode, RepeatableNode,
    StructuredNode, TableNode, TranslatableString,
};

use super::{AemConfig, AemNode, AemOption, OptionAlignment};

// ============================================================================
// Conversion context
// ============================================================================

/// Namespace UUID used for deterministic UUID v5 generation.
const NAMESPACE_AEM: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// Internal state carried through the conversion.
struct ConversionContext {
    /// Counter for generating unique names when the source node has no name.
    counter: u32,
    /// Whether to produce deterministic UUIDs.
    deterministic: bool,
    /// The language to prefer when extracting translatable strings.
    language: String,
    /// Default grid column span (total columns).
    grid_columns: u32,
}

impl ConversionContext {
    fn new(config: &AemConfig) -> Self {
        Self {
            counter: 0,
            deterministic: config.deterministic_uuids,
            language: config.master_language.clone(),
            grid_columns: config.grid_columns,
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

    /// Generate a unique name with the given prefix.
    fn next_name(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{}_{}", prefix, self.counter)
    }
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

    // First pass: split StructuredNodes into sections by H2.
    // Each section is (Option<heading_text>, nodes_in_section).
    let mut sections: Vec<(Option<String>, Vec<&StructuredNode>)> = Vec::new();

    for node in nodes {
        if let StructuredNode::Heading(h) = node {
            if matches!(h.level, HeadingLevel::H2) {
                let title = h.content.as_plain_text().trim().to_string();
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
    let mut children: Vec<AemNode> = Vec::new();

    for (title, section_nodes) in &sections {
        let converted: Vec<AemNode> = section_nodes
            .iter()
            .filter_map(|n| convert_node(n, config, &mut ctx, config.grid_columns))
            .collect();

        if let Some(title) = title {
            // H2 section → wrap in a Panel
            let name = ctx.next_name("PN");
            let uuid = ctx.uuid(&name);
            children.push(AemNode::Panel {
                uuid,
                name,
                title: title.clone(),
                children: converted,
                is_page: true,
                dor_exclude: false,
            });
        } else {
            // Preamble (before first H2) → also wrap in a Panel
            if !converted.is_empty() {
                let name = ctx.next_name("PN");
                let uuid = ctx.uuid(&name);
                children.push(AemNode::Panel {
                    uuid,
                    name,
                    title: String::new(),
                    children: converted,
                    is_page: true,
                    dor_exclude: false,
                });
            }
        }
    }

    AemNode::Root {
        title: config.form_title.clone(),
        children,
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
) -> Option<AemNode> {
    match node {
        StructuredNode::Heading(h) if matches!(h.level, HeadingLevel::H1 | HeadingLevel::H2) => {
            // H1 is used as the form title; H2 is used for section panel titles.
            // Neither should produce an inline TextDraw.
            None
        }
        StructuredNode::Heading(h) => Some(convert_heading(h, config, ctx, colspan)),
        StructuredNode::Paragraph(p) => Some(convert_paragraph(p, config, ctx, colspan)),
        StructuredNode::Image(img) => Some(convert_image(img, config, ctx, colspan)),
        StructuredNode::Table(t) => Some(convert_table(t, config, ctx, colspan)),
        StructuredNode::Field(f) => Some(convert_field(f, config, ctx, colspan)),
        StructuredNode::Repeatable(r) => Some(convert_repeatable(r, config, ctx)),
        StructuredNode::Group(g) => Some(convert_group(g, config, ctx)),
        StructuredNode::Conditional(c) => Some(convert_conditional(c, config, ctx)),
        StructuredNode::GridLayout(gl) => Some(convert_grid_layout(gl, config, ctx)),
        StructuredNode::List(_) => None, // TODO: implement AEM list conversion
        StructuredNode::Empty => None,
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
) -> AemNode {
    let tag = match h.level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    };
    let plain = inline_text_to_html(&h.content, &ctx.language);
    let content = format!("<{tag}>{plain}</{tag}>");
    let name = ctx.next_name("ST");
    let uuid = ctx.uuid(&name);
    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: false,
        colspan,
    }
}

fn convert_paragraph(
    p: &ParagraphNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
) -> AemNode {
    let html = inline_text_to_html(&p.content, &ctx.language);
    let content = format!("<p>{html}</p>");
    let name = ctx.next_name("ST");
    let uuid = ctx.uuid(&name);
    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: false,
        colspan,
    }
}

fn convert_image(
    img: &ImageNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
) -> AemNode {
    let alt = img.alt_text.as_deref().unwrap_or("image");
    let content = if img.content.is_empty() {
        format!("<p>[Image: {alt}]</p>")
    } else {
        let b64 = base64_encode(&img.content);
        format!("<p><img src=\"data:image/png;base64,{b64}\" alt=\"{alt}\" /></p>")
    };
    let name = ctx.next_name("IMG");
    let uuid = ctx.uuid(&name);
    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: false,
        colspan,
    }
}

fn convert_table(
    table: &TableNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
    _colspan: u32,
) -> AemNode {
    // Convert the table into a panel with child rows.
    // Each row becomes a sub-panel with cells distributed across the grid.
    let name = ctx.next_name("TBL");
    let uuid = ctx.uuid(&name);
    let title = table
        .caption
        .as_ref()
        .map(|c| inline_text_to_html(c, &ctx.language))
        .unwrap_or_default();

    let mut children = Vec::new();

    // Header row
    if let Some(header) = &table.header {
        let cols = header.cells.len().max(1) as u32;
        let col_span = ctx.grid_columns / cols;
        let row_name = ctx.next_name("TBLHDR");
        let row_uuid = ctx.uuid(&row_name);
        let cells: Vec<AemNode> = header
            .cells
            .iter()
            .filter_map(|cell| convert_node(cell, config, ctx, col_span))
            .collect();
        children.push(AemNode::Panel {
            uuid: row_uuid,
            name: row_name,
            title: String::new(),
            children: cells,
            is_page: false,
            dor_exclude: false,
        });
    }

    // Body rows
    for row in &table.rows {
        let cols = row.cells.len().max(1) as u32;
        let col_span = ctx.grid_columns / cols;
        let row_name = ctx.next_name("TBLROW");
        let row_uuid = ctx.uuid(&row_name);
        let cells: Vec<AemNode> = row
            .cells
            .iter()
            .filter_map(|cell| convert_node(cell, config, ctx, col_span))
            .collect();
        children.push(AemNode::Panel {
            uuid: row_uuid,
            name: row_name,
            title: String::new(),
            children: cells,
            is_page: false,
            dor_exclude: false,
        });
    }

    AemNode::Panel {
        uuid,
        name,
        title,
        children,
        is_page: false,
        dor_exclude: false,
    }
}

fn convert_field(
    f: &FieldNode,
    _config: &AemConfig,
    ctx: &mut ConversionContext,
    colspan: u32,
) -> AemNode {
    let label = f
        .label
        .as_ref()
        .map(|l| inline_text_to_html(l, &ctx.language))
        .unwrap_or_default();

    match &f.input_type {
        FieldType::Text { max_length, .. } => {
            let name = format!("TF_{}", &f.name);
            let uuid = ctx.uuid(&name);
            AemNode::TextField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                max_chars: *max_length,
                colspan,
            }
        }

        FieldType::Number { .. } => {
            let name = format!("NF_{}", &f.name);
            let uuid = ctx.uuid(&name);
            AemNode::NumberField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                colspan,
            }
        }

        FieldType::Date => {
            let name = format!("DATE_{}", &f.name);
            let uuid = ctx.uuid(&name);
            AemNode::DatePicker {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                colspan,
            }
        }

        FieldType::Email | FieldType::Tel => {
            // Map to a text field with appropriate CSS hint
            let name = format!("TF_{}", &f.name);
            let uuid = ctx.uuid(&name);
            AemNode::TextField {
                uuid,
                name,
                label,
                mandatory: false,
                visible: true,
                max_chars: None,
                colspan,
            }
        }

        FieldType::Bool => {
            let name = format!("CB_{}", &f.name);
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
            }
        }

        FieldType::Radio { options } => {
            let name = format!("RB_{}", &f.name);
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
            }
        }

        FieldType::Select { options } => {
            let name = format!("DD_{}", &f.name);
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
            }
        }
    }
}

fn convert_repeatable(
    r: &RepeatableNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
) -> AemNode {
    let name = ctx.next_name("RPT");
    let uuid = ctx.uuid(&name);
    let inner = convert_node(&r.item, config, ctx, config.grid_columns);
    let children = inner.into_iter().collect();
    AemNode::Repeatable {
        uuid,
        name: name.clone(),
        title: name,
        children,
        min_occur: r.min_occurrences,
        max_occur: r.max_occurrences.unwrap_or(config.repeatable_max_occur),
    }
}

fn convert_group(g: &GroupNode, config: &AemConfig, ctx: &mut ConversionContext) -> AemNode {
    let name = ctx.next_name("PN");
    let uuid = ctx.uuid(&name);
    let children: Vec<AemNode> = g
        .children
        .iter()
        .filter_map(|n| convert_node(n, config, ctx, config.grid_columns))
        .collect();
    AemNode::Panel {
        uuid,
        name,
        title: String::new(),
        children,
        is_page: false,
        dor_exclude: false,
    }
}

fn convert_conditional(
    c: &ConditionalNode,
    config: &AemConfig,
    ctx: &mut ConversionContext,
) -> AemNode {
    let name = ctx.next_name("COND");
    let uuid = ctx.uuid(&name);
    let inner = convert_node(&c.content, config, ctx, config.grid_columns);
    let children: Vec<AemNode> = inner.into_iter().collect();
    // The condition is stored as the panel title for traceability.
    // Proper fd:rules / visibility expressions would be generated in a future
    // iteration.
    let title = format!(
        "Condition: {} = {}",
        c.condition.field_name,
        format_input_value(&c.condition.value)
    );
    AemNode::Panel {
        uuid,
        name,
        title,
        children,
        is_page: false,
        dor_exclude: false,
    }
}

fn convert_grid_layout(
    gl: &GridLayout,
    config: &AemConfig,
    ctx: &mut ConversionContext,
) -> AemNode {
    let name = ctx.next_name("GRID");
    let uuid = ctx.uuid(&name);
    let total = gl.columns as u32;
    let children: Vec<AemNode> = gl
        .elements
        .iter()
        .filter_map(|elem| {
            let col_span = (elem.span as u32 * config.grid_columns) / total;
            convert_node(&elem.node, config, ctx, col_span.max(1))
        })
        .collect();
    AemNode::Panel {
        uuid,
        name,
        title: String::new(),
        children,
        is_page: false,
        dor_exclude: false,
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Convert `InlineText` to a simple HTML string.
fn inline_text_to_html(text: &InlineText, language: &str) -> String {
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
                .or_else(|| map.values().next())
                .map(|s| s.as_str())
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
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn convert_name_values(options: &[NameValue], language: &str) -> Vec<AemOption> {
    options
        .iter()
        .map(|nv| {
            let label = match &nv.name {
                TranslatableString::Plain(s) => s.clone(),
                TranslatableString::Translated(map) => map
                    .get(language)
                    .or_else(|| map.values().next())
                    .cloned()
                    .unwrap_or_default(),
            };
            let value = format_input_value(&nv.value);
            AemOption { label, value }
        })
        .collect()
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
        AemConfig {
            deterministic_uuids: true,
            ..Default::default()
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
    fn convert_heading_produces_textdraw() {
        // H3 headings are NOT used for sectioning — they stay inline
        let nodes = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H3,
            content: InlineText::plain("Sub Title"),
        })];
        let root = convert_to_aem(&nodes, &default_config());
        let children = unwrap_preamble(&root);
        assert_eq!(children.len(), 1);
        match &children[0] {
            AemNode::TextDraw { content, .. } => {
                assert!(content.contains("<h3>"));
                assert!(content.contains("Sub Title"));
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
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H2,
                content: InlineText::plain("Section A"),
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
            }),
        ];
        let root = convert_to_aem(&nodes, &default_config());
        match &root {
            AemNode::Root { children, .. } => {
                // Preamble panel + Panel "Section A" + Panel "Section B"
                assert_eq!(
                    children.len(),
                    3,
                    "Expected 3 root children: preamble panel + 2 section panels"
                );

                // First child: preamble panel wrapping the paragraph
                match &children[0] {
                    AemNode::Panel {
                        title,
                        children: panel_children,
                        ..
                    } => {
                        assert!(title.is_empty(), "Preamble panel should have empty title");
                        assert_eq!(panel_children.len(), 1);
                        assert!(matches!(&panel_children[0], AemNode::TextDraw { .. }));
                    }
                    other => panic!("Expected Panel for preamble, got {:?}", other),
                }

                // Second child: Panel for Section A
                match &children[1] {
                    AemNode::Panel {
                        title,
                        children: panel_children,
                        ..
                    } => {
                        assert_eq!(title, "Section A");
                        // Only fieldA (H2 heading is NOT converted to TextDraw)
                        assert_eq!(panel_children.len(), 1);
                        assert!(matches!(&panel_children[0], AemNode::TextField { .. }));
                    }
                    other => panic!("Expected Panel for Section A, got {:?}", other),
                }

                // Third child: Panel for Section B
                match &children[2] {
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
                let expected_id: crate::structured::FieldId = "firstName".into();
                assert_eq!(name, &format!("TF_{}", expected_id));
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
                let expected_id: crate::structured::FieldId = "gender".into();
                assert_eq!(name, &format!("RB_{}", expected_id));
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
                let expected_id: crate::structured::FieldId = "country".into();
                assert_eq!(name, &format!("DD_{}", expected_id));
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
                let expected_id: crate::structured::FieldId = "agreeTerms".into();
                assert_eq!(name, &format!("CB_{}", expected_id));
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
                let expected_id: crate::structured::FieldId = "birthDate".into();
                assert_eq!(name, &format!("DATE_{}", expected_id));
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
            AemNode::Panel { children, .. } => {
                assert_eq!(children.len(), 2);
                // span=1 of 3 columns → 12/3*1 = 4
                match &children[0] {
                    AemNode::TextField { colspan, .. } => assert_eq!(*colspan, 4),
                    other => panic!("Expected TextField, got {:?}", other),
                }
                // span=2 of 3 columns → 12/3*2 = 8
                match &children[1] {
                    AemNode::TextField { colspan, .. } => assert_eq!(*colspan, 8),
                    other => panic!("Expected TextField, got {:?}", other),
                }
            }
            other => panic!("Expected Panel, got {:?}", other),
        }
    }
}
