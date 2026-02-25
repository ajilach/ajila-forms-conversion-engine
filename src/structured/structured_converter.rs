//! Convert Document groups to StructuredNode tree.
//!
//! This module transforms the analyzed Document (with its GroupKind hierarchy)
//! into a semantic StructuredNode tree suitable for rendering or further processing.
//!
//! # Conversion Rules
//!
//! - `Heading { level }` → `HeadingNode`
//! - `TextBlock` / `Paragraph` → `ParagraphNode`
//! - `LabeledField` → `FieldNode` with label extracted from child text
//! - `RadioButtonGroup` / `ExclGroup` → `FieldNode` with `FieldType::Radio`
//! - `DateField` → `FieldNode` with `FieldType::Date`
//! - `Field` → `FieldNode` (unlabeled)
//! - `RepeatableSection` → `RepeatableNode`
//! - `Section` / `Unknown` → `GroupNode`
//! - Leaf (Text) → `ParagraphNode`
//! - Leaf (Field, non-interactive) → `ParagraphNode` (readonly → text)
//! - Leaf (Field, interactive) → `FieldNode`
//!
//! Header, Footer, and InlineField groups are ignored for now.

use crate::document::{Document, GroupKind};
use crate::flattened::{
    Bounds, FlattenedNode, FlattenedNodeKind, Hint, RichRun, RichText, WidgetKind,
};
use crate::structured::{
    ConditionalNode, FieldCondition, FieldId, FieldNode, FieldType, GroupNode, HeadingLevel,
    HeadingNode, InlineNode, InlineText, InputValue, ListNode, NameValue, ParagraphNode,
    RepeatableNode, StructuredNode, TranslatableString,
};
use crate::xfa::scripting::SomPath;

/// Check if a StructuredNode contains any fields (recursively).
fn contains_fields(node: &StructuredNode) -> bool {
    match node {
        StructuredNode::Field(_) => true,
        StructuredNode::Group(g) => g.children.iter().any(contains_fields),
        StructuredNode::Repeatable(r) => contains_fields(&r.item),
        StructuredNode::Conditional(c) => contains_fields(&c.content),
        StructuredNode::GridLayout(g) => g.elements.iter().any(|e| contains_fields(&e.node)),
        StructuredNode::Heading(_)
        | StructuredNode::Paragraph(_)
        | StructuredNode::Image(_)
        | StructuredNode::Table(_)
        | StructuredNode::List(_)
        | StructuredNode::Empty => false,
    }
}

/// Strip a list marker prefix from InlineText.
///
/// Recognizes the same markers as the list detector: unordered bullets
/// (`-`, `–`, `—`, `•`, `◦`, `▪`, `*`) and ordered markers (`1.`, `a.`, `ii.`, etc.)
/// followed by whitespace. Only strips from the first InlineNode if it's a Text node.
fn strip_list_marker_from_inline_text(mut text: InlineText) -> InlineText {
    if text.0.is_empty() {
        return text;
    }

    // Find the first Text node and strip the marker from it
    if let Some(node) = text.0.iter_mut().next() {
        match node {
            InlineNode::Text(s) => {
                if let Some(stripped) = strip_marker_from_str(s) {
                    *s = stripped;
                }
                return text;
            }
            InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
                if let InlineNode::Text(s) = inner.as_mut() {
                    if let Some(stripped) = strip_marker_from_str(s) {
                        *s = stripped;
                    }
                    return text;
                }
                // Not a text node inside styling — stop looking
                return text;
            }
            _ => return text,
        }
    }

    text
}

/// Try to strip a list marker prefix from a string.
/// Returns the stripped string if a marker was found, None otherwise.
fn strip_marker_from_str(s: &str) -> Option<String> {
    let trimmed = s.trim_start();
    let leading_ws = s.len() - trimmed.len();

    if trimmed.is_empty() {
        return None;
    }

    // Unordered markers: non-ambiguous bullets (–, —, •, ◦, ▪) don't need space after
    let unordered_no_space = ['\u{2013}', '\u{2014}', '\u{2022}', '\u{25E6}', '\u{25AA}'];
    for &ch in &unordered_no_space {
        if trimmed.starts_with(ch) {
            let after = &trimmed[ch.len_utf8()..];
            return Some(after.trim_start().to_string());
        }
    }

    // Unordered markers: ambiguous chars (-, *) require space after
    let unordered_need_space = ['-', '*'];
    for &ch in &unordered_need_space {
        if trimmed.starts_with(ch) {
            let after = &trimmed[ch.len_utf8()..];
            if after.is_empty() || after.starts_with(char::is_whitespace) {
                return Some(after.trim_start().to_string());
            }
        }
    }

    // Ordered: digits followed by . or )
    let bytes = trimmed.as_bytes();
    if !bytes.is_empty() && bytes[0].is_ascii_digit() {
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
            let after = &trimmed[i + 1..];
            return Some(after.trim_start().to_string());
        }
    }

    // Ordered: single letter followed by . or )
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && (bytes[1] == b'.' || bytes[1] == b')')
    {
        let after = &trimmed[2..];
        return Some(after.trim_start().to_string());
    }

    // Ordered: roman numerals followed by . or )
    let roman_chars = b"ivxlcdm";
    if !bytes.is_empty() && roman_chars.contains(&bytes[0].to_ascii_lowercase()) {
        let is_upper = bytes[0].is_ascii_uppercase();
        let mut i = 0;
        while i < bytes.len() {
            let ch = bytes[i];
            let is_roman = if is_upper {
                roman_chars.contains(&ch.to_ascii_lowercase()) && ch.is_ascii_uppercase()
            } else {
                roman_chars.contains(&ch) && ch.is_ascii_lowercase()
            };
            if !is_roman {
                break;
            }
            i += 1;
        }
        if i >= 2 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
            let after = &trimmed[i + 1..];
            return Some(after.trim_start().to_string());
        }
    }

    let _ = leading_ws; // leading whitespace is consumed by trim_start
    None
}

/// Convert a Document to a list of StructuredNodes (one per root group).
/// Output is sorted in reading order: top to bottom, left to right.
pub fn convert(doc: &Document) -> Vec<StructuredNode> {
    let converter = Converter { doc };

    // Get roots and sort by reading order (y first, then x)
    let mut roots: Vec<usize> = doc.roots();
    roots.sort_by(|&a, &b| {
        let bounds_a = doc.get_bounds(a);
        let bounds_b = doc.get_bounds(b);
        compare_bounds_reading_order(bounds_a, bounds_b)
    });

    roots
        .into_iter()
        .filter_map(|idx| converter.convert_group(idx))
        .collect()
}

/// Convert a Document to a DocumentEnvelope with context.
/// This wraps the structured nodes with the context metadata.
pub fn convert_with_context(
    doc: &Document,
    context: crate::context::Context,
) -> crate::structured::DocumentEnvelope {
    let content = convert(doc);
    crate::structured::DocumentEnvelope { context, content }
}

/// Compare two bounds in reading order: top to bottom, then left to right.
/// Elements on the same vertical line (within a threshold) are sorted left to right.
fn compare_bounds_reading_order(a: Option<Bounds>, b: Option<Bounds>) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater, // Items without bounds go to the end
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => {
            // Use a small threshold for "same line" comparison (about 2 points)
            let line_threshold = rust_decimal::Decimal::new(20, 1); // 2.0

            let y_diff = a.y - b.y;
            if y_diff.abs() <= line_threshold {
                // Same line - sort by x (left to right)
                a.x.cmp(&b.x)
            } else {
                // Different lines - sort by y (top to bottom)
                a.y.cmp(&b.y)
            }
        }
    }
}

struct Converter<'a, 'b> {
    doc: &'a Document<'b>,
}

impl<'a, 'b> Converter<'a, 'b> {
    /// Convert a single group to a StructuredNode.
    fn convert_group(&self, group_idx: usize) -> Option<StructuredNode> {
        let group = self.doc.get_group(group_idx)?;

        match &group.kind {
            // Skip header/footer for now
            GroupKind::Header | GroupKind::Footer => None,

            // Skip inline fields for now
            GroupKind::InlineField => None,

            // Skip non-printable elements (relevant="-print")
            GroupKind::NoPrint => None,

            // Heading → HeadingNode
            GroupKind::Heading { level } => {
                let text = self.extract_inline_text(group_idx);
                Some(StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::from_u8(*level),
                    content: text,
                }))
            }

            // TextBlock / Paragraph → ParagraphNode (or multiple if rich text with multiple paragraphs)
            GroupKind::TextBlock | GroupKind::Paragraph => {
                // Check if this is a single-node group with rich text containing multiple paragraphs
                let nodes = self.doc.collect_nodes(group_idx);
                if nodes.len() == 1 {
                    let node = nodes[0];
                    for hint in &node.hints {
                        if let Hint::RichContent(rich_text) = hint {
                            let paragraphs = self.convert_rich_text_to_paragraph_nodes(rich_text);
                            if paragraphs.len() > 1 {
                                // Multiple paragraphs - wrap in a GroupNode
                                return Some(StructuredNode::Group(GroupNode {
                                    children: paragraphs,
                                }));
                            } else if paragraphs.len() == 1 {
                                // Single paragraph - return it directly
                                return Some(paragraphs.into_iter().next().unwrap());
                            }
                            // No paragraphs (all empty) - return None
                            return None;
                        }
                    }
                }

                // Fallback: extract as single inline text
                let text = self.extract_inline_text(group_idx);
                if text.is_empty() {
                    None
                } else {
                    Some(StructuredNode::Paragraph(ParagraphNode { content: text }))
                }
            }

            // LabeledField → FieldNode with label
            GroupKind::LabeledField { label, field } => {
                let label_group = group.children.get(*label).copied()?;
                let field_group = group.children.get(*field).copied()?;

                let label_text = self.extract_inline_text(label_group);
                self.convert_field_group(field_group, Some(label_text))
            }

            // RadioButton → FieldNode (single option, usually wrapped in RadioButtonGroup)
            GroupKind::RadioButton { field, label } => {
                let label_group = group.children.get(*label).copied()?;
                let field_group = group.children.get(*field).copied()?;

                let label_text = self.extract_inline_text(label_group);
                self.convert_field_group(field_group, Some(label_text))
            }

            // Checkbox → FieldNode (boolean field with label)
            GroupKind::Checkbox { field, label } => {
                let label_group = group.children.get(*label).copied()?;
                let field_group = group.children.get(*field).copied()?;

                let label_text = self.extract_inline_text(label_group);
                self.convert_field_group(field_group, Some(label_text))
            }

            // RadioButtonGroup → FieldNode with Radio type
            GroupKind::RadioButtonGroup => self.convert_radio_button_group(group_idx),

            // ExclGroup → FieldNode with Radio type
            GroupKind::ExclGroup { selected_value } => {
                self.convert_excl_group(group_idx, selected_value.clone())
            }

            // DateField → FieldNode with Date type
            GroupKind::DateField { num_fields: _ } => self.convert_date_field(group_idx),

            // InlineDateField → FieldNode with Date type + optional suffix paragraph
            GroupKind::InlineDateField {
                label_text,
                suffix_text,
                generated_name,
                ..
            } => self.convert_inline_date_field(label_text, suffix_text, generated_name),

            // Field → FieldNode (wrapped single field, unlabeled)
            GroupKind::Field => self.convert_field_group(group_idx, None),

            // RepeatableSection → RepeatableNode (only if it contains fields)
            GroupKind::RepeatableSection {
                min_occurrences,
                max_occurrences,
            } => {
                let children = self.convert_children(group_idx);
                if children.is_empty() {
                    return None;
                }
                let item = if children.len() == 1 {
                    children.into_iter().next().unwrap()
                } else {
                    StructuredNode::Group(GroupNode { children })
                };
                // Only create a RepeatableNode if it contains at least one field
                if contains_fields(&item) {
                    Some(StructuredNode::Repeatable(RepeatableNode {
                        item: Box::new(item),
                        min_occurrences: *min_occurrences,
                        max_occurrences: *max_occurrences,
                    }))
                } else {
                    // No fields - just return the content without the repeatable wrapper
                    Some(item)
                }
            }

            // GridLayout → StructuredNode::GridLayout
            GroupKind::GridLayout { columns, spans } => {
                use crate::structured::{GridLayout, GridLayoutElement};

                let children = self.convert_children(group_idx);
                if children.is_empty() {
                    return None;
                }

                // Create grid layout elements with spans
                let elements: Vec<GridLayoutElement> = children
                    .into_iter()
                    .enumerate()
                    .map(|(i, node)| GridLayoutElement {
                        span: spans.get(i).copied().unwrap_or(1),
                        node,
                    })
                    .collect();

                Some(StructuredNode::GridLayout(GridLayout {
                    columns: *columns,
                    elements,
                }))
            }

            // Section / Unknown → GroupNode
            GroupKind::Section | GroupKind::Unknown => {
                let children = self.convert_children(group_idx);
                if children.is_empty() {
                    None
                } else if children.len() == 1 {
                    children.into_iter().next()
                } else {
                    Some(StructuredNode::Group(GroupNode { children }))
                }
            }

            // RadioButtonContent → ConditionalNode wrapping the child content
            GroupKind::RadioButtonContent {
                excl_group_som_path,
                option_field_name,
            } => {
                let children = self.convert_children(group_idx);
                if children.is_empty() {
                    return None;
                }
                let content = if children.len() == 1 {
                    children.into_iter().next().unwrap()
                } else {
                    StructuredNode::Group(GroupNode { children })
                };
                Some(StructuredNode::Conditional(ConditionalNode {
                    condition: FieldCondition {
                        field_name: FieldId::from_som_path(excl_group_som_path),
                        value: InputValue::Text(option_field_name.clone()),
                    },
                    content: Box::new(content),
                }))
            }

            // List → ListNode
            GroupKind::List { list_style } => {
                let group = self.doc.get_group(group_idx)?;
                let mut items = Vec::new();
                // Sort children by reading order
                let mut children = group.children.clone();
                children.sort_by(|&a, &b| {
                    let bounds_a = self.doc.get_bounds(a);
                    let bounds_b = self.doc.get_bounds(b);
                    compare_bounds_reading_order(bounds_a, bounds_b)
                });
                for &child_idx in &children {
                    let text = self.extract_inline_text(child_idx);
                    if !text.is_empty() {
                        // Strip list marker prefix from the item text
                        let stripped = strip_list_marker_from_inline_text(text);
                        items.push(stripped);
                    }
                }
                if items.is_empty() {
                    None
                } else {
                    Some(StructuredNode::List(ListNode {
                        list_style: *list_style,
                        items,
                    }))
                }
            }

            // Leaf → depends on node type
            GroupKind::Leaf { node_index } => self.convert_leaf(*node_index),
        }
    }

    /// Convert all children of a group, sorted in reading order.
    fn convert_children(&self, group_idx: usize) -> Vec<StructuredNode> {
        let Some(group) = self.doc.get_group(group_idx) else {
            return vec![];
        };

        // Sort children by reading order before converting
        let mut children: Vec<usize> = group.children.clone();
        children.sort_by(|&a, &b| {
            let bounds_a = self.doc.get_bounds(a);
            let bounds_b = self.doc.get_bounds(b);
            compare_bounds_reading_order(bounds_a, bounds_b)
        });

        children
            .iter()
            .filter_map(|&child_idx| self.convert_group(child_idx))
            .collect()
    }

    /// Convert a leaf node (text or field).
    fn convert_leaf(&self, node_index: usize) -> Option<StructuredNode> {
        let node = self.doc.get_node(node_index)?;

        match &node.kind {
            FlattenedNodeKind::Text { content, .. } => {
                if content.trim().is_empty() {
                    None
                } else {
                    // Check if this node has rich text with multiple paragraphs
                    for hint in &node.hints {
                        if let Hint::RichContent(rich_text) = hint {
                            let paragraphs = self.convert_rich_text_to_paragraph_nodes(rich_text);
                            if paragraphs.len() > 1 {
                                // Multiple paragraphs - wrap in a GroupNode
                                return Some(StructuredNode::Group(GroupNode {
                                    children: paragraphs,
                                }));
                            } else if paragraphs.len() == 1 {
                                // Single paragraph - return it directly
                                return Some(paragraphs.into_iter().next().unwrap());
                            }
                            // No paragraphs (all empty) - fall through to None
                        }
                    }
                    // No rich text or fallback - use plain text
                    let text = self.build_inline_text_from_node(node);
                    if text.is_empty() {
                        None
                    } else {
                        Some(StructuredNode::Paragraph(ParagraphNode { content: text }))
                    }
                }
            }
            FlattenedNodeKind::Field { .. } => {
                // Check if interactive
                if self.is_interactive(node) {
                    self.build_field_node(node, None)
                } else {
                    // Non-interactive field → treat as text
                    let text = self.field_display_text(node);
                    if text.is_empty() {
                        None
                    } else {
                        Some(StructuredNode::Paragraph(ParagraphNode {
                            content: InlineText::plain(text),
                        }))
                    }
                }
            }
        }
    }

    /// Convert a Field group (wrapping a single field leaf) to FieldNode.
    fn convert_field_group(
        &self,
        group_idx: usize,
        label: Option<InlineText>,
    ) -> Option<StructuredNode> {
        let nodes = self.doc.collect_nodes(group_idx);
        let field_node = nodes
            .into_iter()
            .find(|n| matches!(n.kind, FlattenedNodeKind::Field { .. }))?;

        // Non-interactive → text
        if !self.is_interactive(field_node) {
            let text = self.field_display_text(field_node);
            if text.is_empty() {
                return None;
            }
            return Some(StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain(text),
            }));
        }

        self.build_field_node(field_node, label)
    }

    /// Convert a RadioButtonGroup to a single FieldNode with Radio type.
    fn convert_radio_button_group(&self, group_idx: usize) -> Option<StructuredNode> {
        let group = self.doc.get_group(group_idx)?;

        // Collect all radio button labels as options
        let mut options = Vec::new();
        let mut field_names = Vec::new();
        let mut selected_value: Option<String> = None;
        let mut excl_group_som_path: Option<SomPath> = None;

        for &child_idx in &group.children {
            if let Some(child_group) = self.doc.get_group(child_idx)
                && let GroupKind::RadioButton { field: _, label } = &child_group.kind
            {
                // Get label text
                if let Some(&label_group_idx) = child_group.children.get(*label) {
                    let label_text = self.doc.get_text_content(label_group_idx);
                    options.push(label_text.clone());
                }

                // Get field for checking selected state and collecting names
                let nodes = self.doc.collect_nodes(child_idx);
                for node in nodes {
                    if let FlattenedNodeKind::Field {
                        is_checked, name, ..
                    } = &node.kind
                    {
                        field_names.push(name.clone());
                        if *is_checked == Some(true) {
                            selected_value = options.last().cloned();
                        }
                        // Extract ExclGroupSomPath from first field's hints
                        if excl_group_som_path.is_none() {
                            for hint in &node.hints {
                                if let Hint::ExclGroupSomPath(path) = hint {
                                    excl_group_som_path = Some(path.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        if field_names.is_empty() {
            return None;
        }

        // Use the exclGroup's SOM path as the field name, panic if missing
        let som_path = excl_group_som_path
            .expect("Radio button field must have ExclGroupSomPath hint. Field names found: {:?}");
        let name = FieldId::from_som_path(&som_path);

        // Build NameValue options from field_names and options (labels)
        let name_values: Vec<NameValue> = field_names
            .into_iter()
            .zip(options)
            .map(|(field_name, label)| NameValue {
                name: TranslatableString::Plain(label),
                value: InputValue::Text(field_name),
            })
            .collect();

        Some(StructuredNode::Field(FieldNode {
            name,
            som_path: Some(som_path),
            label: None, // Radio groups typically have options as labels
            input_type: FieldType::Radio {
                options: name_values,
            },
            value: selected_value.map(InputValue::Text),
            placeholder: None,
        }))
    }

    /// Convert an ExclGroup to a single FieldNode with Radio type.
    fn convert_excl_group(
        &self,
        group_idx: usize,
        selected_value: Option<String>,
    ) -> Option<StructuredNode> {
        let nodes = self.doc.collect_nodes(group_idx);

        // Collect all field labels/values as options
        let mut options = Vec::new();
        let mut first_field_node: Option<&FlattenedNode> = None;

        for node in &nodes {
            if let FlattenedNodeKind::Field { label, .. } = &node.kind {
                if !label.is_empty() {
                    options.push(label.clone());
                }
                if first_field_node.is_none() {
                    first_field_node = Some(node);
                }
            }
        }

        let field_node = first_field_node?;

        // Build NameValue options from labels (using label as both name and value)
        let name_values: Vec<NameValue> = options
            .into_iter()
            .map(|label| NameValue {
                name: TranslatableString::Plain(label.clone()),
                value: InputValue::Text(label),
            })
            .collect();

        Some(StructuredNode::Field(FieldNode {
            name: self.get_field_id(field_node),
            som_path: self.get_som_path(field_node).cloned(),
            label: None,
            input_type: FieldType::Radio {
                options: name_values,
            },
            value: selected_value.map(InputValue::Text),
            placeholder: None,
        }))
    }

    /// Convert a DateField group.
    fn convert_date_field(&self, group_idx: usize) -> Option<StructuredNode> {
        let nodes = self.doc.collect_nodes(group_idx);
        let first_field = nodes
            .iter()
            .find(|n| matches!(n.kind, FlattenedNodeKind::Field { .. }))?;

        // Concatenate values from all date component fields
        let value_parts: Vec<String> = nodes
            .iter()
            .filter_map(|n| {
                if let FlattenedNodeKind::Field { value, .. } = &n.kind
                    && !value.is_empty()
                {
                    return Some(value.clone());
                }
                None
            })
            .collect();

        let value = if value_parts.is_empty() {
            None
        } else {
            Some(InputValue::Text(value_parts.join(".")))
        };

        Some(StructuredNode::Field(FieldNode {
            name: self.get_field_id(first_field),
            som_path: self.get_som_path(first_field).cloned(),
            label: None, // Label typically attached by LabeledField wrapper
            input_type: FieldType::Date,
            value,
            placeholder: None,
        }))
    }

    /// Convert an InlineDateField to a FieldNode with Date type.
    /// If suffix_text is present, wraps the field and suffix paragraph in a GroupNode.
    fn convert_inline_date_field(
        &self,
        label_text: &str,
        suffix_text: &Option<String>,
        generated_name: &str,
    ) -> Option<StructuredNode> {
        let som_path = SomPath::new(generated_name);
        let field_node = StructuredNode::Field(FieldNode {
            name: FieldId::from_som_path(&som_path),
            som_path: Some(som_path),
            label: Some(InlineText::plain(label_text.to_string())),
            input_type: FieldType::Date,
            value: None,
            placeholder: None,
        });

        // If there's suffix text, emit it as a trailing paragraph
        if let Some(suffix) = suffix_text
            && !suffix.is_empty()
        {
            let suffix_paragraph = StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain(suffix.clone()),
            });
            return Some(StructuredNode::Group(GroupNode {
                children: vec![field_node, suffix_paragraph],
            }));
        }

        Some(field_node)
    }

    /// Build a FieldNode from a FlattenedNode.
    fn build_field_node(
        &self,
        node: &FlattenedNode,
        label: Option<InlineText>,
    ) -> Option<StructuredNode> {
        let FlattenedNodeKind::Field { name, value, .. } = &node.kind else {
            return None;
        };

        let field_type = self.determine_field_type(node);
        let input_value = self.parse_input_value(value, &field_type);

        // Use the full SOM path as the field name so it matches the condition
        // field_name in conditionals (consistent with radio buttons which use
        // the ExclGroupSomPath).
        let field_som_path = self.get_som_path(node)
            .cloned()
            .unwrap_or_else(|| SomPath::new(name.clone()));

        Some(StructuredNode::Field(FieldNode {
            name: FieldId::from_som_path(&field_som_path),
            som_path: Some(field_som_path),
            label,
            input_type: field_type,
            value: input_value,
            placeholder: self.get_placeholder(node).map(TranslatableString::Plain),
        }))
    }

    /// Determine FieldType from widget hints.
    fn determine_field_type(&self, node: &FlattenedNode) -> FieldType {
        // Check for widget type hint
        for hint in &node.hints {
            if let Hint::WidgetType(widget) = hint {
                return match widget {
                    WidgetKind::Text => self.text_field_type(node),
                    WidgetKind::TextArea => FieldType::Text {
                        regex: self.get_format_pattern(node),
                        max_length: self.get_max_length(node),
                        min_length: None,
                    },
                    WidgetKind::Checkbox => FieldType::Bool,
                    WidgetKind::Radio => FieldType::Radio { options: vec![] },
                    WidgetKind::Dropdown => {
                        let options = self.get_dropdown_options(node);
                        FieldType::Select { options }
                    }
                    WidgetKind::Date | WidgetKind::DateTime => FieldType::Date,
                    WidgetKind::Time => FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    WidgetKind::Numeric => {
                        // TODO: extract min/max/step from validation
                        FieldType::Number {
                            min: None,
                            max: None,
                            step: None,
                        }
                    }
                    WidgetKind::Password => FieldType::Text {
                        regex: None,
                        max_length: self.get_max_length(node),
                        min_length: None,
                    },
                    _ => self.text_field_type(node),
                };
            }
        }

        // Default to text
        self.text_field_type(node)
    }

    /// Create a Text field type with constraints from hints.
    fn text_field_type(&self, node: &FlattenedNode) -> FieldType {
        FieldType::Text {
            regex: self.get_format_pattern(node),
            max_length: self.get_max_length(node),
            min_length: None,
        }
    }

    /// Parse an input value based on field type.
    fn parse_input_value(&self, value: &str, field_type: &FieldType) -> Option<InputValue> {
        if value.is_empty() {
            return None;
        }

        Some(match field_type {
            FieldType::Bool => InputValue::Bool(value == "on" || value == "1" || value == "true"),
            FieldType::Radio { .. } => InputValue::Text(value.to_string()),
            FieldType::Select { .. } => InputValue::Text(value.to_string()),
            FieldType::Date => InputValue::Text(value.to_string()),
            FieldType::Number { .. } => {
                // Try to parse as decimal, fallback to text
                if let Ok(num) = value.parse() {
                    InputValue::Number(num)
                } else {
                    InputValue::Text(value.to_string())
                }
            }
            FieldType::Email => InputValue::Text(value.to_string()),
            FieldType::Tel => InputValue::Text(value.to_string()),
            FieldType::Text { .. } => InputValue::Text(value.to_string()),
        })
    }

    // ========================================================================
    // Helper: Extract hints
    // ========================================================================

    fn is_interactive(&self, node: &FlattenedNode) -> bool {
        for hint in &node.hints {
            if let Hint::FieldBehavior { access, .. } = hint {
                return access.is_interactive();
            }
        }
        true // Default to interactive
    }

    fn get_format_pattern(&self, node: &FlattenedNode) -> Option<String> {
        for hint in &node.hints {
            if let Hint::Validation { format_pattern, .. } = hint {
                return format_pattern.clone();
            }
        }
        None
    }

    /// Extract dropdown options from a Hint::Dropdown, mapping to NameValue pairs.
    fn get_dropdown_options(&self, node: &FlattenedNode) -> Vec<NameValue> {
        for hint in &node.hints {
            if let Hint::Dropdown { options, .. } = hint {
                return options
                    .iter()
                    .map(|(display, save)| NameValue {
                        name: TranslatableString::Plain(display.clone()),
                        value: InputValue::Text(save.clone()),
                    })
                    .collect();
            }
        }
        vec![]
    }

    fn get_max_length(&self, node: &FlattenedNode) -> Option<usize> {
        for hint in &node.hints {
            if let Hint::FieldBehavior { max_length, .. } = hint {
                return max_length.map(|n| n as usize);
            }
        }
        None
    }

    fn get_placeholder(&self, node: &FlattenedNode) -> Option<String> {
        for hint in &node.hints {
            if let Hint::Accessibility { tool_tip, .. } = hint {
                return tool_tip.clone();
            }
        }
        None
    }

    /// Extract the SOM path from a field's hints.
    fn get_som_path<'n>(&self, node: &'n FlattenedNode) -> Option<&'n SomPath> {
        for hint in &node.hints {
            if let Hint::SomPath(path) = hint {
                return Some(path);
            }
        }
        None
    }

    /// Get a FieldId from a FlattenedNode's SOM path hint, falling back to node name.
    fn get_field_id(&self, node: &FlattenedNode) -> FieldId {
        if let Some(som_path) = self.get_som_path(node) {
            FieldId::from_som_path(som_path)
        } else if let FlattenedNodeKind::Field { name, .. } = &node.kind {
            FieldId::from_som_path(&SomPath::new(name.clone()))
        } else {
            FieldId::from_som_path(&SomPath::new(""))
        }
    }

    /// Get the display text for a non-interactive field.
    /// For readonly fields converted to paragraphs, only show the computed value (rawValue),
    /// not the field's label. If there's no value, the field shouldn't produce text output.
    fn field_display_text(&self, node: &FlattenedNode) -> String {
        if let FlattenedNodeKind::Field { value, .. } = &node.kind {
            // Only return actual computed value, not the label
            // Readonly fields without a value should not produce output
            value.clone()
        } else {
            String::new()
        }
    }

    // ========================================================================
    // Helper: InlineText extraction
    // ========================================================================

    /// Extract InlineText from a group (recursively collecting all text).
    fn extract_inline_text(&self, group_idx: usize) -> InlineText {
        let nodes = self.doc.collect_nodes(group_idx);
        let mut result = Vec::new();

        for node in nodes {
            self.append_inline_nodes_from_node(node, &mut result);
        }

        InlineText::new(result)
    }

    /// Build InlineText from a single FlattenedNode.
    fn build_inline_text_from_node(&self, node: &FlattenedNode) -> InlineText {
        let mut result = Vec::new();
        self.append_inline_nodes_from_node(node, &mut result);
        InlineText::new(result)
    }

    /// Append InlineNodes from a FlattenedNode to the result vec.
    fn append_inline_nodes_from_node(&self, node: &FlattenedNode, result: &mut Vec<InlineNode>) {
        // Check for rich text content
        for hint in &node.hints {
            if let Hint::RichContent(rich_text) = hint {
                // Only use rich text if it has actual text content (non-empty runs)
                let has_content = rich_text
                    .paragraphs
                    .iter()
                    .any(|p| p.runs.iter().any(|r| !r.text.is_empty()));
                if has_content {
                    self.append_inline_nodes_from_rich_text(rich_text, result);
                    return;
                }
            }
        }

        // Plain text fallback
        if let FlattenedNodeKind::Text { content, .. } = &node.kind
            && !content.is_empty()
        {
            result.push(InlineNode::Text(content.clone()));
        }
    }

    /// Convert RichText to multiple ParagraphNodes (one per RichParagraph).
    /// Skips empty paragraphs.
    fn convert_rich_text_to_paragraph_nodes(&self, rich_text: &RichText) -> Vec<StructuredNode> {
        rich_text
            .paragraphs
            .iter()
            .filter(|para| !para.is_empty) // Skip empty paragraphs
            .filter_map(|para| {
                let mut inline_nodes = Vec::new();
                for run in &para.runs {
                    if !run.text.is_empty() {
                        let inline_node = self.rich_run_to_inline_node(run);
                        inline_nodes.push(inline_node);
                    }
                }
                if inline_nodes.is_empty() {
                    None
                } else {
                    Some(StructuredNode::Paragraph(ParagraphNode {
                        content: InlineText::new(inline_nodes),
                    }))
                }
            })
            .collect()
    }

    /// Convert RichText to InlineNodes.
    fn append_inline_nodes_from_rich_text(
        &self,
        rich_text: &RichText,
        result: &mut Vec<InlineNode>,
    ) {
        for para in &rich_text.paragraphs {
            for run in &para.runs {
                let inline_node = self.rich_run_to_inline_node(run);
                result.push(inline_node);
            }
        }
    }

    /// Convert a RichRun to an InlineNode with appropriate styling wrappers.
    fn rich_run_to_inline_node(&self, run: &RichRun) -> InlineNode {
        let mut node = InlineNode::Text(run.text.clone());

        // Wrap with emphasis if italic
        if run.italic {
            node = InlineNode::Emphasis(Box::new(node));
        }

        // Wrap with strong if bold
        if run.bold {
            node = InlineNode::Strong(Box::new(node));
        }

        node
    }
}

#[cfg(test)]
mod tests {
    // TODO: Add tests when we have test fixtures
}
