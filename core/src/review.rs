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

use std::collections::{BTreeMap, BTreeSet};

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
    /// Naming-convention violations: output nodes whose `name` does not match
    /// the `PREFIX_<CamelCase>_<shortUuid>` convention for their component type,
    /// plus any names that collide across the tree. Empty when all names conform.
    pub naming_issues: Vec<String>,
    /// Human-readable observations (field-count mismatch, empty tree, truncation).
    pub notes: Vec<String>,
}

/// Cap on how many missing texts to list, so the report stays readable.
const MAX_MISSING: usize = 200;

/// Cap on how many naming issues to list, so the report stays readable.
const MAX_NAMING_ISSUES: usize = 200;

/// Length of the trailing short-UUID segment in a generated node name
/// (mirrors `SHORT_UUID_LEN` in the converter).
const SHORT_UUID_LEN: usize = 8;

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

    // Naming-convention check: every named node must follow the
    // `PREFIX_<CamelCase>_<shortUuid>` convention (with a prefix valid for its
    // component type), and names must be unique across the tree.
    let mut naming_issues: Vec<String> = Vec::new();
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    check_naming(output, &mut naming_issues, &mut name_counts);
    for (name, count) in &name_counts {
        if *count > 1 {
            naming_issues.push(format!("duplicate node name {name:?} used {count} times"));
        }
    }

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
    if !naming_issues.is_empty() {
        notes.push(format!(
            "{} naming-convention issue(s) found",
            naming_issues.len()
        ));
    }
    if naming_issues.len() > MAX_NAMING_ISSUES {
        naming_issues.truncate(MAX_NAMING_ISSUES);
    }

    ReviewReport {
        coverage,
        input_field_count: input_fields,
        output_field_count: output_fields,
        missing_text: missing,
        naming_issues,
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

// ── Naming conventions ───────────────────────────────────────────────────────
//
// The converter names every node `PREFIX_<CamelCase>_<shortUuid>` (see
// `core/src/aem/converter.rs::make_name`), and the prefixes are those defined in
// `specs/AEM Naming Conventions.md` (the authoritative source). The prefix
// identifies the component type, but the mapping is many-to-many: e.g. a `TBL_`
// name is a (flattened) table Panel, an `IMG_` name is a TextDraw, and
// `TXTM_`/`EML_`/`TEL_` names are TextFields. Each variant below lists every
// prefix it may legitimately carry. `Custom` nodes keep the name of whatever
// element they replaced, so any known prefix is valid. Fragments are exempt —
// they follow the fragment library's own naming conventions.

/// All component-name prefixes the converter emits (spec-conforming).
const ALL_PREFIXES: &[&str] = &[
    "PN", "TXT", "TXTM", "EML", "TEL", "NB", "DATE", "DD", "CB", "RB", "ST", "TTL", "IMG", "TBL",
    "RCP",
];

/// The prefixes a given node variant may legitimately use. An empty slice means
/// the node's name is not checked (`Root` has no name; `Fragment` follows the
/// fragment library's own conventions).
fn allowed_prefixes(node: &AemNode) -> &'static [&'static str] {
    match node {
        AemNode::Root { .. } => &[],
        AemNode::Panel { .. } => &["PN", "TBL"],
        AemNode::TextField { .. } => &["TXT", "TXTM", "EML", "TEL"],
        AemNode::NumberField { .. } => &["NB"],
        AemNode::DatePicker { .. } => &["DATE"],
        AemNode::Dropdown { .. } => &["DD"],
        AemNode::Checkbox { .. } => &["CB"],
        AemNode::RadioButton { .. } => &["RB"],
        AemNode::TextDraw { .. } => &["ST", "IMG"],
        AemNode::TitleDraw { .. } => &["TTL"],
        AemNode::Repeatable { .. } => &["RCP"],
        // Fragments use the fragment library's own naming conventions — exempt.
        AemNode::Fragment { .. } => &[],
        AemNode::Preface { .. } => &["PN"],
        AemNode::Appendix { .. } => &["PN"],
        AemNode::FootnotePlaceholder { .. } => &["ST"],
        AemNode::Custom { .. } => ALL_PREFIXES,
    }
}

/// A node's `name`, or `None` for the nameless `Root`.
fn node_name(node: &AemNode) -> Option<&str> {
    match node {
        AemNode::Root { .. } => None,
        AemNode::Panel { name, .. }
        | AemNode::TextField { name, .. }
        | AemNode::NumberField { name, .. }
        | AemNode::DatePicker { name, .. }
        | AemNode::Dropdown { name, .. }
        | AemNode::Checkbox { name, .. }
        | AemNode::RadioButton { name, .. }
        | AemNode::TextDraw { name, .. }
        | AemNode::TitleDraw { name, .. }
        | AemNode::Repeatable { name, .. }
        | AemNode::Fragment { name, .. }
        | AemNode::Preface { name, .. }
        | AemNode::Appendix { name, .. }
        | AemNode::FootnotePlaceholder { name, .. }
        | AemNode::Custom { name, .. } => Some(name),
    }
}

/// A short human label for a node variant, used in issue messages.
fn node_kind(node: &AemNode) -> &'static str {
    match node {
        AemNode::Root { .. } => "Root",
        AemNode::Panel { .. } => "Panel",
        AemNode::TextField { .. } => "TextField",
        AemNode::NumberField { .. } => "NumberField",
        AemNode::DatePicker { .. } => "DatePicker",
        AemNode::Dropdown { .. } => "Dropdown",
        AemNode::Checkbox { .. } => "Checkbox",
        AemNode::RadioButton { .. } => "RadioButton",
        AemNode::TextDraw { .. } => "TextDraw",
        AemNode::TitleDraw { .. } => "TitleDraw",
        AemNode::Repeatable { .. } => "Repeatable",
        AemNode::Fragment { .. } => "Fragment",
        AemNode::Preface { .. } => "Preface",
        AemNode::Appendix { .. } => "Appendix",
        AemNode::FootnotePlaceholder { .. } => "FootnotePlaceholder",
        AemNode::Custom { .. } => "Custom",
    }
}

/// A node's children, or an empty slice for leaves.
fn node_children(node: &AemNode) -> &[AemNode] {
    match node {
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => children,
        _ => &[],
    }
}

/// Split a trailing `_<8 lowercase-hex>` short-UUID suffix off a name, returning
/// the head (prefix + optional CamelCase) and the suffix.
fn split_short_uuid(name: &str) -> Option<(&str, &str)> {
    let (head, tail) = name.rsplit_once('_')?;
    let is_short_uuid = tail.len() == SHORT_UUID_LEN
        && tail
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if is_short_uuid && !head.is_empty() {
        Some((head, tail))
    } else {
        None
    }
}

/// Whether `name` conforms to `PREFIX_<CamelCase>_<shortUuid>` (or the degraded
/// `PREFIX_<shortUuid>`) for one of the `allowed` prefixes.
fn name_conforms(name: &str, allowed: &[&str]) -> bool {
    let Some((head, _uuid)) = split_short_uuid(name) else {
        return false;
    };
    allowed.iter().any(|p| {
        // `PREFIX_<uuid>` — head is exactly the prefix (empty CamelCase part).
        head == *p
            // `PREFIX_<CamelCase>_<uuid>` — head is prefix + `_` + alphanumeric.
            || head
                .strip_prefix(p)
                .and_then(|rest| rest.strip_prefix('_'))
                .is_some_and(|camel| !camel.is_empty() && camel.chars().all(|c| c.is_ascii_alphanumeric()))
    })
}

/// Walk the output tree, recording naming-convention violations and tallying
/// each name for the tree-wide uniqueness check.
fn check_naming(node: &AemNode, issues: &mut Vec<String>, counts: &mut BTreeMap<String, usize>) {
    if let Some(name) = node_name(node) {
        *counts.entry(name.to_string()).or_insert(0) += 1;
        let allowed = allowed_prefixes(node);
        if !allowed.is_empty() && !name_conforms(name, allowed) {
            issues.push(format!(
                "{} node name {name:?} does not match PREFIX_<CamelCase>_<shortUuid> \
                 (expected prefix one of: {})",
                node_kind(node),
                allowed.join(", "),
            ));
        }
    }
    for child in node_children(node) {
        check_naming(child, issues, counts);
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

    #[test]
    fn converter_output_has_no_naming_issues() {
        let input = sample_input();
        let aem = crate::convert_to_aem(&input, &AemConfig::test_default("TEST"));
        let report = review_output(&input, &aem, "en");
        assert!(
            report.naming_issues.is_empty(),
            "converter output should be convention-clean, got {:?}",
            report.naming_issues
        );
    }

    #[test]
    fn conforming_names_are_accepted() {
        assert!(name_conforms("TXT_FirstName_ab12cd34", &["TXT", "TXTM", "EML", "TEL"]));
        assert!(name_conforms("TXTM_Comments_ab12cd34", &["TXT", "TXTM", "EML", "TEL"]));
        assert!(name_conforms("DATE_ab12cd34", &["DATE"])); // degraded: empty CamelCase
        assert!(name_conforms("RCP_deadbeef", &["RCP"]));
        assert!(name_conforms("TBL_Summary_deadbeef", &["PN", "TBL"]));
        // A prefix containing an underscore is matched whole, not split.
        assert!(name_conforms("PN_affrg_Address1_00ff11aa", &["PN_affrg"]));
    }

    #[test]
    fn nonconforming_names_are_rejected() {
        assert!(!name_conforms("firstName", &["TXT"])); // no prefix, no uuid
        assert!(!name_conforms("TXT_FirstName", &["TXT"])); // missing short-uuid
        assert!(!name_conforms("XYZ_FirstName_ab12cd34", &["TXT"])); // wrong prefix
        assert!(!name_conforms("TXT_First Name_ab12cd34", &["TXT"])); // space in CamelCase
        assert!(!name_conforms("DATE_ab12cd3", &["DATE"])); // 7-char suffix, not 8
        assert!(!name_conforms("NB_Age_ABCDEF12", &["NB"])); // uppercase hex
    }

    #[test]
    fn bad_name_is_reported() {
        let aem = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextField {
                uuid: uuid::Uuid::nil(),
                name: "not_a_valid_name".into(),
                label: "First name".into(),
                mandatory: false,
                visible: true,
                max_chars: None,
                colspan: 12,
                dor_colspan: None,
                bind_ref: None,
            }],
        };
        let report = review_output(&[], &aem, "en");
        assert!(
            report.naming_issues.iter().any(|i| i.contains("not_a_valid_name")),
            "expected the malformed name to be flagged, got {:?}",
            report.naming_issues
        );
        assert!(report.notes.iter().any(|n| n.contains("naming-convention")));
    }

    #[test]
    fn duplicate_names_are_reported() {
        let field = |name: &str| AemNode::NumberField {
            uuid: uuid::Uuid::nil(),
            name: name.into(),
            label: "Age".into(),
            mandatory: false,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
        };
        let aem = AemNode::Root {
            title: "Form".into(),
            children: vec![field("NB_Age_ab12cd34"), field("NB_Age_ab12cd34")],
        };
        let report = review_output(&[], &aem, "en");
        assert!(
            report
                .naming_issues
                .iter()
                .any(|i| i.contains("duplicate") && i.contains("NB_Age_ab12cd34")),
            "expected duplicate-name issue, got {:?}",
            report.naming_issues
        );
    }
}
