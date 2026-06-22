//! Post-conversion fidelity review.
//!
//! Compares the engine's parse of the **input** (the merged structured tree)
//! against the **output** (the converted [`AemNode`] tree) and reports text or
//! elements that went missing along the way. This is a deterministic checklist
//! the conversion agent runs before finishing, complementing the structural
//! `validate_aem_*` checks (which verify the AEM contract, not input fidelity).
//!
//! Text is compared in a single language — the form's master language — because
//! AEM node labels are flattened to one language at conversion time
//! (translations ride along separately in the dictionary). Comparing all
//! languages would flag every non-master string as missing.

use std::collections::BTreeSet;

use crate::aem::{AemNode, AemOption};
use crate::structured::{FieldType, StructuredNode, TableNode};

/// The result of reviewing a converted output against its input.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReviewReport {
    /// Fraction of distinct input texts that appear verbatim in the output
    /// (`1.0` when the input has no text).
    pub coverage: f32,
    /// Number of input fields (`StructuredNode::Field`).
    pub input_field_count: usize,
    /// Number of output field-like leaves (text boxes, pickers, choice groups…).
    pub output_field_count: usize,
    /// Distinct input texts (labels, options, headings, paragraphs, …) with no
    /// verbatim match anywhere in the output.
    pub missing_text: Vec<String>,
    /// Human-readable observations (field-count mismatch, empty tree, truncation).
    pub notes: Vec<String>,
}

/// Cap on how many missing texts to list, so the report stays readable.
const MAX_MISSING: usize = 200;

/// Review the converted AEM `output` against the engine's parse of the `input`
/// (the merged structured tree), comparing text in `master_language`.
pub fn review_output(
    input: &[StructuredNode],
    output: &AemNode,
    master_language: &str,
) -> ReviewReport {
    let mut input_texts: Vec<String> = Vec::new();
    let mut input_fields = 0usize;
    for node in input {
        collect_input(node, master_language, &mut input_texts, &mut input_fields);
    }

    let mut output_texts: Vec<String> = Vec::new();
    let mut output_fields = 0usize;
    collect_output(output, &mut output_texts, &mut output_fields);

    // Normalize both sides and build the output lookup set.
    let output_set: BTreeSet<String> = output_texts
        .iter()
        .map(|t| normalize(t))
        .filter(|t| !t.is_empty())
        .collect();

    // Distinct, normalized, non-empty input texts (preserving first-seen order).
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut distinct_input: Vec<String> = Vec::new();
    for t in &input_texts {
        let n = normalize(t);
        if !n.is_empty() && seen.insert(n.clone()) {
            distinct_input.push(n);
        }
    }

    let total = distinct_input.len();
    let mut missing: Vec<String> = distinct_input
        .into_iter()
        .filter(|t| !output_set.contains(t))
        .collect();

    let matched = total - missing.len();
    let coverage = if total == 0 {
        1.0
    } else {
        matched as f32 / total as f32
    };

    let mut notes = Vec::new();
    if input.is_empty() {
        notes.push("input (merged structured tree) is empty — nothing to compare".into());
    }
    if input_fields != output_fields {
        notes.push(format!(
            "field count differs: input has {input_fields}, output has {output_fields}"
        ));
    }
    if missing.len() > MAX_MISSING {
        notes.push(format!(
            "missing_text truncated to {MAX_MISSING} of {} entries",
            missing.len()
        ));
        missing.truncate(MAX_MISSING);
    }

    ReviewReport {
        coverage,
        input_field_count: input_fields,
        output_field_count: output_fields,
        missing_text: missing,
        notes,
    }
}

/// Normalize text for verbatim matching: strip simple `<...>` markup (AEM option
/// labels may carry rich-text HTML), then collapse whitespace runs and trim.
fn normalize(s: &str) -> String {
    let mut stripped = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(c),
            _ => {}
        }
    }
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Input side (structured tree) ─────────────────────────────────────────────

fn collect_input(node: &StructuredNode, lang: &str, out: &mut Vec<String>, fields: &mut usize) {
    match node {
        StructuredNode::Heading(h) => out.push(h.content.plain_text_in(lang)),
        StructuredNode::Paragraph(p) => out.push(p.content.plain_text_in(lang)),
        StructuredNode::Footnote(f) => out.push(f.content.plain_text_in(lang)),
        StructuredNode::Field(f) => {
            *fields += 1;
            if let Some(label) = &f.label {
                out.push(label.plain_text_in(lang));
            }
            if let Some(ph) = &f.placeholder {
                out.push(ph.get_or_default(lang).to_string());
            }
            match &f.input_type {
                FieldType::Radio { options }
                | FieldType::Select { options }
                | FieldType::CheckboxGroup { options } => {
                    for opt in options {
                        out.push(opt.name.get_or_default(lang).to_string());
                    }
                }
                _ => {}
            }
        }
        StructuredNode::Group(g) => {
            for child in &g.children {
                collect_input(child, lang, out, fields);
            }
        }
        StructuredNode::Repeatable(r) => collect_input(&r.item, lang, out, fields),
        StructuredNode::Conditional(c) => collect_input(&c.content, lang, out, fields),
        StructuredNode::GridLayout(grid) => {
            for el in &grid.elements {
                collect_input(&el.node, lang, out, fields);
            }
        }
        StructuredNode::List(list) => {
            for item in &list.items {
                out.push(item.plain_text_in(lang));
                if let Some(sub) = &item.sublist {
                    for sub_item in &sub.items {
                        out.push(sub_item.plain_text_in(lang));
                    }
                }
            }
        }
        StructuredNode::Table(t) => collect_table(t, lang, out, fields),
        // Images carry no text that maps to an AEM node; Empty is a placeholder.
        StructuredNode::Image(_) | StructuredNode::Empty => {}
    }
}

fn collect_table(t: &TableNode, lang: &str, out: &mut Vec<String>, fields: &mut usize) {
    if let Some(caption) = &t.caption {
        out.push(caption.plain_text_in(lang));
    }
    if let Some(header) = &t.header {
        for cell in &header.cells {
            collect_input(cell, lang, out, fields);
        }
    }
    for row in &t.rows {
        for cell in &row.cells {
            collect_input(cell, lang, out, fields);
        }
    }
}

// ── Output side (AEM tree) ───────────────────────────────────────────────────

fn collect_output(node: &AemNode, out: &mut Vec<String>, fields: &mut usize) {
    let push_options = |out: &mut Vec<String>, options: &[AemOption]| {
        for opt in options {
            out.push(opt.label.clone());
        }
    };
    match node {
        AemNode::Root { title, children } => {
            out.push(title.clone());
            for c in children {
                collect_output(c, out, fields);
            }
        }
        AemNode::Panel { title, children, .. } => {
            out.push(title.clone());
            for c in children {
                collect_output(c, out, fields);
            }
        }
        AemNode::Repeatable { title, children, .. } => {
            out.push(title.clone());
            for c in children {
                collect_output(c, out, fields);
            }
        }
        AemNode::TextField { label, .. }
        | AemNode::NumberField { label, .. }
        | AemNode::DatePicker { label, .. } => {
            *fields += 1;
            out.push(label.clone());
        }
        AemNode::Dropdown { label, options, .. }
        | AemNode::Checkbox { label, options, .. }
        | AemNode::RadioButton { label, options, .. }
        | AemNode::Custom { label, options, .. } => {
            *fields += 1;
            out.push(label.clone());
            push_options(out, options);
        }
        AemNode::TextDraw { content, .. } | AemNode::TitleDraw { content, .. } => {
            out.push(content.clone());
        }
        // Runtime-resolved or text-free placeholders.
        AemNode::Fragment { .. }
        | AemNode::Preface { .. }
        | AemNode::Appendix { .. }
        | AemNode::FootnotePlaceholder { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aem::AemConfig;
    use crate::structured::{FieldNode, FieldType, HeadingLevel, HeadingNode, TranslatedText};

    fn text_field(name: &str, label: &str) -> StructuredNode {
        StructuredNode::Field(FieldNode {
            name: name.into(),
            som_path: None,
            label: Some(TranslatedText::plain_with_lang("en", label)),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
            required: false,
        })
    }

    fn sample_input() -> Vec<StructuredNode> {
        vec![
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H1,
                content: TranslatedText::plain_with_lang("en", "Personal data"),
                som_path: None,
                source_name: None,
            }),
            text_field("first_name", "First name"),
            text_field("last_name", "Last name"),
        ]
    }

    #[test]
    fn roundtrip_has_no_misses() {
        let input = sample_input();
        let aem = crate::convert_to_aem(&input, &AemConfig::test_default("TEST"));
        let report = review_output(&input, &aem, "en");
        assert!(
            report.missing_text.is_empty(),
            "expected no misses, got {:?}",
            report.missing_text
        );
        assert_eq!(report.input_field_count, 2);
        assert_eq!(report.output_field_count, 2);
        assert!((report.coverage - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn dropped_field_is_reported() {
        let input = sample_input();
        // Output is missing "Last name".
        let reduced: Vec<StructuredNode> = input.iter().take(2).cloned().collect();
        let aem = crate::convert_to_aem(&reduced, &AemConfig::test_default("TEST"));
        let report = review_output(&input, &aem, "en");
        assert!(
            report.missing_text.iter().any(|t| t == "Last name"),
            "expected 'Last name' missing, got {:?}",
            report.missing_text
        );
        assert!(report.coverage < 1.0);
    }
}
