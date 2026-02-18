//! Translation merger for combining documents in different languages.
//!
//! This module merges multiple `DocumentEnvelope`s (one per language) into a single
//! multilingual representation. Text content is stored per-language using
//! `InlineNode::TranslatedText` and `TranslatableString::Translated`.
//!
//! # Algorithm
//!
//! 1. Take the first language as the base tree structure.
//! 2. For each subsequent language, align its node list against the base using
//!    LCS (longest common subsequence) on `structural_eq_ignore_text`.
//! 3. For matched nodes, recursively merge text content by combining translations.
//! 4. Unmatched nodes are kept with only their source language populated.

use std::collections::HashMap;

use crate::context::Context;
use crate::structured::{
    ConditionalNode, DocumentEnvelope, FieldNode, FieldType, GridLayout, GridLayoutElement,
    GroupNode, HeadingNode, InlineNode, InlineText, ListNode, NameValue, ParagraphNode,
    RepeatableNode, StructuredNode, TableHeader, TableNode, TableRow, TranslatableString,
};

/// Threshold for minimum structural similarity (0.0 to 1.0).
/// Documents must have at least this much structural overlap to be considered
/// translations of the same form.
const MIN_STRUCTURAL_SIMILARITY: f64 = 0.5;

/// Error type for translation merging failures.
#[derive(Debug, Clone)]
pub enum MergeError {
    /// Documents are too structurally different to be translations of the same form.
    InsufficientStructuralSimilarity { similarity: f64, threshold: f64 },
    /// Multiple documents have the same language code.
    DuplicateLanguage { language: String },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::InsufficientStructuralSimilarity {
                similarity,
                threshold,
            } => {
                write!(
                    f,
                    "Documents are too different to be translations (similarity: {:.1}%, required: {:.1}%)",
                    similarity * 100.0,
                    threshold * 100.0
                )
            }
            MergeError::DuplicateLanguage { language } => {
                write!(
                    f,
                    "Cannot merge documents with duplicate language code: '{}'",
                    language
                )
            }
        }
    }
}

impl std::error::Error for MergeError {}

/// Merge multiple `DocumentEnvelope`s from different languages into one multilingual envelope.
///
/// Each envelope is expected to come from the same document in a different language.
/// The context from the first envelope is used as the base, with language set to
/// a comma-separated list of all languages.
///
/// Returns an error if the documents are too structurally different to be translations.
pub fn merge_translations(
    envelopes: Vec<DocumentEnvelope>,
) -> Result<DocumentEnvelope, MergeError> {
    if envelopes.is_empty() {
        return Ok(DocumentEnvelope {
            context: Context::with_language("und"),
            content: Vec::new(),
        });
    }

    if envelopes.len() == 1 {
        return Ok(envelopes.into_iter().next().unwrap());
    }

    // Collect all languages and check for duplicates
    let languages: Vec<String> = envelopes
        .iter()
        .map(|e| e.context.language().to_string())
        .collect();

    let mut seen_languages = std::collections::HashSet::new();
    for lang in &languages {
        if !seen_languages.insert(lang.clone()) {
            return Err(MergeError::DuplicateLanguage {
                language: lang.clone(),
            });
        }
    }

    // Validate structural similarity between all pairs
    for i in 0..envelopes.len() {
        for j in (i + 1)..envelopes.len() {
            let similarity =
                calculate_structural_similarity(&envelopes[i].content, &envelopes[j].content);
            if similarity < MIN_STRUCTURAL_SIMILARITY {
                return Err(MergeError::InsufficientStructuralSimilarity {
                    similarity,
                    threshold: MIN_STRUCTURAL_SIMILARITY,
                });
            }
        }
    }

    // Start with the first envelope as the base
    let mut iter = envelopes.into_iter();
    let base = iter.next().unwrap();
    let base_lang = base.context.language().to_string();
    let mut merged_content = base.content;

    // Merge each subsequent language into the base
    for envelope in iter {
        let other_lang = envelope.context.language().to_string();
        merged_content =
            merge_node_lists(&merged_content, &base_lang, &envelope.content, &other_lang);
    }

    // Create merged context
    let context = Context::with_language(languages.join(","));

    Ok(DocumentEnvelope {
        context,
        content: merged_content,
    })
}

/// Calculate structural similarity between two node lists.
///
/// Returns a value between 0.0 (completely different) and 1.0 (identical structure).
/// Uses the LCS length as a percentage of the average list length.
fn calculate_structural_similarity(a: &[StructuredNode], b: &[StructuredNode]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let dp = lcs_table(a, b);
    let lcs_length = dp[a.len()][b.len()] as f64;
    let avg_length = (a.len() + b.len()) as f64 / 2.0;

    lcs_length / avg_length
}

// ============================================================================
// LCS-based alignment for node lists
// ============================================================================

/// Compute the LCS (longest common subsequence) table for two node slices,
/// using `structural_eq_ignore_text` as the equality predicate.
fn lcs_table(a: &[StructuredNode], b: &[StructuredNode]) -> Vec<Vec<usize>> {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1].structural_eq_ignore_text(&b[j - 1]) {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    dp
}

/// Backtrack through the LCS table to produce aligned pairs.
/// Returns a list of (Option<idx_in_a>, Option<idx_in_b>) pairs.
/// Both Some → matched pair. Only one Some → unmatched node from that side.
fn lcs_align(
    a: &[StructuredNode],
    b: &[StructuredNode],
    dp: &[Vec<usize>],
) -> Vec<(Option<usize>, Option<usize>)> {
    let mut result = Vec::new();
    let mut i = a.len();
    let mut j = b.len();

    // Backtrack to find the LCS alignment
    let mut matches = Vec::new();
    while i > 0 && j > 0 {
        if a[i - 1].structural_eq_ignore_text(&b[j - 1]) {
            matches.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    matches.reverse();

    // Build the full alignment with unmatched nodes
    let mut ai = 0;
    let mut bi = 0;
    for (ma, mb) in &matches {
        // Emit unmatched nodes from a before this match
        while ai < *ma {
            result.push((Some(ai), None));
            ai += 1;
        }
        // Emit unmatched nodes from b before this match
        while bi < *mb {
            result.push((None, Some(bi)));
            bi += 1;
        }
        // Emit the matched pair
        result.push((Some(*ma), Some(*mb)));
        ai = ma + 1;
        bi = mb + 1;
    }
    // Emit remaining unmatched nodes
    while ai < a.len() {
        result.push((Some(ai), None));
        ai += 1;
    }
    while bi < b.len() {
        result.push((None, Some(bi)));
        bi += 1;
    }

    result
}

/// Merge two node lists from different languages using LCS alignment.
fn merge_node_lists(
    base: &[StructuredNode],
    base_lang: &str,
    other: &[StructuredNode],
    other_lang: &str,
) -> Vec<StructuredNode> {
    let dp = lcs_table(base, other);
    let alignment = lcs_align(base, other, &dp);

    let mut result = Vec::new();
    for (ai, bi) in alignment {
        match (ai, bi) {
            (Some(a), Some(b)) => {
                // Matched pair: merge text content from both languages
                result.push(merge_node(&base[a], base_lang, &other[b], other_lang));
            }
            (Some(a), None) => {
                // Only in base language - keep as-is
                result.push(base[a].clone());
            }
            (None, Some(b)) => {
                // Only in other language - keep as-is
                result.push(other[b].clone());
            }
            (None, None) => unreachable!(),
        }
    }
    result
}

// ============================================================================
// Recursive node merging
// ============================================================================

/// Merge two structurally-equivalent nodes from different languages.
/// Combines text content into multilingual `TranslatedText` / `TranslatableString`.
fn merge_node(
    base: &StructuredNode,
    base_lang: &str,
    other: &StructuredNode,
    other_lang: &str,
) -> StructuredNode {
    match (base, other) {
        (StructuredNode::Heading(a), StructuredNode::Heading(b)) => {
            StructuredNode::Heading(HeadingNode {
                level: a.level,
                content: merge_inline_text(&a.content, base_lang, &b.content, other_lang),
            })
        }
        (StructuredNode::Paragraph(a), StructuredNode::Paragraph(b)) => {
            StructuredNode::Paragraph(ParagraphNode {
                content: merge_inline_text(&a.content, base_lang, &b.content, other_lang),
            })
        }
        (StructuredNode::Image(a), StructuredNode::Image(_b)) => {
            // Images are binary - keep the base
            StructuredNode::Image(a.clone())
        }
        (StructuredNode::Table(a), StructuredNode::Table(b)) => {
            StructuredNode::Table(merge_table(a, base_lang, b, other_lang))
        }
        (StructuredNode::Field(a), StructuredNode::Field(b)) => {
            StructuredNode::Field(merge_field(a, base_lang, b, other_lang))
        }
        (StructuredNode::Repeatable(a), StructuredNode::Repeatable(b)) => {
            StructuredNode::Repeatable(RepeatableNode {
                item: Box::new(merge_node(&a.item, base_lang, &b.item, other_lang)),
                min_occurrences: a.min_occurrences,
                max_occurrences: a.max_occurrences,
            })
        }
        (StructuredNode::Group(a), StructuredNode::Group(b)) => {
            let children = merge_node_lists(&a.children, base_lang, &b.children, other_lang);
            StructuredNode::Group(GroupNode { children })
        }
        (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) => {
            StructuredNode::Conditional(ConditionalNode {
                condition: a.condition.clone(),
                content: Box::new(merge_node(&a.content, base_lang, &b.content, other_lang)),
            })
        }
        (StructuredNode::Empty, StructuredNode::Empty) => StructuredNode::Empty,
        (StructuredNode::GridLayout(a), StructuredNode::GridLayout(b)) => {
            let elements: Vec<GridLayoutElement> = a
                .elements
                .iter()
                .zip(b.elements.iter())
                .map(|(ea, eb)| GridLayoutElement {
                    span: ea.span,
                    node: merge_node(&ea.node, base_lang, &eb.node, other_lang),
                })
                .collect();
            StructuredNode::GridLayout(GridLayout {
                columns: a.columns,
                elements,
            })
        }
        (StructuredNode::List(a), StructuredNode::List(b)) => {
            let items = a
                .items
                .iter()
                .zip(b.items.iter())
                .map(|(ia, ib)| merge_inline_text(ia, base_lang, ib, other_lang))
                .collect();
            StructuredNode::List(ListNode {
                ordered: a.ordered,
                items,
            })
        }
        // Fallback: if nodes don't match (shouldn't happen after LCS), keep base
        _ => base.clone(),
    }
}

// ============================================================================
// InlineText merging
// ============================================================================

/// Merge two `InlineText`s from different languages.
///
/// If both have the same number of inline nodes with matching types (Text/Strong/Emphasis/Link),
/// merge them element-wise. Otherwise, produce a single `TranslatedText` node from each
/// side's plain text.
fn merge_inline_text(
    base: &InlineText,
    base_lang: &str,
    other: &InlineText,
    other_lang: &str,
) -> InlineText {
    // Try element-wise merge if structures match
    if base.0.len() == other.0.len()
        && base
            .0
            .iter()
            .zip(other.0.iter())
            .all(|(a, b)| inline_node_variant_eq(a, b))
    {
        let nodes: Vec<InlineNode> = base
            .0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| merge_inline_node(a, base_lang, b, other_lang))
            .collect();
        return InlineText(nodes);
    }

    // Fallback: merge the entire text as a single TranslatedText node
    let base_text = base.as_plain_text();
    let other_text = other.as_plain_text();
    let mut map = HashMap::new();

    // Check if base already has translations
    if let Some(InlineNode::TranslatedText(existing)) = base.0.first() {
        map.extend(existing.clone());
    } else if !base_text.is_empty() {
        map.insert(base_lang.to_string(), base_text);
    }

    if let Some(InlineNode::TranslatedText(existing)) = other.0.first() {
        map.extend(existing.clone());
    } else if !other_text.is_empty() {
        map.insert(other_lang.to_string(), other_text);
    }

    if map.is_empty() {
        InlineText::empty()
    } else {
        InlineText(vec![InlineNode::TranslatedText(map)])
    }
}

/// Check if two `InlineNode`s have the same variant (ignoring content).
fn inline_node_variant_eq(a: &InlineNode, b: &InlineNode) -> bool {
    matches!(
        (a, b),
        (InlineNode::Text(_), InlineNode::Text(_))
            | (InlineNode::TranslatedText(_), InlineNode::Text(_))
            | (InlineNode::Text(_), InlineNode::TranslatedText(_))
            | (InlineNode::TranslatedText(_), InlineNode::TranslatedText(_))
            | (InlineNode::Link(_), InlineNode::Link(_))
            | (InlineNode::Strong(_), InlineNode::Strong(_))
            | (InlineNode::Emphasis(_), InlineNode::Emphasis(_))
    )
}

/// Merge two `InlineNode`s from different languages.
fn merge_inline_node(
    base: &InlineNode,
    base_lang: &str,
    other: &InlineNode,
    other_lang: &str,
) -> InlineNode {
    match (base, other) {
        // Text + Text → TranslatedText
        (InlineNode::Text(a), InlineNode::Text(b)) => {
            let mut map = HashMap::new();
            map.insert(base_lang.to_string(), a.clone());
            map.insert(other_lang.to_string(), b.clone());
            InlineNode::TranslatedText(map)
        }
        // TranslatedText + Text → merge into existing map
        (InlineNode::TranslatedText(existing), InlineNode::Text(b)) => {
            let mut map = existing.clone();
            map.insert(other_lang.to_string(), b.clone());
            InlineNode::TranslatedText(map)
        }
        // Text + TranslatedText → merge into existing map
        (InlineNode::Text(a), InlineNode::TranslatedText(existing)) => {
            let mut map = existing.clone();
            map.insert(base_lang.to_string(), a.clone());
            InlineNode::TranslatedText(map)
        }
        // TranslatedText + TranslatedText → merge maps
        (InlineNode::TranslatedText(a), InlineNode::TranslatedText(b)) => {
            let mut map = a.clone();
            map.extend(b.clone());
            InlineNode::TranslatedText(map)
        }
        // Strong + Strong → merge inner
        (InlineNode::Strong(a), InlineNode::Strong(b)) => {
            InlineNode::Strong(Box::new(merge_inline_node(a, base_lang, b, other_lang)))
        }
        // Emphasis + Emphasis → merge inner
        (InlineNode::Emphasis(a), InlineNode::Emphasis(b)) => {
            InlineNode::Emphasis(Box::new(merge_inline_node(a, base_lang, b, other_lang)))
        }
        // Link + Link → merge content
        (InlineNode::Link(a), InlineNode::Link(b)) => {
            InlineNode::Link(crate::structured::LinkNode {
                href: a.href.clone(),
                content: merge_inline_text(&a.content, base_lang, &b.content, other_lang),
            })
        }
        // Mismatched variants: shouldn't happen after variant check, but fallback
        _ => base.clone(),
    }
}

// ============================================================================
// Field merging
// ============================================================================

/// Merge two `FieldNode`s from different languages.
/// Combines labels, placeholders, and option names into translated forms.
fn merge_field(
    base: &FieldNode,
    base_lang: &str,
    other: &FieldNode,
    other_lang: &str,
) -> FieldNode {
    // Merge label
    let label = match (&base.label, &other.label) {
        (Some(a), Some(b)) => Some(merge_inline_text(a, base_lang, b, other_lang)),
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    };

    // Merge placeholder
    let placeholder =
        merge_translatable_string(&base.placeholder, base_lang, &other.placeholder, other_lang);

    // Merge input type (for Radio/Select option names)
    let input_type = merge_field_type(&base.input_type, base_lang, &other.input_type, other_lang);

    FieldNode {
        name: base.name.clone(),
        som_path: base.som_path.clone(),
        label,
        input_type,
        value: base.value.clone(),
        placeholder,
    }
}

/// Merge two `TranslatableString` options.
fn merge_translatable_string(
    base: &Option<TranslatableString>,
    base_lang: &str,
    other: &Option<TranslatableString>,
    other_lang: &str,
) -> Option<TranslatableString> {
    match (base, other) {
        (Some(a), Some(b)) => {
            let mut map = HashMap::new();
            match a {
                TranslatableString::Plain(s) => {
                    map.insert(base_lang.to_string(), s.clone());
                }
                TranslatableString::Translated(m) => {
                    map.extend(m.clone());
                }
            }
            match b {
                TranslatableString::Plain(s) => {
                    map.insert(other_lang.to_string(), s.clone());
                }
                TranslatableString::Translated(m) => {
                    map.extend(m.clone());
                }
            }
            Some(TranslatableString::Translated(map))
        }
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    }
}

/// Merge two `FieldType`s, combining option names for Radio/Select.
fn merge_field_type(
    base: &FieldType,
    base_lang: &str,
    other: &FieldType,
    other_lang: &str,
) -> FieldType {
    match (base, other) {
        (FieldType::Radio { options: opts_a }, FieldType::Radio { options: opts_b }) => {
            FieldType::Radio {
                options: merge_name_values(opts_a, base_lang, opts_b, other_lang),
            }
        }
        (FieldType::Select { options: opts_a }, FieldType::Select { options: opts_b }) => {
            FieldType::Select {
                options: merge_name_values(opts_a, base_lang, opts_b, other_lang),
            }
        }
        // For non-option types, keep the base
        _ => base.clone(),
    }
}

/// Merge two `NameValue` vectors by zipping and merging names.
fn merge_name_values(
    base: &[NameValue],
    base_lang: &str,
    other: &[NameValue],
    other_lang: &str,
) -> Vec<NameValue> {
    base.iter()
        .zip(other.iter())
        .map(|(a, b)| {
            let merged_name =
                merge_translatable_string_values(&a.name, base_lang, &b.name, other_lang);
            NameValue {
                name: merged_name,
                value: a.value.clone(),
            }
        })
        .collect()
}

/// Merge two `TranslatableString` values (non-optional).
fn merge_translatable_string_values(
    base: &TranslatableString,
    base_lang: &str,
    other: &TranslatableString,
    other_lang: &str,
) -> TranslatableString {
    let mut map = HashMap::new();
    match base {
        TranslatableString::Plain(s) => {
            map.insert(base_lang.to_string(), s.clone());
        }
        TranslatableString::Translated(m) => {
            map.extend(m.clone());
        }
    }
    match other {
        TranslatableString::Plain(s) => {
            map.insert(other_lang.to_string(), s.clone());
        }
        TranslatableString::Translated(m) => {
            map.extend(m.clone());
        }
    }
    TranslatableString::Translated(map)
}

// ============================================================================
// Table merging
// ============================================================================

/// Merge two `TableNode`s from different languages.
fn merge_table(
    base: &TableNode,
    base_lang: &str,
    other: &TableNode,
    other_lang: &str,
) -> TableNode {
    // Merge header
    let header = match (&base.header, &other.header) {
        (Some(h1), Some(h2)) => {
            let cells = merge_node_lists(&h1.cells, base_lang, &h2.cells, other_lang);
            Some(TableHeader { cells })
        }
        (h, _) => h.clone(),
    };

    // Merge rows
    let rows: Vec<TableRow> = base
        .rows
        .iter()
        .zip(other.rows.iter())
        .map(|(r1, r2)| {
            let cells = merge_node_lists(&r1.cells, base_lang, &r2.cells, other_lang);
            TableRow { cells }
        })
        .collect();

    // Merge caption
    let caption = match (&base.caption, &other.caption) {
        (Some(a), Some(b)) => Some(merge_inline_text(a, base_lang, b, other_lang)),
        (c, _) => c.clone(),
    };

    TableNode {
        header,
        rows,
        caption,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{HeadingLevel, HeadingNode, InlineText, ParagraphNode};

    fn make_envelope(lang: &str, content: Vec<StructuredNode>) -> DocumentEnvelope {
        DocumentEnvelope {
            context: Context::with_language(lang),
            content,
        }
    }

    #[test]
    fn test_merge_single_language_passthrough() {
        let envelope = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hallo Welt"),
            })],
        );

        let result = merge_translations(vec![envelope]).unwrap();
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.context.language(), "de");
    }

    #[test]
    fn test_merge_two_languages_identical_structure() {
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Titel"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Hallo"),
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Title"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Hello"),
                }),
            ],
        );

        let result = merge_translations(vec![de, en]).unwrap();
        assert_eq!(result.content.len(), 2);
        assert_eq!(result.context.language(), "de,en");

        // Check heading has translated text
        if let StructuredNode::Heading(h) = &result.content[0] {
            assert_eq!(h.content.0.len(), 1);
            if let InlineNode::TranslatedText(map) = &h.content.0[0] {
                assert_eq!(map.get("de").unwrap(), "Titel");
                assert_eq!(map.get("en").unwrap(), "Title");
            } else {
                panic!("Expected TranslatedText");
            }
        } else {
            panic!("Expected Heading");
        }

        // Check paragraph has translated text
        if let StructuredNode::Paragraph(p) = &result.content[1] {
            assert_eq!(p.content.0.len(), 1);
            if let InlineNode::TranslatedText(map) = &p.content.0[0] {
                assert_eq!(map.get("de").unwrap(), "Hallo");
                assert_eq!(map.get("en").unwrap(), "Hello");
            } else {
                panic!("Expected TranslatedText");
            }
        } else {
            panic!("Expected Paragraph");
        }
    }

    #[test]
    fn test_merge_with_structural_mismatch_lcs() {
        // German has an extra paragraph that English doesn't have
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Einleitung"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Nur auf Deutsch"),
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(InlineText::plain("Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Introduction"),
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(InlineText::plain("Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en]).unwrap();
        // Should have 3 nodes: merged paragraph, DE-only paragraph, merged field
        assert_eq!(result.content.len(), 3);

        // First paragraph should be merged
        assert!(matches!(result.content[0], StructuredNode::Paragraph(_)));
        // Second is DE-only
        assert!(matches!(result.content[1], StructuredNode::Paragraph(_)));
        // Third is the merged field
        assert!(matches!(result.content[2], StructuredNode::Field(_)));
    }

    #[test]
    fn test_merge_three_languages() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hallo"),
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hello"),
            })],
        );
        let fr = make_envelope(
            "fr",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Bonjour"),
            })],
        );

        let result = merge_translations(vec![de, en, fr]).unwrap();
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.context.language(), "de,en,fr");

        if let StructuredNode::Paragraph(p) = &result.content[0] {
            if let InlineNode::TranslatedText(map) = &p.content.0[0] {
                assert_eq!(map.get("de").unwrap(), "Hallo");
                assert_eq!(map.get("en").unwrap(), "Hello");
                assert_eq!(map.get("fr").unwrap(), "Bonjour");
            } else {
                panic!("Expected TranslatedText");
            }
        } else {
            panic!("Expected Paragraph");
        }
    }

    #[test]
    fn test_merge_field_labels_and_options() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Field(FieldNode {
                name: "gender".into(),
                som_path: None,
                label: Some(InlineText::plain("Geschlecht")),
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Männlich".to_string()),
                            value: crate::structured::InputValue::Text("M".to_string()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Weiblich".to_string()),
                            value: crate::structured::InputValue::Text("F".to_string()),
                        },
                    ],
                },
                value: None,
                placeholder: Some(TranslatableString::Plain("Bitte wählen".to_string())),
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Field(FieldNode {
                name: "gender".into(),
                som_path: None,
                label: Some(InlineText::plain("Gender")),
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Male".to_string()),
                            value: crate::structured::InputValue::Text("M".to_string()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Female".to_string()),
                            value: crate::structured::InputValue::Text("F".to_string()),
                        },
                    ],
                },
                value: None,
                placeholder: Some(TranslatableString::Plain("Please select".to_string())),
            })],
        );

        let result = merge_translations(vec![de, en]).unwrap();
        assert_eq!(result.content.len(), 1);

        if let StructuredNode::Field(f) = &result.content[0] {
            // Check label is merged
            let label = f.label.as_ref().unwrap();
            if let InlineNode::TranslatedText(map) = &label.0[0] {
                assert_eq!(map.get("de").unwrap(), "Geschlecht");
                assert_eq!(map.get("en").unwrap(), "Gender");
            } else {
                panic!("Expected TranslatedText in label");
            }

            // Check placeholder is merged
            if let Some(TranslatableString::Translated(map)) = &f.placeholder {
                assert_eq!(map.get("de").unwrap(), "Bitte wählen");
                assert_eq!(map.get("en").unwrap(), "Please select");
            } else {
                panic!("Expected translated placeholder");
            }

            // Check radio option names are merged
            if let FieldType::Radio { options } = &f.input_type {
                if let TranslatableString::Translated(map) = &options[0].name {
                    assert_eq!(map.get("de").unwrap(), "Männlich");
                    assert_eq!(map.get("en").unwrap(), "Male");
                } else {
                    panic!("Expected translated option name");
                }
            } else {
                panic!("Expected Radio field type");
            }
        } else {
            panic!("Expected Field");
        }
    }

    #[test]
    fn test_merge_empty() {
        let result = merge_translations(vec![]).unwrap();
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_reject_completely_different_documents() {
        // Create two completely different documents
        let doc1 = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Formular A"),
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(InlineText::plain("Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
            ],
        );
        let doc2 = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Completely different"),
                }),
                StructuredNode::Table(TableNode {
                    header: None,
                    rows: vec![],
                    caption: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "different_field".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Bool,
                    value: None,
                    placeholder: None,
                }),
            ],
        );

        // Should fail with InsufficientStructuralSimilarity
        let result = merge_translations(vec![doc1, doc2]);
        assert!(result.is_err());
        if let Err(MergeError::InsufficientStructuralSimilarity {
            similarity,
            threshold,
        }) = result
        {
            assert!(similarity < threshold);
            assert_eq!(threshold, MIN_STRUCTURAL_SIMILARITY);
        } else {
            panic!("Expected InsufficientStructuralSimilarity error");
        }
    }

    #[test]
    fn test_reject_partially_different_documents() {
        // Create documents with some overlap but not enough
        let doc1 = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Title"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Text 1"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Text 2"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Text 3"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Text 4"),
                }),
            ],
        );
        let doc2 = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Title"),
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
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
                StructuredNode::Field(FieldNode {
                    name: "field2".into(),
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
                StructuredNode::Field(FieldNode {
                    name: "field3".into(),
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
                StructuredNode::Field(FieldNode {
                    name: "field4".into(),
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
        );

        // Should fail - only 1 out of 5 nodes match (20%)
        let result = merge_translations(vec![doc1, doc2]);
        assert!(result.is_err());
    }

    #[test]
    fn test_accept_similar_documents() {
        // Create documents with good structural overlap
        let doc1 = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Formular"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Beschreibung"),
                }),
                StructuredNode::Field(FieldNode {
                    name: "name".into(),
                    som_path: None,
                    label: Some(InlineText::plain("Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "email".into(),
                    som_path: None,
                    label: Some(InlineText::plain("E-Mail")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
            ],
        );
        let doc2 = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Form"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Description"),
                }),
                StructuredNode::Field(FieldNode {
                    name: "name".into(),
                    som_path: None,
                    label: Some(InlineText::plain("Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "email".into(),
                    som_path: None,
                    label: Some(InlineText::plain("Email")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                }),
            ],
        );

        // Should succeed - 100% match
        let result = merge_translations(vec![doc1, doc2]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_reject_duplicate_languages() {
        // Create two documents with the same language code
        let doc1 = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("First document"),
            })],
        );
        let doc2 = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Second document"),
            })],
        );

        // Should fail with DuplicateLanguage error
        let result = merge_translations(vec![doc1, doc2]);
        assert!(result.is_err());
        if let Err(MergeError::DuplicateLanguage { language }) = result {
            assert_eq!(language, "en");
        } else {
            panic!("Expected DuplicateLanguage error");
        }
    }

    #[test]
    fn test_reject_duplicate_languages_among_three() {
        // Create three documents where two have the same language
        let doc1 = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("German"),
            })],
        );
        let doc2 = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("English"),
            })],
        );
        let doc3 = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Another German"),
            })],
        );

        // Should fail with DuplicateLanguage error
        let result = merge_translations(vec![doc1, doc2, doc3]);
        assert!(result.is_err());
        if let Err(MergeError::DuplicateLanguage { language }) = result {
            assert_eq!(language, "de");
        } else {
            panic!("Expected DuplicateLanguage error");
        }
    }
}
