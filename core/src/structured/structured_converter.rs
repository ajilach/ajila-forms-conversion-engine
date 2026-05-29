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
//! - `CheckboxGroup` → `FieldNode` with `FieldType::CheckboxGroup`
//! - `DateField` → `FieldNode` with `FieldType::Date`
//! - `Field` → `FieldNode` (unlabeled)
//! - `RepeatableSection` → `RepeatableNode`
//! - `Section` / `Unknown` → `GroupNode`
//! - Leaf (Text) → `ParagraphNode`
//! - Leaf (Field, non-interactive) → `ParagraphNode` (readonly → text)
//! - Leaf (Field, interactive) → `FieldNode`
//!
//! Header, Footer, and Background groups are excluded from output.

use crate::document::{Document, GroupKind};
use crate::flattened::{Bounds, FlattenedNode, FlattenedNodeKind, RichRun, RichText, WidgetKind};
use crate::structured::{
    ConditionalNode, FieldCondition, FieldId, FieldNode, FieldType, FootnoteNode, GroupNode,
    HeadingLevel, HeadingNode, InlineNode, InlineText, InputValue, ListItem, ListNode, NameValue,
    ParagraphNode, RepeatableNode, StructuredNode, TranslatableString, TranslatedText,
};
use crate::xfa::scripting::SomPath;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

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
        | StructuredNode::Footnote(_)
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

    // Ordered: parenthesized markers — (1), (a), (i), (ii), etc.
    let bytes = trimmed.as_bytes();
    if !bytes.is_empty() && bytes[0] == b'(' {
        if let Some(close) = bytes.iter().position(|&b| b == b')') {
            if close >= 2 {
                let content = &trimmed[1..close];
                let content_bytes = content.as_bytes();
                let is_valid = content_bytes.iter().all(|b| b.is_ascii_digit())
                    || (content_bytes.len() == 1 && content_bytes[0].is_ascii_alphabetic())
                    || {
                        let roman_chars = b"ivxlcdm";
                        content_bytes
                            .iter()
                            .all(|b| roman_chars.contains(&b.to_ascii_lowercase()))
                    };
                if is_valid {
                    let after = &trimmed[close + 1..];
                    return Some(after.trim_start().to_string());
                }
            }
        }
    }

    // Ordered: digits followed by . or )
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
    convert_with_language(doc, "default")
}

/// Convert a Document to a list of StructuredNodes with a specific language key.
pub fn convert_with_language(doc: &Document, language: &str) -> Vec<StructuredNode> {
    let converter = Converter {
        doc,
        language: language.to_string(),
    };

    // Get roots and sort by reading order (y first, then x)
    let mut roots: Vec<usize> = doc.roots();
    roots.sort_by(|&a, &b| {
        let bounds_a = doc.get_bounds(a);
        let bounds_b = doc.get_bounds(b);
        compare_bounds_reading_order(bounds_a, bounds_b)
    });

    let mut content: Vec<StructuredNode> = roots
        .into_iter()
        .filter_map(|idx| converter.convert_group(idx))
        .collect();

    inherit_heading_labels_for_radios(&mut content);
    move_number_prefixes_to_headings(&mut content);

    content
}

/// Convert a Document to a DocumentEnvelope with context.
/// This wraps the structured nodes with the context metadata.
pub fn convert_with_context(
    doc: &Document,
    context: crate::context::Context,
) -> crate::structured::DocumentEnvelope {
    let language = context.language().to_string();
    let content = convert_with_language(doc, &language);
    crate::structured::DocumentEnvelope {
        context,
        content,
        state_count: 1,
    }
}

/// If a radio button field has no label and the immediately preceding sibling
/// is a heading, copy the heading text as the radio field's label.
/// The heading stays in place — only the text is copied.
/// Recurses into Groups, Repeatables, Conditionals, GridLayouts, and Tables.
fn inherit_heading_labels_for_radios(nodes: &mut [StructuredNode]) {
    // First pass: copy heading text into immediately following unlabeled radios
    for i in 1..nodes.len() {
        let is_unlabeled_radio = matches!(
            &nodes[i],
            StructuredNode::Field(f)
                if f.label.is_none() && matches!(f.input_type, FieldType::Radio { .. })
        );
        if !is_unlabeled_radio {
            continue;
        }
        let is_heading = matches!(&nodes[i - 1], StructuredNode::Heading(_));
        if !is_heading {
            continue;
        }
        let StructuredNode::Heading(h) = &nodes[i - 1] else {
            unreachable!();
        };
        let label = h.content.to_plain();
        let StructuredNode::Field(f) = &mut nodes[i] else {
            unreachable!();
        };
        f.label = Some(label);
    }

    // Second pass: recurse into containers
    for node in nodes.iter_mut() {
        match node {
            StructuredNode::Group(g) => {
                inherit_heading_labels_for_radios(&mut g.children);
            }
            StructuredNode::Repeatable(r) => {
                if let StructuredNode::Group(g) = r.item.as_mut() {
                    inherit_heading_labels_for_radios(&mut g.children);
                }
            }
            StructuredNode::Conditional(c) => {
                if let StructuredNode::Group(g) = c.content.as_mut() {
                    inherit_heading_labels_for_radios(&mut g.children);
                }
            }
            StructuredNode::GridLayout(gl) => {
                let mut children: Vec<&mut StructuredNode> =
                    gl.elements.iter_mut().map(|e| &mut e.node).collect();
                // Check pairs within grid elements
                for i in 1..children.len() {
                    let (left, right) = children.split_at_mut(i);
                    let prev = left.last_mut().unwrap();
                    let curr = right.first_mut().unwrap();
                    let is_unlabeled_radio = matches!(
                        curr,
                        StructuredNode::Field(f)
                            if f.label.is_none()
                                && matches!(f.input_type, FieldType::Radio { .. })
                    );
                    if is_unlabeled_radio {
                        if let StructuredNode::Heading(h) = &**prev {
                            let label = h.content.to_plain();
                            if let StructuredNode::Field(f) = &mut **curr {
                                f.label = Some(label);
                            }
                        }
                    }
                }
                // Recurse into each grid element
                for elem in &mut gl.elements {
                    if let StructuredNode::Group(g) = &mut elem.node {
                        inherit_heading_labels_for_radios(&mut g.children);
                    }
                }
            }
            StructuredNode::Table(t) => {
                for row in &mut t.rows {
                    inherit_heading_labels_for_radios(&mut row.cells);
                }
                if let Some(header) = &mut t.header {
                    inherit_heading_labels_for_radios(&mut header.cells);
                }
            }
            _ => {}
        }
    }
}

/// Move a numeric prefix (e.g. "2. ") from a Paragraph to the immediately
/// preceding Heading when the heading does not already have a number prefix.
///
/// This fixes XFA forms where a narrow "marker column" draw overlaps with a
/// wide "content column" draw, and paragraph-height misalignment causes the
/// section number to land on the paragraph line rather than the heading line.
/// The overlapping-text-block merger then prepends the number to the paragraph
/// text instead of the heading text.
///
/// The function recurses into Groups, Conditionals, Repeatables, etc.
fn move_number_prefixes_to_headings(nodes: &mut Vec<StructuredNode>) {
    // Process siblings: look for Heading followed by Paragraph starting with "N. "
    let mut i = 1;
    while i < nodes.len() {
        let should_move = {
            let is_heading = matches!(&nodes[i - 1], StructuredNode::Heading(_));
            if !is_heading {
                false
            } else if let StructuredNode::Paragraph(p) = &nodes[i] {
                extract_numeric_prefix(&p.content.as_plain_text()).is_some()
            } else {
                false
            }
        };

        if should_move {
            let heading_already_numbered = if let StructuredNode::Heading(h) = &nodes[i - 1] {
                let text = h.content.as_plain_text();
                text.trim().starts_with(|c: char| c.is_ascii_digit())
            } else {
                false
            };

            if !heading_already_numbered {
                // Extract the prefix from the paragraph (use first available language)
                let para_text = if let StructuredNode::Paragraph(p) = &nodes[i] {
                    p.content.as_plain_text()
                } else {
                    String::new()
                };

                if let Some((prefix, _remaining)) = extract_numeric_prefix(&para_text) {
                    // Prepend prefix to heading content (all languages)
                    if let StructuredNode::Heading(h) = &mut nodes[i - 1] {
                        for (_lang, text) in h.content.0.iter_mut() {
                            let heading_text = text.as_plain_text();
                            *text = InlineText(vec![InlineNode::Text(format!(
                                "{prefix}{heading_text}"
                            ))]);
                        }
                    }

                    // Strip prefix from paragraph content (all languages)
                    if let StructuredNode::Paragraph(p) = &mut nodes[i] {
                        for (_lang, text) in p.content.0.iter_mut() {
                            strip_numeric_prefix_from_inline_text(text);
                        }
                        // If the paragraph is now empty, remove it
                        if p.content.is_empty() {
                            nodes.remove(i);
                            continue;
                        }
                    }
                }
            }
        }

        i += 1;
    }

    // Recurse into container nodes
    for node in nodes.iter_mut() {
        match node {
            StructuredNode::Group(g) => {
                move_number_prefixes_to_headings(&mut g.children);
            }
            StructuredNode::Conditional(c) => {
                if let StructuredNode::Group(g) = c.content.as_mut() {
                    move_number_prefixes_to_headings(&mut g.children);
                }
            }
            StructuredNode::Repeatable(r) => {
                if let StructuredNode::Group(g) = r.item.as_mut() {
                    move_number_prefixes_to_headings(&mut g.children);
                }
            }
            StructuredNode::GridLayout(gl) => {
                let mut children: Vec<StructuredNode> =
                    gl.elements.iter().map(|e| e.node.clone()).collect();
                move_number_prefixes_to_headings(&mut children);
                for (elem, new_node) in gl.elements.iter_mut().zip(children) {
                    elem.node = new_node;
                }
            }
            _ => {}
        }
    }
}

/// Extract a numeric prefix like "2. " from the start of text.
/// Returns the prefix (including trailing space) and the remaining text.
fn extract_numeric_prefix(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return None;
    }

    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }

    // Must be followed by '.' or ')'
    if i >= bytes.len() || (bytes[i] != b'.' && bytes[i] != b')') {
        return None;
    }
    i += 1;

    // Consume trailing whitespace
    let after = &trimmed[i..];
    let ws_len = after.len() - after.trim_start().len();

    let prefix = trimmed[..i + ws_len].to_string();
    let remaining = trimmed[i + ws_len..].to_string();

    Some((prefix, remaining))
}

/// Strip a numeric prefix ("N. ") from the first text node in an InlineText.
fn strip_numeric_prefix_from_inline_text(text: &mut InlineText) {
    if text.0.is_empty() {
        return;
    }

    if let Some(node) = text.0.first_mut() {
        match node {
            InlineNode::Text(s) => {
                if let Some((_prefix, remaining)) = extract_numeric_prefix(s) {
                    *s = remaining;
                }
            }
            InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
                if let InlineNode::Text(s) = inner.as_mut() {
                    if let Some((_prefix, remaining)) = extract_numeric_prefix(s) {
                        *s = remaining;
                    }
                }
            }
            _ => {}
        }
    }

    // Remove empty leading text nodes
    while !text.0.is_empty() {
        let is_empty = match &text.0[0] {
            InlineNode::Text(s) => s.trim().is_empty(),
            InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
                if let InlineNode::Text(s) = inner.as_ref() {
                    s.trim().is_empty()
                } else {
                    false
                }
            }
            _ => false,
        };
        if is_empty {
            text.0.remove(0);
        } else {
            break;
        }
    }
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
            // Quantize y into bands so that items on roughly the same line
            // compare equal on the primary axis.  Rounding to a fixed grid
            // (instead of a pairwise threshold) guarantees transitivity.
            let band = rust_decimal::Decimal::new(40, 1); // 4.0pt bands
            let quantize =
                |y: rust_decimal::Decimal| -> rust_decimal::Decimal { (y / band).round() * band };
            let ya = quantize(a.y);
            let yb = quantize(b.y);
            ya.cmp(&yb).then_with(|| a.x.cmp(&b.x))
        }
    }
}

struct Converter<'a, 'b> {
    doc: &'a Document<'b>,
    language: String,
}

impl<'a, 'b> Converter<'a, 'b> {
    /// Wrap an InlineText into a TranslatedText with the converter's language.
    fn translated(&self, text: InlineText) -> TranslatedText {
        TranslatedText::single(&self.language, text)
    }

    /// Convert a single group to a StructuredNode.
    fn convert_group(&self, group_idx: usize) -> Option<StructuredNode> {
        let group = self.doc.get_group(group_idx)?;

        match &group.kind {
            // Skip header/footer/background (master page content)
            GroupKind::Header | GroupKind::Footer | GroupKind::Background => None,

            // Footnote → FootnoteNode
            GroupKind::Footnote => {
                let text = self.extract_inline_text(group_idx);
                let som_path = self.extract_group_som_path(group_idx);
                let source_name = self.extract_group_source_name(group_idx);
                // Parse marker from leading digits in the plain text
                let plain = text.as_plain_text();
                let marker = {
                    let digits: String = plain
                        .trim_start()
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if digits.is_empty() {
                        None
                    } else {
                        Some(digits)
                    }
                };
                Some(StructuredNode::Footnote(FootnoteNode {
                    content: self.translated(text),
                    marker,
                    som_path,
                    source_name,
                }))
            }

            // InlineField → Paragraph(before) + Field("UNKNOWN") + Paragraph(after)
            GroupKind::InlineField {
                before,
                field,
                after,
            } => self.convert_inline_field(group_idx, before, *field, after),

            // Skip non-printable elements (relevant="-print")
            GroupKind::NoPrint => None,

            // Heading → HeadingNode
            GroupKind::Heading { level } => {
                let text = self.extract_inline_text(group_idx).to_plain();
                let som_path = self.extract_group_som_path(group_idx);
                let source_name = self.extract_group_source_name(group_idx);
                Some(StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::from_u8(*level),
                    content: self.translated(text),
                    som_path,
                    source_name,
                }))
            }

            // TextBlock / Paragraph → ParagraphNode (or multiple if rich text with multiple paragraphs)
            GroupKind::TextBlock | GroupKind::Paragraph => {
                let som_path = self.extract_group_som_path(group_idx);
                let source_name = self.extract_group_source_name(group_idx);

                // Check if this is a single-node group with rich text containing multiple paragraphs
                let nodes = self.doc.collect_nodes(group_idx);
                if nodes.len() == 1 {
                    let node = nodes[0];
                    if let Some(rich_text) = node.rich_text() {
                        let paragraphs = self.convert_rich_text_to_paragraph_nodes(
                            rich_text,
                            som_path.clone(),
                            source_name.clone(),
                        );
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

                // Fallback: extract as single inline text
                let text = self.extract_inline_text(group_idx);
                if text.is_empty() {
                    None
                } else {
                    let source_name = self.extract_group_source_name(group_idx);
                    Some(StructuredNode::Paragraph(ParagraphNode {
                        content: self.translated(text),
                        som_path,
                        source_name,
                    }))
                }
            }

            // LabeledField → FieldNode with label
            GroupKind::LabeledField { label, field } => {
                let label_group = group.children.get(*label).copied()?;
                let field_group = group.children.get(*field).copied()?;

                let label_text = self.extract_inline_text(label_group).to_plain();

                // Dispatch based on the kind of the wrapped field group
                let field_kind = self.doc.get_group(field_group).map(|g| g.kind.clone());
                match field_kind {
                    Some(GroupKind::RadioButtonGroup) => {
                        self.convert_radio_button_group(field_group, Some(label_text))
                    }
                    Some(GroupKind::ExclGroup { selected_value }) => {
                        self.convert_excl_group(field_group, selected_value, Some(label_text))
                    }
                    Some(GroupKind::CheckboxGroup) => {
                        self.convert_checkbox_group(field_group, Some(label_text))
                    }
                    _ => self.convert_field_group(field_group, Some(label_text)),
                }
            }

            // RadioButton → FieldNode (single option, usually wrapped in RadioButtonGroup)
            GroupKind::RadioButton { field, label } => {
                let label_group = group.children.get(*label).copied()?;
                let field_group = group.children.get(*field).copied()?;

                let label_text = self.extract_inline_text(label_group).to_plain();
                self.convert_field_group(field_group, Some(label_text))
            }

            // Checkbox → FieldNode (boolean field with label)
            GroupKind::Checkbox { field, label } => {
                let label_group = group.children.get(*label).copied()?;
                let field_group = group.children.get(*field).copied()?;

                let label_text = self.extract_inline_text(label_group).to_plain();
                self.convert_field_group(field_group, Some(label_text))
            }

            // RadioButtonGroup → FieldNode with Radio type
            GroupKind::RadioButtonGroup => self.convert_radio_button_group(group_idx, None),

            // CheckboxGroup → GroupNode wrapping individual checkbox fields
            GroupKind::CheckboxGroup => self.convert_checkbox_group(group_idx, None),

            // ExclGroup → FieldNode with Radio type
            GroupKind::ExclGroup { selected_value } => {
                self.convert_excl_group(group_idx, selected_value.clone(), None)
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
                is_user_repeatable,
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

                // Script-managed sections (no buttons) → plain Group, not Repeatable
                if !is_user_repeatable {
                    return Some(item);
                }

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

            // ColumnSection → GroupNode with children in preserved order (left then right).
            // Children must NOT be re-sorted by position because the right column may start
            // at a lower y-coordinate than the left column.
            GroupKind::ColumnSection => {
                let children = self.convert_children_ordered(group_idx);
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

            // CheckboxContent → ConditionalNode wrapping the child content
            GroupKind::CheckboxContent { checkbox_som_path } => {
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
                        field_name: FieldId::from_som_path(checkbox_som_path),
                        value: InputValue::Bool(true),
                    },
                    content: Box::new(content),
                }))
            }

            // SelectionInlineField → ConditionalNode wrapping a labeled FieldNode
            GroupKind::SelectionInlineField {
                condition_som_path,
                option_field_name,
                label_text,
                field,
            } => {
                let group = self.doc.get_group(group_idx)?;
                let field_group = *group.children.get(*field)?;

                let label = InlineText::plain(label_text.clone());
                let field_node = self.convert_field_group(field_group, Some(label))?;

                let condition = if let Some(option_name) = option_field_name {
                    FieldCondition {
                        field_name: FieldId::from_som_path(condition_som_path),
                        value: InputValue::Text(option_name.clone()),
                    }
                } else {
                    FieldCondition {
                        field_name: FieldId::from_som_path(condition_som_path),
                        value: InputValue::Bool(true),
                    }
                };

                Some(StructuredNode::Conditional(ConditionalNode {
                    condition,
                    content: Box::new(field_node),
                }))
            }

            // List → ListNode
            GroupKind::List { list_style } => {
                let group = self.doc.get_group(group_idx)?;
                let mut items: Vec<ListItem> = Vec::new();
                // Sort children by reading order
                let mut children = group.children.clone();
                children.sort_by(|&a, &b| {
                    let bounds_a = self.doc.get_bounds(a);
                    let bounds_b = self.doc.get_bounds(b);
                    compare_bounds_reading_order(bounds_a, bounds_b)
                });
                for &child_idx in &children {
                    // Check if this child is a nested List (sublist)
                    if let Some(child_group) = self.doc.get_group(child_idx) {
                        if let GroupKind::List { list_style: _ } = &child_group.kind {
                            // Convert the sublist recursively
                            if let Some(StructuredNode::List(sub_list)) =
                                self.convert_group(child_idx)
                            {
                                // Attach as sublist of the previous item
                                if let Some(last_item) = items.last_mut() {
                                    last_item.sublist = Some(Box::new(sub_list));
                                }
                            }
                            continue;
                        }
                    }
                    let text = self.extract_inline_text(child_idx);
                    if !text.is_empty() {
                        // Strip list marker prefix from the item text
                        let stripped = strip_list_marker_from_inline_text(text);
                        if !stripped.is_empty() {
                            items.push(ListItem::simple(self.translated(stripped)));
                        }
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

            // Table → TableNode
            GroupKind::Table {
                columns,
                has_header,
            } => {
                use crate::structured::{TableHeader, TableNode, TableRow};

                let group = self.doc.get_group(group_idx)?;
                let children = &group.children;

                if children.is_empty() || *columns == 0 {
                    return None;
                }

                let num_rows = children.len() / columns;
                if num_rows == 0 {
                    return None;
                }

                let mut rows: Vec<TableRow> = Vec::new();
                let mut header: Option<TableHeader> = None;

                for row_idx in 0..num_rows {
                    let start = row_idx * columns;
                    let end = start + columns;
                    let row_children = &children[start..end];

                    let cells: Vec<StructuredNode> = row_children
                        .iter()
                        .map(|&child_idx| {
                            let text = self.extract_inline_text(child_idx);
                            StructuredNode::Paragraph(ParagraphNode {
                                content: self.translated(text),
                                som_path: None,
                                source_name: None,
                            })
                        })
                        .collect();

                    if row_idx == 0 && *has_header {
                        header = Some(TableHeader { cells });
                    } else {
                        rows.push(TableRow { cells });
                    }
                }

                Some(StructuredNode::Table(TableNode {
                    header,
                    rows,
                    caption: None,
                }))
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

        let mut result: Vec<StructuredNode> = children
            .iter()
            .filter_map(|&child_idx| self.convert_group(child_idx))
            .collect();

        inherit_heading_labels_for_radios(&mut result);

        result
    }

    /// Convert all children of a group in their stored order (no re-sorting).
    /// Used for ColumnSection groups where left-column always precedes right-column.
    fn convert_children_ordered(&self, group_idx: usize) -> Vec<StructuredNode> {
        let Some(group) = self.doc.get_group(group_idx) else {
            return vec![];
        };

        let result: Vec<StructuredNode> = group
            .children
            .iter()
            .filter_map(|&child_idx| self.convert_group(child_idx))
            .collect();

        result
    }

    /// Convert a leaf node (text or field).
    fn convert_leaf(&self, node_index: usize) -> Option<StructuredNode> {
        let node = self.doc.get_node(node_index)?;

        match &node.kind {
            FlattenedNodeKind::Text {
                content,
                source_name,
                ..
            } => {
                if content.trim().is_empty() {
                    None
                } else {
                    let som_path = node.som_path().cloned();
                    let source_name = source_name.clone();
                    // Check if this node has rich text with multiple paragraphs
                    if let Some(rich_text) = node.rich_text() {
                        let paragraphs = self.convert_rich_text_to_paragraph_nodes(
                            rich_text,
                            som_path.clone(),
                            source_name.clone(),
                        );
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
                    // No rich text or fallback - use plain text
                    let text = self.build_inline_text_from_node(node);
                    if text.is_empty() {
                        None
                    } else {
                        Some(StructuredNode::Paragraph(ParagraphNode {
                            content: self.translated(text),
                            som_path,
                            source_name,
                        }))
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
                            content: self.translated(InlineText::plain(text)),
                            som_path: node.som_path().cloned(),
                            source_name: None,
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
                content: self.translated(InlineText::plain(text)),
                som_path: field_node.som_path().cloned(),
                source_name: None,
            }));
        }

        self.build_field_node(field_node, label)
    }

    /// Convert a RadioButtonGroup to a single FieldNode with Radio type.
    fn convert_radio_button_group(
        &self,
        group_idx: usize,
        label: Option<InlineText>,
    ) -> Option<StructuredNode> {
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
                            if let Some(path) = node.excl_group_som_path() {
                                excl_group_som_path = Some(path.clone());
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
            label: label.map(|l| self.translated(l.to_plain())),
            input_type: FieldType::Radio {
                options: name_values,
            },
            value: selected_value.map(InputValue::Text),
            placeholder: None,
            required: false,
        }))
    }

    /// Convert a CheckboxGroup to a single FieldNode with CheckboxGroup type.
    ///
    /// Mirrors the radio-button-group pattern: each child Checkbox's label becomes
    /// an option name, and the field's SOM path becomes the option value.
    fn convert_checkbox_group(
        &self,
        group_idx: usize,
        label: Option<InlineText>,
    ) -> Option<StructuredNode> {
        let group = self.doc.get_group(group_idx)?;

        let mut options = Vec::new();
        let mut first_som_path: Option<SomPath> = None;
        let mut first_field_id: Option<FieldId> = None;

        for &child_idx in &group.children {
            if let Some(child_group) = self.doc.get_group(child_idx)
                && let GroupKind::Checkbox { field: _, label } = &child_group.kind
            {
                // Get label text
                let option_label = if let Some(&label_group_idx) = child_group.children.get(*label)
                {
                    self.doc.get_text_content(label_group_idx)
                } else {
                    String::new()
                };

                // Get field info
                let nodes = self.doc.collect_nodes(child_idx);
                for node in &nodes {
                    if let FlattenedNodeKind::Field { name, .. } = &node.kind {
                        if first_som_path.is_none() {
                            first_som_path = node.som_path().cloned();
                            first_field_id = Some(self.get_field_id(node));
                        }
                        options.push(NameValue {
                            name: TranslatableString::Plain(option_label.clone()),
                            value: InputValue::Text(name.clone()),
                        });
                        break;
                    }
                }
            }
        }

        if options.is_empty() {
            return None;
        }

        let field_id = first_field_id.unwrap_or_else(FieldId::random);

        Some(StructuredNode::Field(FieldNode {
            name: field_id,
            som_path: first_som_path,
            label: label.map(|l| self.translated(l.to_plain())),
            input_type: FieldType::CheckboxGroup { options },
            value: None,
            placeholder: None,
            required: false,
        }))
    }

    /// Convert an ExclGroup to a single FieldNode with Radio type.
    fn convert_excl_group(
        &self,
        group_idx: usize,
        selected_value: Option<String>,
        label: Option<InlineText>,
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
            label: label.map(|l| self.translated(l.to_plain())),
            input_type: FieldType::Radio {
                options: name_values,
            },
            value: selected_value.map(InputValue::Text),
            placeholder: None,
            required: false,
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
            required: false,
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
            label: Some(self.translated(InlineText::plain(label_text.to_string()))),
            input_type: FieldType::Date,
            value: None,
            placeholder: None,
            required: false,
        });

        // If there's suffix text, emit it as a trailing paragraph
        if let Some(suffix) = suffix_text
            && !suffix.is_empty()
        {
            let suffix_paragraph = StructuredNode::Paragraph(ParagraphNode {
                content: self.translated(InlineText::plain(suffix.clone())),
                som_path: None,
                source_name: None,
            });
            return Some(StructuredNode::Group(GroupNode {
                children: vec![field_node, suffix_paragraph],
            }));
        }

        Some(field_node)
    }

    /// Convert an InlineField group to structured nodes.
    ///
    /// Produces: Paragraph(before text) + Field("UNKNOWN" label) + Paragraph(after text),
    /// wrapped in a GroupNode if there are multiple result nodes.
    ///
    /// When a "before" text group contains multiple text nodes (e.g. from paragraph
    /// splitting during flattening), individual text nodes whose vertical position
    /// is below the field are treated as "after" text instead.
    fn convert_inline_field(
        &self,
        group_idx: usize,
        before: &[usize],
        field_child_idx: usize,
        after: &[usize],
    ) -> Option<StructuredNode> {
        let group = self.doc.get_group(group_idx)?;

        // Get the field bounds (used to classify text nodes as before/after)
        let field_group_idx = group.children.get(field_child_idx).copied();
        let field_bounds = field_group_idx.and_then(|idx| self.doc.get_bounds(idx));

        let mut before_nodes: Vec<StructuredNode> = Vec::new();
        let mut after_nodes: Vec<StructuredNode> = Vec::new();

        // Helper closure to classify and split a text group by position.
        // Returns (before_paragraphs, after_paragraphs).
        let split_by_position =
            |child_group_idx: usize, fb: &Bounds| -> (Vec<StructuredNode>, Vec<StructuredNode>) {
                let node_indices = self.doc.collect_node_indices(child_group_idx);
                let mut before = Vec::new();
                let mut after = Vec::new();

                if node_indices.len() > 1 {
                    // Multiple text nodes: classify each by position
                    for &ni in &node_indices {
                        if let Some(node) = self.doc.get_node(ni) {
                            let text = self.build_inline_text_from_node(node);
                            if text.is_empty() {
                                continue;
                            }
                            let node_bounds = node.bounds();

                            // For multi-node text groups, classify each node:
                            // 1. If on a different vertical line than the field, use y-position
                            // 2. If on the same line as the field, use x-position

                            let line_tolerance = Decimal::from(8); // same as InlineFieldDetector

                            // Check if a multi-line text node spans the field's
                            // vertical position.  When a text node is much taller
                            // than the field it wraps multiple lines; we need to
                            // split its content at the field boundary instead of
                            // assigning the whole node to "before" or "after".
                            let is_multiline_spanning_field = node_bounds.height
                                > fb.height * Decimal::from(2)
                                && fb.y >= node_bounds.y
                                && fb.y <= node_bounds.y + node_bounds.height;

                            if is_multiline_spanning_field {
                                if let Some((before_str, after_str)) =
                                    Self::split_text_at_field_position(node, fb)
                                {
                                    if !before_str.is_empty() {
                                        before.push(StructuredNode::Paragraph(ParagraphNode {
                                            content: self.translated(InlineText::plain(before_str)),
                                            som_path: None,
                                            source_name: None,
                                        }));
                                    }
                                    if !after_str.is_empty() {
                                        after.push(StructuredNode::Paragraph(ParagraphNode {
                                            content: self.translated(InlineText::plain(after_str)),
                                            som_path: None,
                                            source_name: None,
                                        }));
                                    }
                                    continue;
                                }
                                // Fall through to normal classification if splitting fails
                            }

                            let on_same_line = (node_bounds.y - fb.y).abs() < line_tolerance
                                || (node_bounds.y + node_bounds.height - fb.y - fb.height).abs()
                                    < line_tolerance;

                            let is_after = if on_same_line {
                                // Same line: compare x positions
                                node_bounds.x >= fb.x + fb.width
                            } else {
                                // Different line: compare y positions
                                node_bounds.y > fb.y + fb.height
                            };

                            if is_after {
                                after.push(StructuredNode::Paragraph(ParagraphNode {
                                    content: self.translated(text),
                                    som_path: None,
                                    source_name: None,
                                }));
                            } else {
                                before.push(StructuredNode::Paragraph(ParagraphNode {
                                    content: self.translated(text),
                                    som_path: None,
                                    source_name: None,
                                }));
                            }
                        }
                    }
                } else {
                    // Single node: check if it's a multi-line node spanning the field
                    // and split it at the field boundary (same logic as multi-node branch).
                    if node_indices.len() == 1 {
                        if let Some(node) = self.doc.get_node(node_indices[0]) {
                            let node_bounds = node.bounds();
                            let is_multiline_spanning_field = node_bounds.height
                                > fb.height * Decimal::from(2)
                                && fb.y >= node_bounds.y
                                && fb.y <= node_bounds.y + node_bounds.height;

                            if is_multiline_spanning_field {
                                if let Some((before_str, after_str)) =
                                    Self::split_text_at_field_position(node, fb)
                                {
                                    if !before_str.is_empty() {
                                        before.push(StructuredNode::Paragraph(ParagraphNode {
                                            content: self.translated(InlineText::plain(before_str)),
                                            som_path: None,
                                            source_name: None,
                                        }));
                                    }
                                    if !after_str.is_empty() {
                                        after.push(StructuredNode::Paragraph(ParagraphNode {
                                            content: self.translated(InlineText::plain(after_str)),
                                            som_path: None,
                                            source_name: None,
                                        }));
                                    }
                                    return (before, after);
                                }
                            }
                        }
                    }

                    // Fallback: use horizontal position relative to field
                    let text = self.extract_inline_text(child_group_idx);
                    if !text.is_empty() {
                        if let Some(text_bounds) = self.doc.get_bounds(child_group_idx) {
                            // If text ends before field starts, it's "before"
                            // If text starts after field ends, it's "after"
                            // Otherwise, classify by center position
                            if text_bounds.x + text_bounds.width <= fb.x {
                                before.push(StructuredNode::Paragraph(ParagraphNode {
                                    content: self.translated(text),
                                    som_path: None,
                                    source_name: None,
                                }));
                            } else if text_bounds.x >= fb.x + fb.width {
                                after.push(StructuredNode::Paragraph(ParagraphNode {
                                    content: self.translated(text),
                                    som_path: None,
                                    source_name: None,
                                }));
                            } else {
                                // Overlapping: use center
                                let text_center = text_bounds.x + text_bounds.width / Decimal::TWO;
                                let field_center = fb.x + fb.width / Decimal::TWO;
                                if text_center < field_center {
                                    before.push(StructuredNode::Paragraph(ParagraphNode {
                                        content: self.translated(text),
                                        som_path: None,
                                        source_name: None,
                                    }));
                                } else {
                                    after.push(StructuredNode::Paragraph(ParagraphNode {
                                        content: self.translated(text),
                                        som_path: None,
                                        source_name: None,
                                    }));
                                }
                            }
                        } else {
                            // No bounds, fall back to original classification
                            before.push(StructuredNode::Paragraph(ParagraphNode {
                                content: self.translated(text),
                                som_path: None,
                                source_name: None,
                            }));
                        }
                    }
                }
                (before, after)
            };

        // Convert "before" text groups, splitting by position when possible
        for &child_index in before {
            if let Some(&child_group_idx) = group.children.get(child_index) {
                if let Some(fb) = &field_bounds {
                    let (b, a) = split_by_position(child_group_idx, fb);
                    before_nodes.extend(b);
                    after_nodes.extend(a);
                } else {
                    // No field bounds — treat entire group as "before"
                    let text = self.extract_inline_text(child_group_idx);
                    if !text.is_empty() {
                        before_nodes.push(StructuredNode::Paragraph(ParagraphNode {
                            content: self.translated(text),
                            som_path: None,
                            source_name: None,
                        }));
                    }
                }
            }
        }

        // Convert "after" text groups, also splitting by position when possible
        for &child_index in after {
            if let Some(&child_group_idx) = group.children.get(child_index) {
                if let Some(fb) = &field_bounds {
                    let (b, a) = split_by_position(child_group_idx, fb);
                    before_nodes.extend(b);
                    after_nodes.extend(a);
                } else {
                    // No field bounds — treat entire group as "after"
                    let text = self.extract_inline_text(child_group_idx);
                    if !text.is_empty() {
                        after_nodes.push(StructuredNode::Paragraph(ParagraphNode {
                            content: self.translated(text),
                            som_path: None,
                            source_name: None,
                        }));
                    }
                }
            }
        }

        let mut result_nodes: Vec<StructuredNode> = Vec::new();
        result_nodes.extend(before_nodes);

        // Convert the field itself with label "UNKNOWN"
        if let Some(&field_group_idx) = group.children.get(field_child_idx) {
            if let Some(field_node) =
                self.convert_field_group(field_group_idx, Some(InlineText::plain("UNKNOWN")))
            {
                result_nodes.push(field_node);
            }
        }

        // Emit paragraphs that belong after the field
        result_nodes.extend(after_nodes);

        match result_nodes.len() {
            0 => None,
            1 => Some(result_nodes.into_iter().next().unwrap()),
            _ => Some(StructuredNode::Group(GroupNode {
                children: result_nodes,
            })),
        }
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
        let field_som_path = self
            .get_som_path(node)
            .cloned()
            .unwrap_or_else(|| SomPath::new(name.clone()));

        Some(StructuredNode::Field(FieldNode {
            name: FieldId::from_som_path(&field_som_path),
            som_path: Some(field_som_path),
            label: label.map(|l| self.translated(l.to_plain())),
            input_type: field_type,
            value: input_value,
            placeholder: self.get_placeholder(node).map(TranslatableString::Plain),
            required: false,
        }))
    }

    /// Determine FieldType from widget hints.
    fn determine_field_type(&self, node: &FlattenedNode) -> FieldType {
        // Check for widget type hint
        if let Some(widget) = node.widget_type() {
            return match widget {
                WidgetKind::Text => self.text_field_type(node),
                WidgetKind::TextArea => {
                    // Some forms mark single-line fields with multiLine="1" even
                    // though the field is too short to display multiple lines.
                    // Treat fields shorter than ~18pt as regular text fields.
                    let min_textarea_height = Decimal::from(18);
                    if node.bounds().height < min_textarea_height {
                        self.text_field_type(node)
                    } else {
                        FieldType::Textarea {
                            max_length: self.get_max_length(node),
                        }
                    }
                }
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
            FieldType::CheckboxGroup { .. } => InputValue::Text(value.to_string()),
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
            FieldType::Textarea { .. } => InputValue::Text(value.to_string()),
        })
    }

    // ========================================================================
    // Helper: Extract hints
    // ========================================================================

    fn is_interactive(&self, node: &FlattenedNode) -> bool {
        node.is_interactive()
    }

    fn get_format_pattern(&self, node: &FlattenedNode) -> Option<String> {
        node.validation().and_then(|(_, fmt, _)| fmt.cloned())
    }

    /// Extract dropdown options from a Hint::Dropdown, mapping to NameValue pairs.
    fn get_dropdown_options(&self, node: &FlattenedNode) -> Vec<NameValue> {
        node.dropdown()
            .map(|info| {
                info.options
                    .iter()
                    .filter(|(display, _)| !display.trim().is_empty())
                    .map(|(display, save)| NameValue {
                        name: TranslatableString::Plain(display.clone()),
                        value: InputValue::Text(save.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn get_max_length(&self, node: &FlattenedNode) -> Option<usize> {
        node.field_behavior()
            .and_then(|(_, _, max_len, _)| max_len.map(|n| n as usize))
    }

    fn get_placeholder(&self, node: &FlattenedNode) -> Option<String> {
        node.accessibility().and_then(|(_, tip, _)| tip.cloned())
    }

    /// Extract the SOM path from a field's hints.
    fn get_som_path<'n>(&self, node: &'n FlattenedNode) -> Option<&'n SomPath> {
        node.som_path()
    }

    /// Extract the SOM path from the first FlattenedNode in a group.
    fn extract_group_som_path(&self, group_idx: usize) -> Option<SomPath> {
        self.doc
            .collect_nodes(group_idx)
            .first()
            .and_then(|n| n.som_path().cloned())
    }

    /// Extract the `source_name` (XFA draw node name) from the first text node in a group.
    fn extract_group_source_name(&self, group_idx: usize) -> Option<String> {
        self.doc
            .collect_nodes(group_idx)
            .iter()
            .find_map(|n| match &n.kind {
                FlattenedNodeKind::Text { source_name, .. } => source_name.clone(),
                _ => None,
            })
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
    // Helper: Multi-line text splitting for inline fields
    // ========================================================================

    /// Split the text content of a multi-line FlattenedNode at the position
    /// where an inline field is located.
    ///
    /// Uses a proportional estimate based on the field's (x, y) position
    /// relative to the text node's bounding box.  The content is split at
    /// the nearest word boundary.
    ///
    /// Returns `Some((before, after))` on success, or `None` if splitting
    /// cannot be performed (e.g. empty text or field outside node).
    fn split_text_at_field_position(
        node: &FlattenedNode,
        field_bounds: &Bounds,
    ) -> Option<(String, String)> {
        let (content, font_size, _font_name) = match &node.kind {
            FlattenedNodeKind::Text {
                content,
                font_size,
                font_name,
                ..
            } => (content.as_str(), *font_size, font_name.as_str()),
            _ => return None,
        };

        if content.trim().is_empty() {
            return None;
        }

        let node_bounds = node.bounds();
        let node_width = node_bounds.width;
        let node_height = node_bounds.height;

        if node_width <= Decimal::ZERO || node_height <= Decimal::ZERO {
            return None;
        }

        // Estimate line height using AXTE convention (font_size * 1.2).
        let line_height = font_size * Decimal::from_str_exact("1.2").unwrap();
        if line_height <= Decimal::ZERO {
            return None;
        }

        // Which line the field occupies (0-indexed).
        let field_y_offset = field_bounds.y - node_bounds.y;
        let field_line = (field_y_offset / line_height)
            .to_f32()
            .unwrap_or(0.0)
            .floor()
            .max(0.0) as usize;

        // Fraction of the line before the field.
        let field_x_offset = field_bounds.x - node_bounds.x;
        let line_frac = (field_x_offset / node_width)
            .to_f32()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);

        // The field also occupies horizontal space on the line.
        // Text before the field occupies `field_x_offset` width, the field
        // itself occupies `field_bounds.width`, and remaining text (if any)
        // comes after.  Compute the effective fraction of text BEFORE the
        // field on that line — this is `field_x_offset / (node_width - field_width)`.
        // We cap the effective text width at node_width (the field could be
        // wider than the remaining space).
        let text_width_on_line = (node_width - field_bounds.width).max(Decimal::ONE);
        let effective_frac = (field_x_offset / text_width_on_line)
            .to_f32()
            .unwrap_or(line_frac)
            .clamp(0.0, 1.0);

        // Estimate total number of lines via the node height.
        let num_lines = (node_height / line_height)
            .to_f32()
            .unwrap_or(1.0)
            .ceil()
            .max(1.0);

        // Approximate the split as a fraction of total content.
        let overall_frac = (field_line as f32 + effective_frac) / num_lines;
        let total_chars = content.chars().count();
        let approx_char = ((total_chars as f32) * overall_frac).round() as usize;
        let approx_char = approx_char.min(total_chars);

        // Snap to the best word/sentence boundary near the estimated position.
        // Prefer sentence-ending punctuation ('. ', '.(' etc.) over plain
        // whitespace, because inline fields typically sit at clause/sentence
        // boundaries and the proportional estimate has limited accuracy.
        let chars: Vec<char> = content.chars().collect();

        // Phase 1: look for a sentence boundary (period/colon/semicolon
        //          followed by whitespace or '(') within ±20 chars.
        let mut best_split = None;
        let search_radius = 20usize;
        for delta in 0..=search_radius {
            // Forward
            let fwd = approx_char + delta;
            if best_split.is_none()
                && fwd < chars.len()
                && (chars[fwd] == '.' || chars[fwd] == ':' || chars[fwd] == ';')
            {
                let next = fwd + 1;
                if next >= chars.len() || chars[next].is_whitespace() || chars[next] == '(' {
                    best_split = Some(next); // split AFTER the punctuation
                }
            }
            // Backward
            if best_split.is_none() && approx_char >= delta {
                let bwd = approx_char - delta;
                if bwd < chars.len()
                    && (chars[bwd] == '.' || chars[bwd] == ':' || chars[bwd] == ';')
                {
                    let next = bwd + 1;
                    if next >= chars.len() || chars[next].is_whitespace() || chars[next] == '(' {
                        best_split = Some(next);
                    }
                }
            }
            if best_split.is_some() {
                break;
            }
        }

        // Phase 2: fall back to nearest whitespace within ±30 chars.
        if best_split.is_none() {
            for delta in 0..30 {
                if approx_char + delta < chars.len() && chars[approx_char + delta].is_whitespace() {
                    best_split = Some(approx_char + delta);
                    break;
                }
                if approx_char >= delta
                    && approx_char - delta < chars.len()
                    && chars[approx_char - delta].is_whitespace()
                {
                    best_split = Some(approx_char - delta);
                    break;
                }
            }
        }

        let best_split = best_split.unwrap_or(approx_char);

        let before_text: String = chars[..best_split].iter().collect();
        let after_text: String = chars[best_split..].iter().collect();

        let before_trimmed = before_text.trim_end().to_string();
        let after_trimmed = after_text.trim_start().to_string();

        Some((before_trimmed, after_trimmed))
    }

    // ========================================================================
    // Helper: InlineText extraction
    // ========================================================================

    /// Extract InlineText from a group (recursively collecting all text).
    fn extract_inline_text(&self, group_idx: usize) -> InlineText {
        let nodes = self.doc.collect_nodes(group_idx);
        let mut result = Vec::new();

        for node in nodes {
            // Before appending new content from a separate flattened node,
            // check whether a separator space is needed.
            if !result.is_empty() {
                let prev_trailing = result.last().and_then(|n: &InlineNode| n.trailing_text());
                let next_leading = node.leading_text();
                if let (Some(l), Some(r)) = (prev_trailing, next_leading) {
                    if super::needs_separator(l, r) {
                        result.push(InlineNode::Text(" ".to_string()));
                    }
                }
            }
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
        if let Some(rich_text) = node.rich_text() {
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

        // Plain text fallback
        if let FlattenedNodeKind::Text { content, .. } = &node.kind
            && !content.is_empty()
        {
            result.push(InlineNode::Text(content.clone()));
        }
    }

    /// Convert RichText to multiple ParagraphNodes (one per RichParagraph).
    /// Skips empty paragraphs.
    fn convert_rich_text_to_paragraph_nodes(
        &self,
        rich_text: &RichText,
        som_path: Option<SomPath>,
        source_name: Option<String>,
    ) -> Vec<StructuredNode> {
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
                        content: self.translated(InlineText::new(inline_nodes)),
                        som_path: som_path.clone(),
                        source_name: source_name.clone(),
                    }))
                }
            })
            .collect()
    }

    /// Convert RichText to InlineNodes.
    ///
    /// When flattening multiple paragraphs into a single inline sequence,
    /// inserts a space between paragraphs when neither side already provides
    /// whitespace at the boundary.
    fn append_inline_nodes_from_rich_text(
        &self,
        rich_text: &RichText,
        result: &mut Vec<InlineNode>,
    ) {
        for (para_idx, para) in rich_text.paragraphs.iter().enumerate() {
            // Before appending the first run of a new paragraph (after the
            // first), check whether a separator space is needed.
            if para_idx > 0 && !result.is_empty() {
                let last_text = result.last().and_then(|n| n.trailing_text());
                let first_text = para
                    .runs
                    .iter()
                    .map(|r| r.text.as_str())
                    .find(|t| !t.is_empty());
                if let (Some(l), Some(r)) = (last_text, first_text) {
                    if super::needs_separator(l, r) {
                        result.push(InlineNode::Text(" ".to_string()));
                    }
                }
            }
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

        // Wrap with superscript if detected from vertical-align
        if run.superscript {
            node = InlineNode::Superscript(Box::new(node));
        }

        node
    }
}

#[cfg(test)]
mod tests {
    // TODO: Add tests when we have test fixtures
}
