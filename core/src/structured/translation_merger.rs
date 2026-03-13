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
            state_count: 1,
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

    // Log a warning if envelopes have different state counts.
    // Different state counts can occur when one language version's scripts
    // don't differentiate layouts as finely as another's. The merger handles
    // this by matching Conditional nodes by their condition values.
    {
        let first_count = envelopes[0].state_count;
        if envelopes.iter().any(|e| e.state_count != first_count) {
            let details: Vec<String> = envelopes
                .iter()
                .zip(languages.iter())
                .map(|(e, lang)| format!("{}: {}", lang, e.state_count))
                .collect();
            log::warn!(
                "Language versions have different state counts ({}). \
                 Merging by condition values.",
                details.join(", ")
            );
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

    // Start with the first envelope as the base, preserving its context
    // (which contains XFA variables, modules, etc.).
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

    // Create merged context — start from the base context to preserve variables
    // and modules, then update the language to the combined list.
    let mut context = base.context;
    context.set_language(languages.join(","));

    Ok(DocumentEnvelope {
        context,
        content: merged_content,
        state_count: base.state_count,
    })
}

/// Calculate structural similarity between two node lists.
///
/// Returns a value between 0.0 (completely different) and 1.0 (identical structure).
/// Uses the LCS length (with relaxed node matching) as a percentage of the average list length.
///
/// The relaxed matching treats container nodes (Conditional, Group, GridLayout, Repeatable)
/// as matching by type/shape rather than requiring identical deep structure. This correctly
/// handles translation pairs where layout details differ slightly between languages.
fn calculate_structural_similarity(a: &[StructuredNode], b: &[StructuredNode]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let dp = lcs_table_with(a, b, node_matches_for_similarity);
    let lcs_length = dp[a.len()][b.len()] as f64;
    let avg_length = (a.len() + b.len()) as f64 / 2.0;

    lcs_length / avg_length
}

/// Relaxed node matching used only for the similarity pre-check.
///
/// Unlike `structural_eq_ignore_text`, this function matches nodes based on their
/// high-level type and shape without requiring identical deep structure. This allows
/// translation pairs with minor layout differences (different groupings, different
/// conditional content, etc.) to be correctly identified as structurally compatible.
///
/// Rules:
/// - Headings: same level required
/// - Fields: same `FieldType` variant required (FieldIds may differ across languages)
/// - Tables: same header column count required
/// - GridLayouts: same column count required (element count may differ)
/// - Paragraphs, Images, Groups, Conditionals, Repeatables, Lists, Empty: match by type only
fn node_matches_for_similarity(a: &StructuredNode, b: &StructuredNode) -> bool {
    match (a, b) {
        (StructuredNode::Heading(ha), StructuredNode::Heading(hb)) => {
            ha.level.as_u8() == hb.level.as_u8()
        }
        (StructuredNode::Paragraph(_), StructuredNode::Paragraph(_)) => true,
        (StructuredNode::Image(_), StructuredNode::Image(_)) => true,
        (StructuredNode::Table(ta), StructuredNode::Table(tb)) => {
            let a_cols = ta.header.as_ref().map_or(0, |h| h.cells.len());
            let b_cols = tb.header.as_ref().map_or(0, |h| h.cells.len());
            a_cols == b_cols
        }
        (StructuredNode::Field(fa), StructuredNode::Field(fb)) => {
            // Match by FieldType variant — FieldIds are derived from SOM paths
            // which can differ across languages for the same logical field.
            std::mem::discriminant(&fa.input_type) == std::mem::discriminant(&fb.input_type)
        }
        (StructuredNode::Repeatable(_), StructuredNode::Repeatable(_)) => true,
        (StructuredNode::Group(_), StructuredNode::Group(_)) => true,
        (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) => {
            a.condition == b.condition
        }
        (StructuredNode::Empty, StructuredNode::Empty) => true,
        (StructuredNode::GridLayout(ga), StructuredNode::GridLayout(gb)) => {
            ga.columns == gb.columns
        }
        (StructuredNode::List(la), StructuredNode::List(lb)) => la.list_style == lb.list_style,
        _ => false,
    }
}

// ============================================================================
// LCS-based alignment for node lists
// ============================================================================

/// Compute the LCS (longest common subsequence) table for two node slices,
/// using the given equality predicate.
fn lcs_table_with(
    a: &[StructuredNode],
    b: &[StructuredNode],
    eq: impl Fn(&StructuredNode, &StructuredNode) -> bool,
) -> Vec<Vec<usize>> {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if eq(&a[i - 1], &b[j - 1]) {
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

/// Tracks the origin of each entry produced by the alignment phase,
/// so the post-processing step knows which language an unmatched paragraph
/// belongs to.
enum AlignedEntry<'a> {
    /// Both languages matched — the two source nodes are stored so we can
    /// re-derive the plain text when absorbing an adjacent orphan.
    Matched {
        node: StructuredNode,
        base: &'a StructuredNode,
        other: &'a StructuredNode,
    },
    /// Present only in the *base* language.
    BaseOnly(StructuredNode),
    /// Present only in the *other* language.
    OtherOnly(StructuredNode),
}

/// Merge two node lists from different languages using LCS alignment.
fn merge_node_lists(
    base: &[StructuredNode],
    base_lang: &str,
    other: &[StructuredNode],
    other_lang: &str,
) -> Vec<StructuredNode> {
    let dp = lcs_table_with(base, other, StructuredNode::structural_eq_ignore_text);
    let alignment = lcs_align(base, other, &dp);

    let mut entries: Vec<AlignedEntry> = Vec::new();
    for (ai, bi) in &alignment {
        match (ai, bi) {
            (Some(a), Some(b)) => {
                let node = merge_node(&base[*a], base_lang, &other[*b], other_lang);
                entries.push(AlignedEntry::Matched {
                    node,
                    base: &base[*a],
                    other: &other[*b],
                });
            }
            (Some(a), None) => {
                entries.push(AlignedEntry::BaseOnly(base[*a].clone()));
            }
            (None, Some(b)) => {
                entries.push(AlignedEntry::OtherOnly(other[*b].clone()));
            }
            (None, None) => unreachable!(),
        }
    }

    consolidate_orphan_paragraphs(&mut entries, base_lang, other_lang);
    consolidate_orphan_conditionals(&mut entries, base_lang, other_lang);

    entries
        .into_iter()
        .map(|e| match e {
            AlignedEntry::Matched { node, .. } => node,
            AlignedEntry::BaseOnly(node) => node,
            AlignedEntry::OtherOnly(node) => node,
        })
        .collect()
}

/// Post-process aligned entries to absorb orphaned (unmatched) `Paragraph`
/// nodes into an adjacent matched `Paragraph`.
///
/// When one language splits a block of text into multiple paragraphs while
/// another keeps it as a single paragraph, LCS alignment leaves some
/// paragraphs unmatched. This step detects such orphans and prepends/appends
/// their text to the nearest matched paragraph's `TranslatedText` map.
fn consolidate_orphan_paragraphs(
    entries: &mut Vec<AlignedEntry>,
    base_lang: &str,
    other_lang: &str,
) {
    // When one language splits a block of text into N paragraphs while another
    // keeps it as a single paragraph, the LCS alignment matches one paragraph
    // from each side and leaves the remaining N-1 paragraphs unmatched.  Since
    // the LCS backtrace works from the end, the unmatched paragraphs appear
    // *before* the matched one.  This pass detects such orphans and prepends
    // their text into the adjacent matched paragraph's TranslatedText map.
    //
    // We only absorb forward (orphan → next matched paragraph) and stop at any
    // non-paragraph boundary (e.g. a Field or Heading) to avoid merging text
    // across unrelated sections.
    let len = entries.len();
    let mut absorbed = vec![false; len];

    let mut prepend_ops: Vec<(usize, usize, String, String)> = Vec::new();
    for i in 0..len {
        if absorbed[i] {
            continue;
        }

        // Only absorb OtherOnly paragraphs. BaseOnly paragraphs are kept as-is
        // because the base language defines the document structure: its unique
        // paragraphs represent genuinely extra content, while extra paragraphs
        // from other languages are likely split-paragraph artifacts.
        let (orphan_lang, orphan_text) = match &entries[i] {
            AlignedEntry::OtherOnly(StructuredNode::Paragraph(p)) => {
                (other_lang, p.content.as_plain_text())
            }
            _ => continue,
        };
        if orphan_text.is_empty() {
            continue;
        }

        // Search forward for the nearest Matched(Paragraph). Stop if we
        // encounter a non-paragraph entry (e.g. a Field or Heading) that
        // acts as a section boundary.
        let mut target = None;
        for j in (i + 1)..len {
            if absorbed[j] {
                continue;
            }
            match &entries[j] {
                AlignedEntry::Matched {
                    node: StructuredNode::Paragraph(_),
                    ..
                } => {
                    target = Some(j);
                    break;
                }
                // Allow skipping other orphan paragraphs (they'll be
                // absorbed in their own iteration).
                AlignedEntry::OtherOnly(StructuredNode::Paragraph(_)) => continue,
                // Any non-paragraph entry is a barrier — don't absorb
                // across section boundaries.
                _ => break,
            }
        }

        if let Some(j) = target {
            prepend_ops.push((i, j, orphan_text, orphan_lang.to_string()));
            absorbed[i] = true;
        }
    }

    for (_, target, text, lang) in prepend_ops {
        if let AlignedEntry::Matched {
            node: StructuredNode::Paragraph(para),
            ..
        } = &mut entries[target]
        {
            for inline in &mut para.content.0 {
                if let InlineNode::TranslatedText(map) = inline {
                    let entry = map.entry(lang.clone()).or_default();
                    *entry = format!("{}{}", text, entry);
                    break;
                }
            }
        }
    }

    // Remove absorbed entries (iterate in reverse to preserve indices)
    for i in (0..len).rev() {
        if absorbed[i] {
            entries.remove(i);
        }
    }
}

/// Post-process aligned entries to merge orphaned `Conditional` nodes that
/// have the same `FieldCondition` (field + value) but ended up unmatched
/// because the two languages emitted them in a different order.
///
/// For each `BaseOnly(Conditional)` entry, we look for an `OtherOnly(Conditional)`
/// with the same condition. If found, we merge the two nodes and replace the
/// base-only entry with a `Matched` entry, removing the other-only entry.
fn consolidate_orphan_conditionals(
    entries: &mut Vec<AlignedEntry>,
    base_lang: &str,
    other_lang: &str,
) {
    let len = entries.len();
    // Collect indices of OtherOnly(Conditional) entries.
    let mut other_only_indices: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if let AlignedEntry::OtherOnly(StructuredNode::Conditional(_)) = entry {
            other_only_indices.push(i);
        }
    }

    if other_only_indices.is_empty() {
        return;
    }

    // For each BaseOnly(Conditional), find a matching OtherOnly(Conditional)
    // by condition equality. We collect operations first to avoid borrow issues.
    let mut merge_ops: Vec<(usize, usize)> = Vec::new(); // (base_idx, other_idx)
    let mut consumed_other: Vec<bool> = vec![false; other_only_indices.len()];

    for i in 0..len {
        if let AlignedEntry::BaseOnly(StructuredNode::Conditional(base_cond)) = &entries[i] {
            // Linear search for a matching OtherOnly conditional
            for (k, &other_idx) in other_only_indices.iter().enumerate() {
                if consumed_other[k] {
                    continue;
                }
                if let AlignedEntry::OtherOnly(StructuredNode::Conditional(other_cond)) =
                    &entries[other_idx]
                {
                    if base_cond.condition == other_cond.condition {
                        merge_ops.push((i, other_idx));
                        consumed_other[k] = true;
                        break;
                    }
                }
            }
        }
    }

    if merge_ops.is_empty() {
        return;
    }

    // Apply merges: replace BaseOnly with Matched, mark OtherOnly for removal.
    let mut to_remove = vec![false; len];
    for (base_idx, other_idx) in merge_ops {
        // We need to extract the nodes to merge. Use a temporary swap.
        let base_node = std::mem::replace(
            &mut entries[base_idx],
            AlignedEntry::BaseOnly(StructuredNode::Empty),
        );
        let other_node = std::mem::replace(
            &mut entries[other_idx],
            AlignedEntry::OtherOnly(StructuredNode::Empty),
        );

        if let (AlignedEntry::BaseOnly(base_sn), AlignedEntry::OtherOnly(other_sn)) =
            (&base_node, &other_node)
        {
            let merged = merge_node(base_sn, base_lang, other_sn, other_lang);
            entries[base_idx] = AlignedEntry::Matched {
                node: merged,
                // We don't have references to the original nodes anymore since
                // we consumed them, but we can store references to Empty as
                // placeholders — these are only used by consolidate_orphan_paragraphs
                // which runs before us.
                base: &StructuredNode::Empty,
                other: &StructuredNode::Empty,
            };
        }
        to_remove[other_idx] = true;
    }

    // Remove consumed OtherOnly entries (reverse order to preserve indices).
    for i in (0..len).rev() {
        if to_remove[i] {
            entries.remove(i);
        }
    }
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
                list_style: a.list_style,
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
    let mut map = inline_text_to_text_map(base, base_lang);
    map.extend(inline_text_to_text_map(other, other_lang));

    if map.is_empty() {
        InlineText::empty()
    } else {
        InlineText(vec![InlineNode::TranslatedText(map)])
    }
}

/// Extract a language→text map from an `InlineText`.
/// If it contains a single `TranslatedText` node, returns that map.
/// Otherwise returns a single-entry map from the given language to the plain text.
fn inline_text_to_text_map(text: &InlineText, lang: &str) -> HashMap<String, String> {
    if let Some(InlineNode::TranslatedText(existing)) = text.0.first() {
        existing.clone()
    } else {
        let plain = text.as_plain_text();
        if plain.is_empty() {
            HashMap::new()
        } else {
            HashMap::from([(lang.to_string(), plain)])
        }
    }
}

/// Extract a language→text map from an `InlineNode`.
/// `Text` → single-entry map, `TranslatedText` → existing map, others → empty.
fn into_text_map(node: &InlineNode, lang: &str) -> HashMap<String, String> {
    match node {
        InlineNode::Text(s) => HashMap::from([(lang.to_string(), s.clone())]),
        InlineNode::TranslatedText(m) => m.clone(),
        _ => HashMap::new(),
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
        // Text/TranslatedText combinations → merge into a single TranslatedText
        (
            InlineNode::Text(_) | InlineNode::TranslatedText(_),
            InlineNode::Text(_) | InlineNode::TranslatedText(_),
        ) => {
            let mut map = into_text_map(base, base_lang);
            map.extend(into_text_map(other, other_lang));
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

/// Merge two `Option<T>` values, using a merge function when both are `Some`.
/// Preserves the value from either side when only one is present.
fn merge_option<T: Clone>(
    base: &Option<T>,
    other: &Option<T>,
    merge_fn: impl FnOnce(&T, &T) -> T,
) -> Option<T> {
    match (base, other) {
        (Some(a), Some(b)) => Some(merge_fn(a, b)),
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    }
}

/// Merge two `FieldNode`s from different languages.
/// Combines labels, placeholders, and option names into translated forms.
fn merge_field(
    base: &FieldNode,
    base_lang: &str,
    other: &FieldNode,
    other_lang: &str,
) -> FieldNode {
    let label = merge_option(&base.label, &other.label, |a, b| {
        merge_inline_text(a, base_lang, b, other_lang)
    });

    let placeholder = merge_option(&base.placeholder, &other.placeholder, |a, b| {
        a.merge(base_lang, b, other_lang)
    });

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
        .map(|(a, b)| NameValue {
            name: a.name.merge(base_lang, &b.name, other_lang),
            value: a.value.clone(),
        })
        .collect()
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
    let header = merge_option(&base.header, &other.header, |h1, h2| {
        let cells = merge_node_lists(&h1.cells, base_lang, &h2.cells, other_lang);
        TableHeader { cells }
    });

    let rows: Vec<TableRow> = base
        .rows
        .iter()
        .zip(other.rows.iter())
        .map(|(r1, r2)| {
            let cells = merge_node_lists(&r1.cells, base_lang, &r2.cells, other_lang);
            TableRow { cells }
        })
        .collect();

    let caption = merge_option(&base.caption, &other.caption, |a, b| {
        merge_inline_text(a, base_lang, b, other_lang)
    });

    TableNode {
        header,
        rows,
        caption,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{
        HeadingLevel, HeadingNode, InlineText, ParagraphNode, TableHeader, TableNode, TableRow,
    };

    fn make_envelope(lang: &str, content: Vec<StructuredNode>) -> DocumentEnvelope {
        DocumentEnvelope {
            context: Context::with_language(lang),
            content,
            state_count: 1,
        }
    }

    fn make_envelope_with_variables(
        lang: &str,
        variables: HashMap<String, String>,
        content: Vec<StructuredNode>,
    ) -> DocumentEnvelope {
        DocumentEnvelope {
            context: Context::new(lang.to_string(), variables),
            content,
            state_count: 1,
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
    fn test_merge_mismatched_state_counts_succeeds_with_warning() {
        // Mismatched state counts should now produce a warning (not an error)
        // and succeed with condition-based merging.
        let mut de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hallo"),
            })],
        );
        de.state_count = 2;

        let en = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hello"),
            })],
        );

        let result = merge_translations(vec![de, en]);
        assert!(
            result.is_ok(),
            "Mismatched state counts should not be an error"
        );
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

    #[test]
    fn test_merge_preserves_context_variables() {
        let vars: HashMap<String, String> = [
            ("formrange_code".to_string(), "AAAI".to_string()),
            ("formrange_entity".to_string(), "019".to_string()),
        ]
        .into_iter()
        .collect();

        let de = make_envelope_with_variables(
            "de",
            vars.clone(),
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hallo"),
            })],
        );
        let en = make_envelope_with_variables(
            "en",
            vars.clone(),
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Hello"),
            })],
        );

        let merged = merge_translations(vec![de, en]).unwrap();
        assert_eq!(merged.context.language(), "de,en");
        assert_eq!(merged.context.get_variable("formrange_code"), Some("AAAI"));
        assert_eq!(merged.context.get_variable("formrange_entity"), Some("019"));
    }

    #[test]
    fn test_accept_documents_with_differing_conditional_and_gridlayout_structure() {
        // Regression test for: similarity check was rejecting translation pairs where
        // Conditionals had different internal structure or GridLayouts had different
        // element counts but the same column count.
        //
        // Synthetic structure mirroring AACC_019 DE vs EN:
        //   DE: H1, H2, Field(shared), H2, Para, Cond, Cond, Cond, Cond, H2, GridLayout(12, 4 elems)
        //   EN: H1, H2, Field(shared), Para, Cond, Cond, Cond, Cond, H2, GridLayout(12, 2 elems)
        use crate::structured::{
            ConditionalNode, FieldCondition, FieldId, FieldType, GridLayout, GridLayoutElement,
            GroupNode, InputValue,
        };

        let shared_field = FieldNode {
            name: "shared_field".into(),
            som_path: None,
            label: Some(InlineText::plain("Shared")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
        };

        let dummy_condition = FieldCondition {
            field_name: FieldId::from("some_field"),
            value: InputValue::Text("yes".to_string()),
        };

        // DE Conditional wraps a Group with 3 children.
        let de_cond = || {
            StructuredNode::Conditional(ConditionalNode {
                condition: dummy_condition.clone(),
                content: Box::new(StructuredNode::Group(GroupNode {
                    children: vec![
                        StructuredNode::Paragraph(ParagraphNode {
                            content: InlineText::plain("Absatz 1"),
                        }),
                        StructuredNode::Paragraph(ParagraphNode {
                            content: InlineText::plain("Absatz 2"),
                        }),
                        StructuredNode::Paragraph(ParagraphNode {
                            content: InlineText::plain("Absatz 3"),
                        }),
                    ],
                })),
            })
        };

        // EN Conditional wraps a Group with 2 children (structurally different from DE).
        let en_cond = || {
            StructuredNode::Conditional(ConditionalNode {
                condition: dummy_condition.clone(),
                content: Box::new(StructuredNode::Group(GroupNode {
                    children: vec![
                        StructuredNode::Paragraph(ParagraphNode {
                            content: InlineText::plain("Paragraph 1"),
                        }),
                        StructuredNode::Paragraph(ParagraphNode {
                            content: InlineText::plain("Paragraph 2"),
                        }),
                    ],
                })),
            })
        };

        let de_grid = StructuredNode::GridLayout(GridLayout {
            columns: 12,
            elements: vec![
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
            ],
        });

        // EN has 2 elements instead of 4 — different count, same column count.
        let en_grid = StructuredNode::GridLayout(GridLayout {
            columns: 12,
            elements: vec![
                GridLayoutElement {
                    span: 6,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 6,
                    node: StructuredNode::Empty,
                },
            ],
        });

        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Formular"),
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: InlineText::plain("Abschnitt"),
                }),
                StructuredNode::Field(shared_field.clone()),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: InlineText::plain("Hinweis"),
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Erklärung"),
                }),
                de_cond(),
                de_cond(),
                de_cond(),
                de_cond(),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: InlineText::plain("Unterschrift"),
                }),
                de_grid,
            ],
        );

        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: InlineText::plain("Form"),
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: InlineText::plain("Section"),
                }),
                StructuredNode::Field(shared_field.clone()),
                StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Instruction"),
                }),
                en_cond(),
                en_cond(),
                en_cond(),
                en_cond(),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: InlineText::plain("Signature"),
                }),
                en_grid,
            ],
        );

        // Should succeed: relaxed similarity check recognises Conditionals and
        // GridLayouts with the same column count as structurally compatible.
        let result = merge_translations(vec![de, en]);
        assert!(
            result.is_ok(),
            "Expected merge to succeed for documents with differing Conditional/GridLayout internals, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_merge_table_caption_only_in_other_language() {
        // Base (de) has a table with no caption; other (en) has a caption.
        // The caption from the other language should be preserved, not dropped.
        let base = TableNode {
            header: None,
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Zelle"),
                })],
            }],
            caption: None,
        };
        let other = TableNode {
            header: None,
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Cell"),
                })],
            }],
            caption: Some(InlineText::plain("My Table")),
        };

        let merged = merge_table(&base, "de", &other, "en");
        assert!(
            merged.caption.is_some(),
            "Caption from 'en' should not be dropped when base has None"
        );
    }

    #[test]
    fn test_merge_table_header_only_in_other_language() {
        // Base (de) has no header; other (en) has a header.
        // The header should be preserved, not dropped.
        let base = TableNode {
            header: None,
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Zelle"),
                })],
            }],
            caption: None,
        };
        let other = TableNode {
            header: Some(TableHeader {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Column"),
                })],
            }),
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: InlineText::plain("Cell"),
                })],
            }],
            caption: None,
        };

        let merged = merge_table(&base, "de", &other, "en");
        assert!(
            merged.header.is_some(),
            "Header from 'en' should not be dropped when base has None"
        );
    }
}
