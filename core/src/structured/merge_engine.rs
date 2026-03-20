use std::collections::HashMap;

use crate::structured::{
    ConditionalNode, FieldNode, FieldType, GridLayout, GridLayoutElement, GroupNode, HeadingNode,
    InlineNode, InlineText, ListNode, NameValue, ParagraphNode, RepeatableNode, StructuredNode,
    TableHeader, TableNode, TableRow, TranslatableString,
};

pub(crate) const MISSING_TRANSLATION_TEXT: &str = "MISSING TRANSLATION";

/// Compute the LCS (longest common subsequence) table for two node slices,
/// using the given equality predicate.
pub(crate) fn lcs_table_with<F>(
    a: &[StructuredNode],
    b: &[StructuredNode],
    eq: F,
) -> Vec<Vec<usize>>
where
    F: Fn(&StructuredNode, &StructuredNode) -> bool,
{
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
/// Both Some -> matched pair. Only one Some -> unmatched node from that side.
pub(crate) fn lcs_align_with<F>(
    a: &[StructuredNode],
    b: &[StructuredNode],
    dp: &[Vec<usize>],
    eq: F,
) -> Vec<(Option<usize>, Option<usize>)>
where
    F: Fn(&StructuredNode, &StructuredNode) -> bool,
{
    let mut result = Vec::new();
    let mut i = a.len();
    let mut j = b.len();

    let mut matches = Vec::new();
    while i > 0 && j > 0 {
        if eq(&a[i - 1], &b[j - 1]) {
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

    let mut ai = 0;
    let mut bi = 0;
    for (ma, mb) in &matches {
        while ai < *ma {
            result.push((Some(ai), None));
            ai += 1;
        }
        while bi < *mb {
            result.push((None, Some(bi)));
            bi += 1;
        }
        result.push((Some(*ma), Some(*mb)));
        ai = ma + 1;
        bi = mb + 1;
    }

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

/// Merge any Conditional nodes that have the same condition.
/// Content from duplicates is unioned into a single representative node.
pub(crate) fn merge_duplicate_conditionals(nodes: Vec<StructuredNode>) -> Vec<StructuredNode> {
    if nodes.len() < 2 {
        return nodes;
    }

    let has_dup = {
        let mut found = false;
        'outer: for (i, ni) in nodes.iter().enumerate() {
            if let StructuredNode::Conditional(ci) = ni {
                for nj in nodes[i + 1..].iter() {
                    if let StructuredNode::Conditional(cj) = nj {
                        if ci.condition == cj.condition {
                            found = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        found
    };

    if !has_dup {
        return nodes;
    }

    let mut skip = vec![false; nodes.len()];
    let mut extra: Vec<Vec<StructuredNode>> = nodes.iter().map(|_| Vec::new()).collect();

    for j in 1..nodes.len() {
        if let StructuredNode::Conditional(cj) = &nodes[j] {
            for i in 0..j {
                if skip[i] {
                    continue;
                }
                if let StructuredNode::Conditional(ci) = &nodes[i] {
                    if ci.condition == cj.condition {
                        skip[j] = true;
                        match cj.content.as_ref() {
                            StructuredNode::Group(g) => extra[i].extend(g.children.clone()),
                            other => extra[i].push(other.clone()),
                        }
                        break;
                    }
                }
            }
        }
    }

    nodes
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !skip[*i])
        .map(|(i, node)| {
            if extra[i].is_empty() {
                return node;
            }
            if let StructuredNode::Conditional(c) = node {
                let mut children = match *c.content {
                    StructuredNode::Group(g) => g.children,
                    other => vec![other],
                };
                children.append(&mut extra[i]);
                StructuredNode::Conditional(ConditionalNode {
                    condition: c.condition,
                    content: Box::new(StructuredNode::Group(GroupNode { children })),
                })
            } else {
                node
            }
        })
        .collect()
}

pub(crate) fn fill_missing_translation_placeholders(
    nodes: &mut [StructuredNode],
    all_languages: &[String],
    primary_language: &str,
    placeholder: &str,
) {
    for node in nodes {
        fill_node(node, all_languages, primary_language, placeholder);
    }
}

fn fill_node(
    node: &mut StructuredNode,
    all_languages: &[String],
    primary_language: &str,
    placeholder: &str,
) {
    match node {
        StructuredNode::Heading(h) => {
            fill_inline_text(&mut h.content, all_languages, primary_language, placeholder)
        }
        StructuredNode::Paragraph(p) => {
            fill_inline_text(&mut p.content, all_languages, primary_language, placeholder)
        }
        StructuredNode::Field(f) => {
            if let Some(label) = &mut f.label {
                fill_inline_text(label, all_languages, primary_language, placeholder);
            }
            if let Some(ts) = &mut f.placeholder {
                fill_translatable_string(ts, all_languages, primary_language, placeholder);
            }
            fill_field_type(
                &mut f.input_type,
                all_languages,
                primary_language,
                placeholder,
            );
        }
        StructuredNode::Table(t) => {
            if let Some(caption) = &mut t.caption {
                fill_inline_text(caption, all_languages, primary_language, placeholder);
            }
            if let Some(header) = &mut t.header {
                for cell in &mut header.cells {
                    fill_node(cell, all_languages, primary_language, placeholder);
                }
            }
            for row in &mut t.rows {
                for cell in &mut row.cells {
                    fill_node(cell, all_languages, primary_language, placeholder);
                }
            }
        }
        StructuredNode::Group(g) => {
            for child in &mut g.children {
                fill_node(child, all_languages, primary_language, placeholder);
            }
        }
        StructuredNode::Repeatable(r) => {
            fill_node(&mut r.item, all_languages, primary_language, placeholder)
        }
        StructuredNode::Conditional(c) => {
            fill_node(&mut c.content, all_languages, primary_language, placeholder)
        }
        StructuredNode::GridLayout(g) => {
            for element in &mut g.elements {
                fill_node(
                    &mut element.node,
                    all_languages,
                    primary_language,
                    placeholder,
                );
            }
        }
        StructuredNode::List(l) => {
            for item in &mut l.items {
                fill_inline_text(item, all_languages, primary_language, placeholder);
            }
        }
        StructuredNode::Image(_) | StructuredNode::Empty => {}
    }
}

fn fill_field_type(
    input_type: &mut FieldType,
    all_languages: &[String],
    primary_language: &str,
    placeholder: &str,
) {
    match input_type {
        FieldType::Radio { options } | FieldType::Select { options } => {
            for NameValue { name, .. } in options {
                fill_translatable_string(name, all_languages, primary_language, placeholder);
            }
        }
        _ => {}
    }
}

fn fill_inline_text(
    text: &mut InlineText,
    all_languages: &[String],
    primary_language: &str,
    placeholder: &str,
) {
    for node in &mut text.0 {
        fill_inline_node(node, all_languages, primary_language, placeholder);
    }
}

fn fill_inline_node(
    node: &mut InlineNode,
    all_languages: &[String],
    primary_language: &str,
    placeholder: &str,
) {
    match node {
        InlineNode::Text(s) => {
            let mut map = HashMap::new();
            map.insert(primary_language.to_string(), s.clone());
            ensure_all_languages(&mut map, all_languages, placeholder);
            *node = InlineNode::TranslatedText(map);
        }
        InlineNode::TranslatedText(map) => ensure_all_languages(map, all_languages, placeholder),
        InlineNode::Link(link) => fill_inline_text(
            &mut link.content,
            all_languages,
            primary_language,
            placeholder,
        ),
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            fill_inline_node(inner, all_languages, primary_language, placeholder)
        }
    }
}

fn fill_translatable_string(
    value: &mut TranslatableString,
    all_languages: &[String],
    primary_language: &str,
    placeholder: &str,
) {
    match value {
        TranslatableString::Plain(s) => {
            let mut map = HashMap::new();
            map.insert(primary_language.to_string(), s.clone());
            ensure_all_languages(&mut map, all_languages, placeholder);
            *value = TranslatableString::Translated(map);
        }
        TranslatableString::Translated(map) => {
            ensure_all_languages(map, all_languages, placeholder)
        }
    }
}

fn ensure_all_languages(map: &mut HashMap<String, String>, langs: &[String], placeholder: &str) {
    for lang in langs {
        match map.get_mut(lang) {
            Some(value) if value.trim().is_empty() => {
                *value = placeholder.to_string();
            }
            Some(_) => {}
            None => {
                map.insert(lang.clone(), placeholder.to_string());
            }
        }
    }
}

// ============================================================================
// Policy infrastructure for parameterised merge strategies
// ============================================================================

/// Context for a pairwise node-list merge.
pub(crate) struct PairwiseMergeCtx<'a> {
    pub base_lang: &'a str,
    pub other_lang: &'a str,
}

/// Aligned entry produced by LCS alignment, tagged with its origin.
///
/// The inner `StructuredNode` is already merged (for `Matched`) but NOT
/// localised (for `LeftOnly` / `RightOnly`).  Consolidation passes can
/// therefore inspect un-localised content before the final collection step
/// applies [`localize_structured_node`].
pub(crate) enum AlignedNode {
    /// Both sides matched — contains the already-merged node.
    Matched(StructuredNode),
    /// Present only on the left (base) side.  Contains the original node.
    LeftOnly(StructuredNode),
    /// Present only on the right (other) side.  Contains the original node.
    RightOnly(StructuredNode),
}

/// Policy trait for resolving conflicts during LCS-based node alignment.
///
/// Implementors define how nodes are matched and how matched pairs are merged.
/// Unmatched nodes are kept raw in [`AlignedNode`] and processed by the caller.
pub(crate) trait MergePolicy {
    /// Decide whether two nodes should be paired during LCS alignment.
    fn nodes_match(a: &StructuredNode, b: &StructuredNode) -> bool;

    /// Combine two paired nodes into one.
    fn merge_matched(
        ctx: &PairwiseMergeCtx,
        a: &StructuredNode,
        b: &StructuredNode,
    ) -> StructuredNode;
}

/// Align two node lists via LCS and resolve each entry through the given
/// [`MergePolicy`].  Returns tagged [`AlignedNode`] entries so callers can
/// post-process (e.g. orphan consolidation) before collecting the final list.
///
/// When nodes carry SOM path hints, matching SOM paths act as hard anchors
/// that force alignment.  The algorithm:
///  1. Collect monotonically-increasing anchor pairs from unique SOM matches.
///  2. Run LCS independently on each segment between anchors.
///  3. Anchors always produce `Matched` entries.
pub(crate) fn align_and_tag<P: MergePolicy>(
    ctx: &PairwiseMergeCtx,
    base: &[StructuredNode],
    other: &[StructuredNode],
) -> Vec<AlignedNode> {
    let anchors = find_som_anchors(base, other);

    if anchors.is_empty() {
        // No anchors — plain LCS over the whole lists.
        return align_segment::<P>(ctx, base, other);
    }

    let mut result = Vec::new();
    let mut ai = 0usize;
    let mut bi = 0usize;

    for (anchor_a, anchor_b) in &anchors {
        // Align the segment before this anchor.
        result.extend(align_segment::<P>(ctx, &base[ai..*anchor_a], &other[bi..*anchor_b]));

        // Emit the anchor pair as Matched.
        result.push(AlignedNode::Matched(P::merge_matched(
            ctx,
            &base[*anchor_a],
            &other[*anchor_b],
        )));

        ai = anchor_a + 1;
        bi = anchor_b + 1;
    }

    // Align the tail after the last anchor.
    result.extend(align_segment::<P>(ctx, &base[ai..], &other[bi..]));

    result
}

/// Run LCS alignment on a single segment (sub-slice) of nodes.
fn align_segment<P: MergePolicy>(
    ctx: &PairwiseMergeCtx,
    base: &[StructuredNode],
    other: &[StructuredNode],
) -> Vec<AlignedNode> {
    let dp = lcs_table_with(base, other, P::nodes_match);
    let alignment = lcs_align_with(base, other, &dp, P::nodes_match);

    alignment
        .into_iter()
        .map(|(ai, bi)| match (ai, bi) {
            (Some(a), Some(b)) => AlignedNode::Matched(P::merge_matched(ctx, &base[a], &other[b])),
            (Some(a), None) => AlignedNode::LeftOnly(base[a].clone()),
            (None, Some(b)) => AlignedNode::RightOnly(other[b].clone()),
            (None, None) => unreachable!(),
        })
        .collect()
}

/// Find monotonically-increasing anchor pairs where both sides share the same
/// SOM path.  Only SOM paths that appear exactly once in each list qualify.
fn find_som_anchors(
    base: &[StructuredNode],
    other: &[StructuredNode],
) -> Vec<(usize, usize)> {
    use std::collections::HashMap;

    // Build index: som_path → position (only keep paths that appear exactly once).
    let collect_unique = |nodes: &[StructuredNode]| -> HashMap<String, usize> {
        let mut counts: HashMap<String, (usize, usize)> = HashMap::new(); // path → (first_index, count)
        for (i, node) in nodes.iter().enumerate() {
            if let Some(sp) = node.som_path() {
                counts
                    .entry(sp.as_str().to_owned())
                    .and_modify(|(_, c)| *c += 1)
                    .or_insert((i, 1));
            }
        }
        counts
            .into_iter()
            .filter(|(_, (_, c))| *c == 1)
            .map(|(path, (idx, _))| (path, idx))
            .collect()
    };

    let base_unique = collect_unique(base);
    let other_unique = collect_unique(other);

    // Collect matching pairs where the SOM path is unique in both lists.
    let mut pairs: Vec<(usize, usize)> = base_unique
        .iter()
        .filter_map(|(path, &a_idx)| other_unique.get(path).map(|&b_idx| (a_idx, b_idx)))
        .collect();

    // Sort by base index for LIS computation.
    pairs.sort_by_key(|(a, _)| *a);

    // Compute longest increasing subsequence on b-indices using patience sorting.
    if pairs.is_empty() {
        return Vec::new();
    }

    let b_vals: Vec<usize> = pairs.iter().map(|(_, b)| *b).collect();
    let n = b_vals.len();
    // tails[i] = smallest tail element for IS of length i+1
    let mut tails: Vec<usize> = Vec::new();
    // parent tracking for reconstruction
    let mut indices: Vec<usize> = Vec::new(); // index into tails
    let mut parent: Vec<Option<usize>> = vec![None; n];

    for i in 0..n {
        let pos = tails.partition_point(|&t| t < b_vals[i]);
        if pos == tails.len() {
            tails.push(b_vals[i]);
            indices.push(i);
        } else {
            tails[pos] = b_vals[i];
            indices[pos] = i;
        }
        if pos > 0 {
            parent[i] = Some(indices[pos - 1]);
        }
    }

    // Reconstruct the LIS
    let mut lis = Vec::with_capacity(tails.len());
    let mut idx = *indices.last().unwrap();
    loop {
        lis.push(pairs[idx]);
        if let Some(p) = parent[idx] {
            idx = p;
        } else {
            break;
        }
    }
    lis.reverse();

    lis
}

// ============================================================================
// Translation merge policy
// ============================================================================

/// Policy that merges two single-language trees into a bilingual tree.
///
/// * Matching uses relaxed type/shape comparison ([`node_matches_for_similarity`]).
/// * Matched nodes are merged recursively by combining text into `TranslatedText`.
/// * Unmatched nodes are kept raw for later localisation.
pub(crate) struct TranslationPolicy;

impl MergePolicy for TranslationPolicy {
    fn nodes_match(a: &StructuredNode, b: &StructuredNode) -> bool {
        node_matches_for_similarity(a, b)
    }

    fn merge_matched(
        ctx: &PairwiseMergeCtx,
        a: &StructuredNode,
        b: &StructuredNode,
    ) -> StructuredNode {
        merge_node(a, ctx.base_lang, b, ctx.other_lang)
    }
}

// ============================================================================
// Node matching (relaxed, for translation alignment)
// ============================================================================

/// Relaxed node matching for translation alignment and similarity pre-check.
///
/// Matches nodes based on their high-level type and shape without requiring
/// identical deep structure.  This allows translation pairs with minor layout
/// differences to be correctly aligned.
///
/// Rules:
/// - Headings: same level required
/// - Fields: same `FieldType` variant required (FieldIds may differ across languages)
/// - Tables: same header column count required
/// - GridLayouts: same column count required (element count may differ)
/// - Paragraphs, Images, Groups, Conditionals, Repeatables, Lists, Empty: match by type only
pub(crate) fn node_matches_for_similarity(a: &StructuredNode, b: &StructuredNode) -> bool {
    // If both nodes carry the same SOM path and the same top-level variant,
    // they match regardless of content/shape differences.
    if std::mem::discriminant(a) == std::mem::discriminant(b) {
        if let (Some(sa), Some(sb)) = (a.som_path(), b.som_path()) {
            if sa.as_str() == sb.as_str() {
                return true;
            }
        }
    }

    match (a, b) {
        (StructuredNode::Heading(ha), StructuredNode::Heading(hb)) => {
            ha.level.as_u8() == hb.level.as_u8()
                && inline_text_shape_compatible(&ha.content, &hb.content)
        }
        (StructuredNode::Paragraph(pa), StructuredNode::Paragraph(pb)) => {
            inline_text_shape_compatible(&pa.content, &pb.content)
        }
        (StructuredNode::Image(_), StructuredNode::Image(_)) => true,
        (StructuredNode::Table(ta), StructuredNode::Table(tb)) => {
            let a_cols = ta.header.as_ref().map_or(0, |h| h.cells.len());
            let b_cols = tb.header.as_ref().map_or(0, |h| h.cells.len());
            a_cols == b_cols
        }
        (StructuredNode::Field(fa), StructuredNode::Field(fb)) => {
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
        // Conditional vs non-Conditional: when one language wraps content in a
        // Conditional (due to exhaustive-state differences) and the other doesn't,
        // try matching the conditional's inner content against the bare node.
        (StructuredNode::Conditional(c), other) => {
            node_matches_for_similarity(&c.content, other)
        }
        (other, StructuredNode::Conditional(c)) => {
            node_matches_for_similarity(other, &c.content)
        }
        _ => false,
    }
}

fn inline_text_shape_compatible(a: &InlineText, b: &InlineText) -> bool {
    text_shape_compatible(
        &stable_inline_text_projection(a),
        &stable_inline_text_projection(b),
    )
}

fn stable_inline_text_projection(text: &InlineText) -> String {
    let mut out = String::new();
    for node in &text.0 {
        append_stable_inline_node_projection(node, &mut out);
    }
    out
}

fn append_stable_inline_node_projection(node: &InlineNode, out: &mut String) {
    match node {
        InlineNode::Text(s) => out.push_str(s),
        InlineNode::TranslatedText(map) => {
            if let Some((_lang, value)) = map.iter().min_by_key(|(lang, _)| *lang) {
                out.push_str(value);
            }
        }
        InlineNode::Link(link) => {
            for child in &link.content.0 {
                append_stable_inline_node_projection(child, out);
            }
        }
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            append_stable_inline_node_projection(inner, out)
        }
    }
}

fn text_shape_compatible(a: &str, b: &str) -> bool {
    let shape_a = text_shape(a);
    let shape_b = text_shape(b);

    if shape_a.chars == 0 && shape_b.chars == 0 {
        return true;
    }
    if shape_a.chars == 0 || shape_b.chars == 0 {
        return false;
    }

    // Similarity gate by coarse structure. Language content can differ,
    // but paragraph/heading shapes should be in the same rough range.
    let char_ratio = ratio(shape_a.chars, shape_b.chars);
    let word_ratio = ratio(shape_a.words.max(1), shape_b.words.max(1));
    let digit_delta = shape_a.digits.abs_diff(shape_b.digits);
    let punct_delta = shape_a.punct.abs_diff(shape_b.punct);

    let short_text = shape_a.chars.max(shape_b.chars) <= 32;
    let max_char_ratio = if short_text { 3.5 } else { 2.2 };

    char_ratio <= max_char_ratio && word_ratio <= 2.5 && digit_delta <= 3 && punct_delta <= 8
}

fn ratio(a: usize, b: usize) -> f64 {
    let max_v = a.max(b) as f64;
    let min_v = a.min(b).max(1) as f64;
    max_v / min_v
}

#[derive(Clone, Copy)]
struct TextShape {
    chars: usize,
    words: usize,
    digits: usize,
    punct: usize,
}

fn text_shape(input: &str) -> TextShape {
    let chars = input.chars().filter(|c| !c.is_whitespace()).count();
    let words = input.split_whitespace().count();
    let digits = input.chars().filter(|c| c.is_ascii_digit()).count();
    let punct = input.chars().filter(|c| c.is_ascii_punctuation()).count();

    TextShape {
        chars,
        words,
        digits,
        punct,
    }
}

// ============================================================================
// Localisation — tag a node tree with a single source language
// ============================================================================

fn localize_inline_node(node: &InlineNode, lang: &str) -> InlineNode {
    match node {
        InlineNode::Text(text) => {
            InlineNode::TranslatedText(HashMap::from([(lang.to_string(), text.clone())]))
        }
        InlineNode::TranslatedText(map) => InlineNode::TranslatedText(map.clone()),
        InlineNode::Link(link) => InlineNode::Link(crate::structured::LinkNode {
            href: link.href.clone(),
            content: localize_inline_text(&link.content, lang),
        }),
        InlineNode::Strong(inner) => {
            InlineNode::Strong(Box::new(localize_inline_node(inner, lang)))
        }
        InlineNode::Emphasis(inner) => {
            InlineNode::Emphasis(Box::new(localize_inline_node(inner, lang)))
        }
    }
}

fn localize_inline_text(text: &InlineText, lang: &str) -> InlineText {
    InlineText(
        text.0
            .iter()
            .map(|node| localize_inline_node(node, lang))
            .collect(),
    )
}

fn localize_translatable_string(value: &TranslatableString, lang: &str) -> TranslatableString {
    match value {
        TranslatableString::Plain(text) => {
            TranslatableString::Translated(HashMap::from([(lang.to_string(), text.clone())]))
        }
        TranslatableString::Translated(map) => TranslatableString::Translated(map.clone()),
    }
}

fn localize_field_type(field_type: &FieldType, lang: &str) -> FieldType {
    match field_type {
        FieldType::Radio { options } => FieldType::Radio {
            options: options
                .iter()
                .map(|option| NameValue {
                    name: localize_translatable_string(&option.name, lang),
                    value: option.value.clone(),
                })
                .collect(),
        },
        FieldType::Select { options } => FieldType::Select {
            options: options
                .iter()
                .map(|option| NameValue {
                    name: localize_translatable_string(&option.name, lang),
                    value: option.value.clone(),
                })
                .collect(),
        },
        _ => field_type.clone(),
    }
}

fn localize_structured_node(node: &StructuredNode, lang: &str) -> StructuredNode {
    match node {
        StructuredNode::Heading(heading) => StructuredNode::Heading(HeadingNode {
            level: heading.level,
            content: localize_inline_text(&heading.content, lang),
            som_path: heading.som_path.clone(),
        }),
        StructuredNode::Paragraph(paragraph) => StructuredNode::Paragraph(ParagraphNode {
            content: localize_inline_text(&paragraph.content, lang),
            som_path: paragraph.som_path.clone(),
        }),
        StructuredNode::Image(image) => StructuredNode::Image(image.clone()),
        StructuredNode::Table(table) => StructuredNode::Table(TableNode {
            header: table.header.as_ref().map(|header| TableHeader {
                cells: header
                    .cells
                    .iter()
                    .map(|cell| localize_structured_node(cell, lang))
                    .collect(),
            }),
            rows: table
                .rows
                .iter()
                .map(|row| TableRow {
                    cells: row
                        .cells
                        .iter()
                        .map(|cell| localize_structured_node(cell, lang))
                        .collect(),
                })
                .collect(),
            caption: table
                .caption
                .as_ref()
                .map(|caption| localize_inline_text(caption, lang)),
        }),
        StructuredNode::Field(field) => StructuredNode::Field(FieldNode {
            name: field.name.clone(),
            som_path: field.som_path.clone(),
            label: field
                .label
                .as_ref()
                .map(|label| localize_inline_text(label, lang)),
            input_type: localize_field_type(&field.input_type, lang),
            value: field.value.clone(),
            placeholder: field
                .placeholder
                .as_ref()
                .map(|placeholder| localize_translatable_string(placeholder, lang)),
        }),
        StructuredNode::Repeatable(repeatable) => StructuredNode::Repeatable(RepeatableNode {
            item: Box::new(localize_structured_node(&repeatable.item, lang)),
            min_occurrences: repeatable.min_occurrences,
            max_occurrences: repeatable.max_occurrences,
        }),
        StructuredNode::Group(group) => StructuredNode::Group(GroupNode {
            children: group
                .children
                .iter()
                .map(|child| localize_structured_node(child, lang))
                .collect(),
        }),
        StructuredNode::Conditional(conditional) => StructuredNode::Conditional(ConditionalNode {
            condition: conditional.condition.clone(),
            content: Box::new(localize_structured_node(&conditional.content, lang)),
        }),
        StructuredNode::Empty => StructuredNode::Empty,
        StructuredNode::GridLayout(grid) => StructuredNode::GridLayout(GridLayout {
            columns: grid.columns,
            elements: grid
                .elements
                .iter()
                .map(|element| GridLayoutElement {
                    span: element.span,
                    node: localize_structured_node(&element.node, lang),
                })
                .collect(),
        }),
        StructuredNode::List(list) => StructuredNode::List(ListNode {
            list_style: list.list_style,
            items: list
                .items
                .iter()
                .map(|item| localize_inline_text(item, lang))
                .collect(),
        }),
    }
}

// ============================================================================
// Translation merge — node list with consolidation
// ============================================================================

fn collect_inline_languages(node: &InlineNode, langs: &mut Vec<String>) {
    match node {
        InlineNode::TranslatedText(map) => {
            for lang in map.keys() {
                if !langs.contains(lang) {
                    langs.push(lang.clone());
                }
            }
        }
        InlineNode::Link(link) => {
            for child in &link.content.0 {
                collect_inline_languages(child, langs);
            }
        }
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            collect_inline_languages(inner, langs);
        }
        InlineNode::Text(_) => {}
    }
}

fn collect_inline_text_languages(text: &InlineText) -> Vec<String> {
    let mut langs = Vec::new();
    for node in &text.0 {
        collect_inline_languages(node, &mut langs);
    }
    langs
}

fn matched_paragraph_has_nonempty_language(entry: &AlignedNode, lang: &str) -> bool {
    if let AlignedNode::Matched(StructuredNode::Paragraph(para)) = entry {
        for inline in &para.content.0 {
            if let InlineNode::TranslatedText(map) = inline {
                if let Some(value) = map.get(lang) {
                    if !value.trim().is_empty() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub(crate) fn prepend_orphan_text_to_matched_paragraph(
    entry: &mut AlignedNode,
    text: &str,
    lang: &str,
    base_lang: &str,
    other_lang: &str,
) -> bool {
    if let AlignedNode::Matched(StructuredNode::Paragraph(para)) = entry {
        if let Some(InlineNode::TranslatedText(map)) = para.content.0.first_mut() {
            map.entry(base_lang.to_string()).or_default();
            map.entry(other_lang.to_string()).or_default();
            let existing = map.entry(lang.to_string()).or_default();
            *existing = format!("{}{}", text, existing);
            return true;
        }

        let mut map: HashMap<String, String> = collect_inline_text_languages(&para.content)
            .into_iter()
            .map(|existing_lang| (existing_lang, String::new()))
            .collect();
        map.entry(base_lang.to_string()).or_default();
        map.entry(other_lang.to_string()).or_default();
        map.insert(lang.to_string(), text.to_string());

        para.content.0.insert(0, InlineNode::TranslatedText(map));
        return true;
    }

    false
}

/// Merge two node lists from different languages using LCS alignment.
///
/// Uses the [`TranslationPolicy`] for alignment and merging, then runs
/// orphan consolidation passes to absorb split-paragraph artifacts and
/// reorder-mismatched conditionals.
pub(crate) fn merge_node_lists(
    base: &[StructuredNode],
    base_lang: &str,
    other: &[StructuredNode],
    other_lang: &str,
) -> Vec<StructuredNode> {
    let ctx = PairwiseMergeCtx {
        base_lang,
        other_lang,
    };
    let mut entries = align_and_tag::<TranslationPolicy>(&ctx, base, other);

    consolidate_orphan_paragraphs(&mut entries, base_lang, other_lang);
    consolidate_orphan_conditionals(&mut entries, base_lang, other_lang);

    entries
        .into_iter()
        .map(|e| match e {
            AlignedNode::Matched(node) => node,
            AlignedNode::LeftOnly(node) => localize_structured_node(&node, base_lang),
            AlignedNode::RightOnly(node) => localize_structured_node(&node, other_lang),
        })
        .collect()
}

/// Post-process aligned entries to absorb orphaned (unmatched) `Paragraph`
/// nodes into an adjacent matched `Paragraph`.
///
/// When one language splits text into multiple paragraphs while another keeps
/// it as a single paragraph, LCS alignment leaves some paragraphs unmatched.
/// This step detects such orphans and prepends their text to the nearest
/// matched paragraph's `TranslatedText` map.
fn consolidate_orphan_paragraphs(
    entries: &mut Vec<AlignedNode>,
    base_lang: &str,
    other_lang: &str,
) {
    let len = entries.len();
    let mut absorbed = vec![false; len];

    let mut prepend_ops: Vec<(usize, usize, String, String)> = Vec::new();
    for i in 0..len {
        if absorbed[i] {
            continue;
        }

        // Absorb single-language orphan paragraphs from either side.
        let (orphan_lang, orphan_text) = match &entries[i] {
            AlignedNode::LeftOnly(StructuredNode::Paragraph(p)) => {
                let langs = collect_inline_text_languages(&p.content);
                if langs.len() > 1 {
                    // Already multilingual: do not flatten it into one language key.
                    continue;
                }
                let lang = langs
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| base_lang.to_string());
                (lang, p.content.as_plain_text())
            }
            AlignedNode::RightOnly(StructuredNode::Paragraph(p)) => {
                (other_lang.to_string(), p.content.as_plain_text())
            }
            _ => continue,
        };
        if orphan_text.is_empty() {
            continue;
        }

        // Search forward for the nearest Matched(Paragraph).
        let mut target = None;
        for j in (i + 1)..len {
            if absorbed[j] {
                continue;
            }
            match &entries[j] {
                AlignedNode::Matched(StructuredNode::Paragraph(_)) => {
                    let is_left_orphan = orphan_lang == base_lang;
                    if !is_left_orphan
                        || !matched_paragraph_has_nonempty_language(&entries[j], &orphan_lang)
                    {
                        target = Some(j);
                        break;
                    }
                }
                AlignedNode::LeftOnly(StructuredNode::Paragraph(_)) => continue,
                AlignedNode::RightOnly(StructuredNode::Paragraph(_)) => continue,
                _ => break,
            }
        }

        // If none found forward, search backward for nearest Matched(Paragraph).
        if target.is_none() {
            let mut j = i;
            while j > 0 {
                j -= 1;
                if absorbed[j] {
                    continue;
                }
                match &entries[j] {
                    AlignedNode::Matched(StructuredNode::Paragraph(_)) => {
                        let is_left_orphan = orphan_lang == base_lang;
                        if !is_left_orphan
                            || !matched_paragraph_has_nonempty_language(&entries[j], &orphan_lang)
                        {
                            target = Some(j);
                            break;
                        }
                    }
                    AlignedNode::LeftOnly(StructuredNode::Paragraph(_)) => continue,
                    AlignedNode::RightOnly(StructuredNode::Paragraph(_)) => continue,
                    _ => break,
                }
            }
        }

        if let Some(j) = target {
            prepend_ops.push((i, j, orphan_text, orphan_lang.to_string()));
        }
    }

    for (orphan_idx, target, text, lang) in prepend_ops.into_iter().rev() {
        if prepend_orphan_text_to_matched_paragraph(
            &mut entries[target],
            &text,
            &lang,
            base_lang,
            other_lang,
        ) {
            absorbed[orphan_idx] = true;
        }
    }

    for i in (0..len).rev() {
        if absorbed[i] {
            entries.remove(i);
        }
    }
}

/// Post-process aligned entries to merge orphaned `Conditional` nodes that
/// have the same `FieldCondition` but ended up unmatched because the two
/// languages emitted them in a different order.
fn consolidate_orphan_conditionals(
    entries: &mut Vec<AlignedNode>,
    base_lang: &str,
    other_lang: &str,
) {
    let len = entries.len();
    let mut other_only_indices: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if let AlignedNode::RightOnly(StructuredNode::Conditional(_)) = entry {
            other_only_indices.push(i);
        }
    }

    if other_only_indices.is_empty() {
        return;
    }

    let mut merge_ops: Vec<(usize, usize)> = Vec::new();
    let mut consumed_other: Vec<bool> = vec![false; other_only_indices.len()];

    for i in 0..len {
        if let AlignedNode::LeftOnly(StructuredNode::Conditional(base_cond)) = &entries[i] {
            for (k, &other_idx) in other_only_indices.iter().enumerate() {
                if consumed_other[k] {
                    continue;
                }
                if let AlignedNode::RightOnly(StructuredNode::Conditional(other_cond)) =
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

    let mut to_remove = vec![false; len];
    for (base_idx, other_idx) in merge_ops {
        let base_node = std::mem::replace(
            &mut entries[base_idx],
            AlignedNode::LeftOnly(StructuredNode::Empty),
        );
        let other_node = std::mem::replace(
            &mut entries[other_idx],
            AlignedNode::RightOnly(StructuredNode::Empty),
        );

        if let (AlignedNode::LeftOnly(base_sn), AlignedNode::RightOnly(other_sn)) =
            (&base_node, &other_node)
        {
            let merged = merge_node(base_sn, base_lang, other_sn, other_lang);
            entries[base_idx] = AlignedNode::Matched(merged);
        }
        to_remove[other_idx] = true;
    }

    for i in (0..len).rev() {
        if to_remove[i] {
            entries.remove(i);
        }
    }
}

// ============================================================================
// Recursive node merging (translation)
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
                som_path: a.som_path.clone(),
            })
        }
        (StructuredNode::Paragraph(a), StructuredNode::Paragraph(b)) => {
            StructuredNode::Paragraph(ParagraphNode {
                content: merge_inline_text(&a.content, base_lang, &b.content, other_lang),
                som_path: a.som_path.clone(),
            })
        }
        (StructuredNode::Image(a), StructuredNode::Image(_b)) => StructuredNode::Image(a.clone()),
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
            let elements = merge_grid_elements(&a.elements, base_lang, &b.elements, other_lang);
            StructuredNode::GridLayout(GridLayout {
                columns: a.columns,
                elements,
            })
        }
        (StructuredNode::List(a), StructuredNode::List(b)) => {
            let items = merge_list_items(&a.items, base_lang, &b.items, other_lang);
            StructuredNode::List(ListNode {
                list_style: a.list_style,
                items,
            })
        }
        // Mismatched variants can occur in recursive cases (e.g. different
        // sub-structures inside Repeatable or Conditional content).  Keep base.
        //
        // Conditional vs non-Conditional: one language may wrap content in a
        // Conditional due to exhaustive-state differences. Merge the inner
        // content and preserve the Conditional wrapper.
        (StructuredNode::Conditional(c), other) => {
            StructuredNode::Conditional(ConditionalNode {
                condition: c.condition.clone(),
                content: Box::new(merge_node(&c.content, base_lang, other, other_lang)),
            })
        }
        (other, StructuredNode::Conditional(c)) => {
            StructuredNode::Conditional(ConditionalNode {
                condition: c.condition.clone(),
                content: Box::new(merge_node(other, base_lang, &c.content, other_lang)),
            })
        }
        _ => base.clone(),
    }
}

// ============================================================================
// InlineText merging
// ============================================================================

/// Merge two `InlineText`s from different languages.
///
/// If both have the same number of inline nodes with matching types, merge
/// element-wise.  Otherwise, produce a single `TranslatedText` node from each
/// side's plain text.
fn merge_inline_text(
    base: &InlineText,
    base_lang: &str,
    other: &InlineText,
    other_lang: &str,
) -> InlineText {
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

    let mut map = inline_text_to_text_map(base, base_lang);
    map.extend(inline_text_to_text_map(other, other_lang));

    if map.is_empty() {
        InlineText::empty()
    } else {
        InlineText(vec![InlineNode::TranslatedText(map)])
    }
}

/// Extract a language→text map from an `InlineText`.
fn inline_text_to_text_map(text: &InlineText, lang: &str) -> HashMap<String, String> {
    if text.0.len() == 1 {
        if let Some(InlineNode::TranslatedText(existing)) = text.0.first() {
            return existing.clone();
        }
    }

    let plain = text.as_plain_text();
    if plain.is_empty() {
        HashMap::new()
    } else {
        HashMap::from([(lang.to_string(), plain)])
    }
}

/// Extract a language→text map from an `InlineNode`.
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
        (
            InlineNode::Text(_) | InlineNode::TranslatedText(_),
            InlineNode::Text(_) | InlineNode::TranslatedText(_),
        ) => {
            let mut map = into_text_map(base, base_lang);
            map.extend(into_text_map(other, other_lang));
            InlineNode::TranslatedText(map)
        }
        (InlineNode::Strong(a), InlineNode::Strong(b)) => {
            InlineNode::Strong(Box::new(merge_inline_node(a, base_lang, b, other_lang)))
        }
        (InlineNode::Emphasis(a), InlineNode::Emphasis(b)) => {
            InlineNode::Emphasis(Box::new(merge_inline_node(a, base_lang, b, other_lang)))
        }
        (InlineNode::Link(a), InlineNode::Link(b)) => {
            InlineNode::Link(crate::structured::LinkNode {
                href: a.href.clone(),
                content: merge_inline_text(&a.content, base_lang, &b.content, other_lang),
            })
        }
        // Mismatched variants — shouldn't happen after variant check, but
        // keep base as a safe fallback.
        _ => base.clone(),
    }
}

// ============================================================================
// Field merging
// ============================================================================

/// Merge two `Option<T>` values, using a merge function when both are `Some`.
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
        _ => base.clone(),
    }
}

/// Merge two `GridLayout` element vectors.
fn merge_grid_elements(
    base: &[GridLayoutElement],
    base_lang: &str,
    other: &[GridLayoutElement],
    other_lang: &str,
) -> Vec<GridLayoutElement> {
    if base.len() != other.len() {
        log::warn!(
            "GridLayout element count mismatch when merging {} and {} translations: \
             {} vs {} elements; unmatched elements will be preserved from the longer side",
            base_lang,
            other_lang,
            base.len(),
            other.len()
        );
    }
    let paired = base.len().min(other.len());
    let mut elements: Vec<GridLayoutElement> = base
        .iter()
        .zip(other.iter())
        .map(|(ea, eb)| GridLayoutElement {
            span: ea.span,
            node: merge_node(&ea.node, base_lang, &eb.node, other_lang),
        })
        .collect();
    elements.extend(base[paired..].iter().cloned());
    elements.extend(other[paired..].iter().cloned());
    elements
}

/// Merge two `List` item vectors.
fn merge_list_items(
    base: &[InlineText],
    base_lang: &str,
    other: &[InlineText],
    other_lang: &str,
) -> Vec<InlineText> {
    if base.len() != other.len() {
        log::warn!(
            "List item count mismatch when merging {} and {} translations: \
             {} vs {} items; unmatched items will be preserved from the longer side",
            base_lang,
            other_lang,
            base.len(),
            other.len()
        );
    }
    let paired = base.len().min(other.len());
    let mut items: Vec<InlineText> = base
        .iter()
        .zip(other.iter())
        .map(|(ia, ib)| merge_inline_text(ia, base_lang, ib, other_lang))
        .collect();
    for ia in &base[paired..] {
        let map = inline_text_to_text_map(ia, base_lang);
        items.push(if map.is_empty() {
            ia.clone()
        } else {
            InlineText(vec![InlineNode::TranslatedText(map)])
        });
    }
    for ib in &other[paired..] {
        let map = inline_text_to_text_map(ib, other_lang);
        items.push(if map.is_empty() {
            ib.clone()
        } else {
            InlineText(vec![InlineNode::TranslatedText(map)])
        });
    }
    items
}

/// Merge two `NameValue` vectors by zipping and merging names.
fn merge_name_values(
    base: &[NameValue],
    base_lang: &str,
    other: &[NameValue],
    other_lang: &str,
) -> Vec<NameValue> {
    if base.len() != other.len() {
        log::warn!(
            "Option count mismatch when merging {} and {} translations: \
             {} vs {} options; unmatched options will be preserved as \
             single-language entries",
            base_lang,
            other_lang,
            base.len(),
            other.len()
        );
    }
    let paired = base.len().min(other.len());
    let mut options: Vec<NameValue> = base
        .iter()
        .zip(other.iter())
        .map(|(a, b)| NameValue {
            name: a.name.merge(base_lang, &b.name, other_lang),
            value: a.value.clone(),
        })
        .collect();
    for a in &base[paired..] {
        let name = match &a.name {
            TranslatableString::Plain(s) => {
                TranslatableString::Translated(HashMap::from([(base_lang.to_string(), s.clone())]))
            }
            TranslatableString::Translated(m) => TranslatableString::Translated(m.clone()),
        };
        options.push(NameValue {
            name,
            value: a.value.clone(),
        });
    }
    for b in &other[paired..] {
        let name = match &b.name {
            TranslatableString::Plain(s) => {
                TranslatableString::Translated(HashMap::from([(other_lang.to_string(), s.clone())]))
            }
            TranslatableString::Translated(m) => TranslatableString::Translated(m.clone()),
        };
        options.push(NameValue {
            name,
            value: b.value.clone(),
        });
    }
    options
}

// ============================================================================
// Table merging
// ============================================================================

/// Merge two `TableNode`s from different languages.
pub(crate) fn merge_table(
    base: &TableNode,
    base_lang: &str,
    other: &TableNode,
    other_lang: &str,
) -> TableNode {
    let header = merge_option(&base.header, &other.header, |h1, h2| {
        let cells = merge_node_lists(&h1.cells, base_lang, &h2.cells, other_lang);
        TableHeader { cells }
    });

    let rows: Vec<TableRow> = {
        if base.rows.len() != other.rows.len() {
            log::warn!(
                "Table row count mismatch when merging {} and {} translations: \
                 {} vs {} rows; unmatched rows will be preserved from the longer side",
                base_lang,
                other_lang,
                base.rows.len(),
                other.rows.len()
            );
        }
        let paired = base.rows.len().min(other.rows.len());
        let mut rows: Vec<TableRow> = base
            .rows
            .iter()
            .zip(other.rows.iter())
            .map(|(r1, r2)| {
                let cells = merge_node_lists(&r1.cells, base_lang, &r2.cells, other_lang);
                TableRow { cells }
            })
            .collect();
        rows.extend(base.rows[paired..].iter().cloned());
        rows.extend(other.rows[paired..].iter().cloned());
        rows
    };

    let caption = merge_option(&base.caption, &other.caption, |a, b| {
        merge_inline_text(a, base_lang, b, other_lang)
    });

    TableNode {
        header,
        rows,
        caption,
    }
}
