use std::collections::HashMap;

use crate::structured::{
    ConditionalNode, FieldType, GroupNode, InlineNode, InlineText, NameValue, StructuredNode,
    TranslatableString,
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
