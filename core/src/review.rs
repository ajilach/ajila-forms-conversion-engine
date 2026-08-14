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

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::aem::{AemConfig, AemNode, AemOption, generate_aem_xml};
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
    /// Naming-convention violations found in the rendered JCR XML. Mirrors the
    /// `find_naming_violations` detector: each author-named component's leading
    /// `PREFIX_` token must be valid for its `sling:resourceType`. Empty when
    /// every named node conforms.
    pub naming_violations: Vec<NamingViolation>,
    /// Input components whose `jcr:title` is missing or is not a plain label
    /// (a parenthetical hint, leaked rich-text markup, a whole paragraph …).
    /// Every input must carry a title; positional label attachment can leave
    /// one empty or bind the wrong nearby text.
    pub label_issues: Vec<LabelIssue>,
    /// Human-readable observations (field-count mismatch, empty tree, truncation).
    pub notes: Vec<String>,
}

/// A single naming-convention violation (one non-conforming named node).
#[derive(Debug, Clone, serde::Serialize)]
pub struct NamingViolation {
    /// JCR node tag (AEM-internal id, e.g. `panel_77897ce3…`).
    pub node: String,
    /// The offending `name` attribute.
    pub name: String,
    /// `sling:resourceType` leaf (e.g. `textbox`, `panel`, `titledraw`).
    pub rt: String,
    /// Detected role (`panel`, `repeat-panel`, `repeat-subpanel`, `button`, or the rt).
    pub role: String,
    /// The canonical prefix expected for this role/type.
    pub expected: String,
    /// `wrong-prefix` (a known prefix, wrong for the type) or `raw` (not a known prefix).
    pub bucket: String,
    /// `high` for type / plain-panel / repeat-panel checks; `low` for repeat sub-panels.
    pub confidence: String,
}

/// A single input component whose label is missing or malformed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LabelIssue {
    /// JCR node tag (AEM-internal id, e.g. `radiobutton_77897ce3…`).
    pub node: String,
    /// The component's `name` attribute.
    pub name: String,
    /// `sling:resourceType` leaf (e.g. `textbox`, `radiobutton`).
    pub rt: String,
    /// The offending `jcr:title`, unescaped and truncated for readability
    /// (empty for `missing`).
    pub title: String,
    /// `missing` | `parenthetical` | `markup` | `quoted`.
    pub kind: String,
    /// `high` for a missing title or a structurally wrong one (parenthetical /
    /// markup); `low` for `quoted`, which also occurs on genuine labels in the
    /// reference forms (`Classification as "Eligible Counterparty" …`).
    pub confidence: String,
}

/// Cap on how many missing texts to list, so the report stays readable.
const MAX_MISSING: usize = 200;

/// Cap on how many label issues to list, so the report stays readable.
const MAX_LABEL_ISSUES: usize = 400;

/// Cap on how many naming violations to list, so the report stays readable.
const MAX_NAMING: usize = 400;

/// Review the converted AEM `output` against the engine's parse of the `input`
/// (the merged structured tree), comparing text in `master_language`.
pub fn review_output(
    input: &[StructuredNode],
    output: &AemNode,
    config: &AemConfig,
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

    // Naming-convention check: render the tree to JCR XML and classify every
    // named node by its `sling:resourceType`, exactly like the
    // `find_naming_violations` detector (leading `PREFIX_` only). Rendering (not
    // walking the `AemNode` tree) is what lets this see template-expanded nodes
    // and custom-element internals, just as the detector sees the shipped ZIP.
    let aem_xml = generate_aem_xml(output, config);
    let (mut naming_violations, naming_counts, mut label_issues) =
        check_naming_conventions(&aem_xml);

    let (coverage, mut missing) = coverage_against(&input_texts, &output_texts);

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
    let [n_ok, n_wrong, n_raw] = naming_counts;
    if !naming_violations.is_empty() {
        notes.push(format!("naming: {n_wrong} wrong-prefix, {n_raw} raw ({n_ok} ok)"));
    }
    if naming_violations.len() > MAX_NAMING {
        notes.push(format!(
            "naming_violations truncated to {MAX_NAMING} of {} entries",
            naming_violations.len()
        ));
        naming_violations.truncate(MAX_NAMING);
    }
    if !label_issues.is_empty() {
        let missing_titles = label_issues.iter().filter(|l| l.kind == "missing").count();
        notes.push(format!(
            "labels: {missing_titles} inputs without a title, {} with a malformed one",
            label_issues.len() - missing_titles
        ));
    }
    if label_issues.len() > MAX_LABEL_ISSUES {
        notes.push(format!(
            "label_issues truncated to {MAX_LABEL_ISSUES} of {} entries",
            label_issues.len()
        ));
        label_issues.truncate(MAX_LABEL_ISSUES);
    }

    ReviewReport {
        coverage,
        input_field_count: input_fields,
        output_field_count: output_fields,
        missing_text: missing,
        naming_violations,
        label_issues,
        notes,
    }
}

/// Review a generated [`RedactoDump`] against the engine's parse of the `input`,
/// comparing text in `master_language`.
///
/// Reviews **the dump**, not the structured tree it was generated from: the
/// tree is an intermediate, and text can still be dropped on the way into the
/// dump (input fields, images, and the other node kinds the Redacto converter
/// warns about and skips). Reviewing the artefact that actually ships is the
/// point — deriving the review from a different source than the output is
/// precisely how an empty Redacto dump once passed a clean fidelity check.
///
/// [`ReviewReport::naming_violations`] is always empty and
/// [`ReviewReport::output_field_count`] always zero: Redacto documents carry no
/// named components and no input fields.
pub fn review_redacto(
    input: &[StructuredNode],
    dump: &crate::redacto::RedactoDump,
    master_language: &str,
) -> ReviewReport {
    let mut input_texts: Vec<String> = Vec::new();
    let mut input_fields = 0usize;
    for node in input {
        collect_input(node, master_language, &mut input_texts, &mut input_fields);
    }

    // Compare against the master-language asset bodies. Fall back to every
    // language when the master has no variants, so a monolingual dump in another
    // language still reviews.
    let master_versions: Vec<&str> = dump
        .asset_versions
        .iter()
        .filter(|v| v.language == master_language)
        .map(|v| v.content.as_str())
        .collect();
    let bodies: Vec<&str> = if master_versions.is_empty() {
        dump.asset_versions
            .iter()
            .map(|v| v.content.as_str())
            .collect()
    } else {
        master_versions
    };

    // Each asset body is one HTML fragment that may hold several source texts —
    // a list asset carries every `<li>` — so offer both the whole fragment and
    // each tag-delimited run. Without the runs, list items and table cells look
    // missing merely because stripping the tags concatenates them.
    let mut output_texts: Vec<String> = Vec::new();
    for body in bodies {
        output_texts.push(body.to_string());
        output_texts.extend(html_text_runs(body));
    }

    let (coverage, mut missing) = coverage_against(&input_texts, &output_texts);

    let mut notes = Vec::new();
    if input.is_empty() {
        notes.push("input (structured tree) is empty — nothing to compare".into());
    }
    if dump.assets.is_empty() {
        notes.push("the dump contains no text assets — it describes an empty document".into());
    }
    if input_fields > 0 {
        notes.push(format!(
            "{input_fields} input field(s) were skipped: the Redacto target supports \
             text-only documents"
        ));
    }
    for warning in &dump.warnings {
        notes.push(warning.clone());
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
        output_field_count: 0,
        missing_text: missing,
        naming_violations: Vec::new(),
        // Redacto is a text-only target: it has no AEM inputs to label.
        label_issues: Vec::new(),
        notes,
    }
}

/// The text runs of an HTML fragment: the character data between tags, one
/// entry per run, empty runs dropped.
///
/// Deliberately not a parser — the fragments here are the ones the Redacto
/// content renderer emits, and all this needs to do is keep separate source
/// texts separate.
fn html_text_runs(html: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                if !current.trim().is_empty() {
                    runs.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
            '>' => in_tag = false,
            _ if !in_tag => current.push(c),
            _ => {}
        }
    }
    if !current.trim().is_empty() {
        runs.push(current);
    }
    runs
}

/// Fraction of distinct input texts that appear verbatim in the output, plus the
/// ones that do not (both sides normalized, first-seen order preserved).
///
/// Shared by [`review_output`] and [`review_redacto`]: both answer the same
/// question — which input text did not survive into the shipped artefact — and
/// differ only in how they harvest the output side.
fn coverage_against(input_texts: &[String], output_texts: &[String]) -> (f32, Vec<String>) {
    let output_set: BTreeSet<String> = output_texts
        .iter()
        .map(|t| normalize(t))
        .filter(|t| !t.is_empty())
        .collect();

    // Distinct, normalized, non-empty input texts (preserving first-seen order).
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut distinct_input: Vec<String> = Vec::new();
    for t in input_texts {
        let n = normalize(t);
        if !n.is_empty() && seen.insert(n.clone()) {
            distinct_input.push(n);
        }
    }

    let total = distinct_input.len();
    let missing: Vec<String> = distinct_input
        .into_iter()
        .filter(|t| !output_set.contains(t))
        .collect();

    let coverage = if total == 0 {
        1.0
    } else {
        (total - missing.len()) as f32 / total as f32
    };
    (coverage, missing)
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

// ── Naming conventions (ports `find_naming_violations.py`) ───────────────────
//
// The UBS AEM naming convention: every author-named component's `name` attribute
// begins with `PREFIX_`, where PREFIX is fixed per component type (see
// `specs/AEM Naming Conventions.md`). This is the exact classification the
// `ajila-forms-conversion-feedback` detector applies to the shipped ZIP —
// reproduced here so the conversion agent gets the same verdicts up front.
//
// Scoping (kept identical to the detector; do not silently change):
//   - Only the leading `PREFIX_` token is enforced; the `<shortUuid>` is
//     optional and the `<CamelCaseName>` is never inspected or invented.
//   - System / fixed-name resourceTypes are exempt; fragment-referenced nodes
//     (`fragRef`, `affrg` in the name, an `AF_` prefix) are exempt; the `preview`
//     construct and `*Title` step-title wrappers are exempt.
//   - Three buckets: `ok`, `wrong-prefix` (a known prefix, wrong for the type),
//     `raw` (not a known prefix at all).

/// Canonical prefix(es) for a resourceType leaf (first is canonical). `None`
/// means "not a mapped field type".
fn type_prefixes(rt: &str) -> Option<&'static [&'static str]> {
    Some(match rt {
        "textbox" | "guidetextbox" => &["TXT"],
        "checkbox" => &["CB"],
        "radiobutton" => &["RB"],
        "dropdownlist" => &["DD"],
        "datepicker" | "guidedatepicker" => &["DATE"],
        "numericbox" => &["NB"],
        "telephone" => &["TEL"],
        "email" => &["EML"],
        "textboxMultiline" => &["TXTM"],
        "titledraw" => &["TTL"],
        "signature" => &["SIGN"],
        "textdraw" => &["ST", "ITXT", "ETXT"],
        "image" => &["IMG"],
        "chart" => &["CRT"],
        "separator" => &["SPT"],
        "barcode" => &["BARCODE"],
        "qrcode" => &["QRCODE"],
        "table" => &["TBL"],
        _ => return None,
    })
}

/// The authoritative closed set of allowed prefixes — used to tell a
/// `wrong-prefix` (a known prefix used on the wrong type) from a `raw` name.
const KNOWN_PREFIXES: &[&str] = &[
    "RB", "DATE", "TXT", "CB", "DD", "PN", "RCHT", "RCP", "RCBP", "RCHP", "NB", "BT", "ST", "TEL",
    "TTL", "TXTM", "EML", "IMG", "CRT", "SPT", "ITXT", "ETXT", "TBL", "BARCODE", "QRCODE", "SIGN",
];

/// System / fixed-name resourceType leaves — never author-named, skip entirely.
const EXEMPT_RT: &[&str] = &[
    "guideContainer", "rootPanel", "toolbar", "nextitemnav", "previtemnav", "submit", "summary",
    "dorOptionsUBS", "metadata", "letterhead", "carousel", "messagebox",
    "messagebox-CarouselPreviewError", "errorboxcarouselpreview", "previewbutton", "signaturebox",
    "guidefootnoteplaceholder", "guideheader", "guidefooter", "formtitle", "aftemplatedpage",
    "responsivegrid", "guidefieldset", "defaultGuideLayout", "wizard", "gridFluidLayout2",
    "defaultToolbarLayout", "dorProperties",
];

/// Button resourceType leaves (also anything ending in `button`).
const BUTTON_RT: &[&str] = &["removebutton", "tertiarybutton", "secondarybutton", "primarybutton"];

/// System panels with fixed engine names — exempt.
const EXEMPT_PANEL_NAMES: &[&str] = &[
    "summaryPanel",
    "previewPanel",
    "PN_Preview",
    "guideRootPanel",
    // The global FormMetadata fragment step carries the fragment's own fixed
    // name, so the `PN_` convention does not apply to it.
    "FormMetadata",
];

/// Input (data-entry) resourceType leaves. Every one of these must present a
/// label to the user, so a missing `jcr:title` is a defect — unlike a draw or
/// a panel, which legitimately carries none.
const INPUT_RT: &[&str] = &[
    "textbox", "guidetextbox", "textboxMultiline", "numericbox", "datepicker", "guidedatepicker",
    "dropdownlist", "radiobutton", "checkbox", "telephone", "email",
];

/// Classify one input's `jcr:title`. `None` = the title is a plain, usable
/// label.
///
/// Both failure modes this catches come from positional label attachment
/// (`document::modules::label_attacher`): it binds the nearest free text block
/// to a field, so a field can end up with no label at all once its neighbours
/// are taken, or with a fragment that merely sits close by — a parenthetical
/// hint, a rich-text paragraph — instead of its actual question.
fn classify_title(
    rt_leaf: &str,
    title: &str,
    option_labels_present: bool,
) -> Option<(&'static str, &'static str)> {
    if !INPUT_RT.contains(&rt_leaf) {
        return None;
    }
    let t = title.trim();
    if t.is_empty() {
        // A single-option checkbox carries its label in the option by design
        // (`converter.rs`, `FieldType::Bool`), so it renders a label even with
        // an empty title.
        if rt_leaf == "checkbox" && option_labels_present {
            return None;
        }
        return Some(("missing", "high"));
    }
    if t.starts_with('(') && t.ends_with(')') {
        return Some(("parenthetical", "high"));
    }
    if t.contains('<') && t.contains('>') {
        return Some(("markup", "high"));
    }
    if t.contains('"') {
        return Some(("quoted", "low"));
    }
    None
}

/// A classified verdict for one node.
struct Verdict {
    bucket: &'static str, // "ok" | "wrong-prefix" | "raw"
    expected: String,
    role: String,
    confidence: &'static str,
}

/// Leading prefix token — everything before the first `_` (or the whole name).
fn name_prefix(nm: &str) -> &str {
    nm.split_once('_').map(|(a, _)| a).unwrap_or(nm)
}

/// Classify one element. `None` = skip (exempt / unmapped / no name). Mirrors
/// `classify()` in the detector.
fn classify(
    tag: &str,
    rt_leaf: &str,
    name: &str,
    has_frag_ref: bool,
    self_repeatable: bool,
    ancestor_in_repeat: bool,
) -> Option<Verdict> {
    if name.is_empty() || rt_leaf.is_empty() {
        return None;
    }
    if EXEMPT_RT.contains(&rt_leaf) || rt_leaf.to_ascii_lowercase().contains("layout") {
        return None;
    }
    // fragment-referenced / engine-injected — exempt.
    if has_frag_ref || name.contains("affrg") || name.starts_with("AF_") {
        return None;
    }
    // the UBS "Preview" construct is referenced by name on nearly every form.
    if name.eq_ignore_ascii_case("preview") {
        return None;
    }

    let (role, valid, confidence): (String, Vec<&'static str>, &'static str) = if rt_leaf == "panel"
    {
        if EXEMPT_PANEL_NAMES.contains(&name)
            || tag.starts_with("summarypanel")
            || tag.starts_with("previewpanel")
        {
            return None;
        }
        // title-wrapper panels track their parent's name — exempt (parent drives the fix).
        if tag.starts_with("panel_title") || name.ends_with("Title") {
            return None;
        }
        if self_repeatable {
            // a repeatable panel → repeat-container panel; accept the RC* family.
            ("repeat-panel".into(), vec!["RCP", "RCBP", "RCHP", "RCHT"], "high")
        } else if ancestor_in_repeat {
            // a sub-panel within a repeat container — low confidence, human review.
            ("repeat-subpanel".into(), vec!["RCP", "RCHP", "RCBP", "RCHT", "PN"], "low")
        } else {
            ("panel".into(), vec!["PN"], "high")
        }
    } else if let Some(tp) = type_prefixes(rt_leaf) {
        // must precede the button catch — radiobutton ends in "button".
        (rt_leaf.to_string(), tp.to_vec(), "high")
    } else if BUTTON_RT.contains(&rt_leaf) || rt_leaf.ends_with("button") {
        ("button".into(), vec!["BT"], "high")
    } else {
        return None; // unknown / unmapped type — don't guess.
    };

    let expected = valid[0].to_string();
    let pfx = name_prefix(name);
    let bucket = if valid.contains(&pfx) {
        "ok"
    } else if KNOWN_PREFIXES.contains(&pfx) {
        "wrong-prefix"
    } else {
        "raw"
    };
    Some(Verdict { bucket, expected, role, confidence })
}

/// Read the attributes we care about + whether this element is itself a repeat
/// container, classify it, and record a violation if non-`ok`. Returns whether
/// the element is repeatable (so descendants count as "in a repeat container").
fn scan_element(
    e: &BytesStart,
    ancestor_in_repeat: bool,
    counts: &mut [usize; 3],
    out: &mut Vec<NamingViolation>,
    labels: &mut Vec<LabelIssue>,
) -> bool {
    let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut name = String::new();
    let mut rt_full = String::new();
    let mut title = String::new();
    let mut options = String::new();
    let mut has_frag_ref = false;
    let mut has_occur = false;
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"name" => name = attr.unescape_value().unwrap_or_default().into_owned(),
            b"sling:resourceType" => {
                rt_full = attr.unescape_value().unwrap_or_default().into_owned()
            }
            b"jcr:title" => title = attr.unescape_value().unwrap_or_default().into_owned(),
            b"options" => options = attr.unescape_value().unwrap_or_default().into_owned(),
            b"fragRef" => has_frag_ref = true,
            b"minOccur" | b"maxOccur" => has_occur = true,
            _ => {}
        }
    }
    let rt_leaf = rt_full.rsplit('/').next().unwrap_or("");
    let self_repeatable = has_occur || tag.starts_with("repeatable");

    // `options` is `[value=Label,…]`; a label is present when any entry carries
    // text after its `=`.
    let option_labels_present = options
        .trim_matches(['[', ']'])
        .split(',')
        .any(|o| o.split_once('=').is_some_and(|(_, l)| !l.trim().is_empty()));
    if let Some((kind, confidence)) = classify_title(rt_leaf, &title, option_labels_present) {
        labels.push(LabelIssue {
            node: tag.clone(),
            name: name.clone(),
            rt: rt_leaf.to_string(),
            title: title.chars().take(160).collect(),
            kind: kind.to_string(),
            confidence: confidence.to_string(),
        });
    }

    if let Some(v) = classify(&tag, rt_leaf, &name, has_frag_ref, self_repeatable, ancestor_in_repeat)
    {
        match v.bucket {
            "ok" => counts[0] += 1,
            "wrong-prefix" => counts[1] += 1,
            "raw" => counts[2] += 1,
            _ => {}
        }
        if v.bucket != "ok" {
            out.push(NamingViolation {
                node: tag,
                name,
                rt: rt_leaf.to_string(),
                role: v.role,
                expected: v.expected,
                bucket: v.bucket.to_string(),
                confidence: v.confidence.to_string(),
            });
        }
    }
    self_repeatable
}

/// Parse rendered JCR XML once and return the naming violations with their
/// `[ok, wrong-prefix, raw]` counts, plus every input whose label is missing or
/// malformed.
fn check_naming_conventions(
    xml: &str,
) -> (Vec<NamingViolation>, [usize; 3], Vec<LabelIssue>) {
    let mut reader = Reader::from_str(xml);
    let mut violations = Vec::new();
    let mut labels = Vec::new();
    let mut counts = [0usize; 3];
    // Stack of "is this open element a repeat container" flags; the running
    // count of `true`s is the number of repeat ancestors.
    let mut stack: Vec<bool> = Vec::new();
    let mut repeat_ancestors: usize = 0;
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let rep = scan_element(
                    e,
                    repeat_ancestors > 0,
                    &mut counts,
                    &mut violations,
                    &mut labels,
                );
                stack.push(rep);
                if rep {
                    repeat_ancestors += 1;
                }
            }
            Ok(Event::Empty(ref e)) => {
                scan_element(
                    e,
                    repeat_ancestors > 0,
                    &mut counts,
                    &mut violations,
                    &mut labels,
                );
            }
            Ok(Event::End(_)) => {
                if let Some(rep) = stack.pop() {
                    if rep {
                        repeat_ancestors = repeat_ancestors.saturating_sub(1);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break, // our own render is well-formed; bail on the unexpected.
            _ => {}
        }
    }
    (violations, counts, labels)
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
        let cfg = AemConfig::test_default("TEST");
        let aem = crate::convert_to_aem(&input, &cfg);
        let report = review_output(&input, &aem, &cfg, "en");
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
        let cfg = AemConfig::test_default("TEST");
        let aem = crate::convert_to_aem(&reduced, &cfg);
        let report = review_output(&input, &aem, &cfg, "en");
        assert!(
            report.missing_text.iter().any(|t| t == "Last name"),
            "expected 'Last name' missing, got {:?}",
            report.missing_text
        );
        assert!(report.coverage < 1.0);
    }

    // ── Naming-convention checks (ported detector) ────────────────────────────

    #[test]
    fn name_prefix_is_leading_token() {
        assert_eq!(name_prefix("TXT_FirstName_ab12cd34"), "TXT");
        assert_eq!(name_prefix("PN_affrg_Address1"), "PN"); // splits at FIRST underscore
        assert_eq!(name_prefix("bareName"), "bareName");
    }

    /// Only the leading `PREFIX_` matters — a missing shortUuid or an arbitrary
    /// CamelCase part is still `ok`.
    #[test]
    fn leading_prefix_only_is_ok() {
        let v = classify("textbox_x", "textbox", "TXT_FirstName_ab12cd34", false, false, false).unwrap();
        assert_eq!(v.bucket, "ok");
        let v = classify("textbox_x", "textbox", "TXT_FirstName", false, false, false).unwrap();
        assert_eq!(v.bucket, "ok"); // no shortUuid — still fine
        let v = classify("datepicker_x", "datepicker", "DATE", false, false, false).unwrap();
        assert_eq!(v.bucket, "ok"); // prefix only
    }

    #[test]
    fn wrong_prefix_vs_raw_buckets() {
        // NUM_ is not a known prefix at all → raw.
        let v = classify("numericbox_x", "numericbox", "NUM_Age_ab12", false, false, false).unwrap();
        assert_eq!((v.bucket, v.expected.as_str()), ("raw", "NB"));
        // PN_ is a known prefix but wrong for a repeat panel (expects RCP) → wrong-prefix.
        let v = classify("panel_x", "panel", "PN_Rows", false, true, false).unwrap();
        assert_eq!((v.bucket, v.expected.as_str(), v.role.as_str()), ("wrong-prefix", "RCP", "repeat-panel"));
        // legacy RP_ / lowercase tag word → raw.
        assert_eq!(classify("panel_x", "panel", "RP_x", false, false, false).unwrap().bucket, "raw");
        assert_eq!(classify("panel_x", "panel", "panel_11881", false, false, false).unwrap().bucket, "raw");
    }

    #[test]
    fn exemptions_are_skipped() {
        // fragment-referenced
        assert!(classify("panel_x", "panel", "PN_x", true, false, false).is_none());
        // affrg in name / AF_ prefix
        assert!(classify("panel_x", "panel", "affrg_Address", false, false, false).is_none());
        assert!(classify("textbox_x", "textbox", "AF_bound", false, false, false).is_none());
        // system resourceType + layouts
        assert!(classify("toolbar_x", "toolbar", "whatever", false, false, false).is_none());
        assert!(classify("layout", "gridFluidLayout2", "x", false, false, false).is_none());
        // preview construct + Title wrappers + system panels
        assert!(classify("tertiarybutton_x", "tertiarybutton", "preview", false, false, false).is_none());
        assert!(classify("panel_title_x", "panel", "PN_FooTitle", false, false, false).is_none());
        assert!(classify("panel_x", "panel", "guideRootPanel", false, false, false).is_none());
        // unmapped resourceType → skip (don't guess)
        assert!(classify("weird_x", "somethingElse", "XX_y", false, false, false).is_none());
    }

    #[test]
    fn panel_roles_and_repeat_context() {
        // plain panel expects PN
        assert_eq!(classify("panel_x", "panel", "PN_Foo", false, false, false).unwrap().bucket, "ok");
        // repeat panel (own minOccur/maxOccur) → RC* family, high confidence
        let v = classify("panel_x", "panel", "RCP_Foo", false, true, false).unwrap();
        assert_eq!((v.role.as_str(), v.confidence), ("repeat-panel", "high"));
        // sub-panel inside a repeat container → low confidence, PN also accepted
        let v = classify("panel_x", "panel", "PN_Inner", false, false, true).unwrap();
        assert_eq!((v.bucket, v.role.as_str(), v.confidence), ("ok", "repeat-subpanel", "low"));
    }

    #[test]
    fn buttons_expect_bt() {
        assert_eq!(classify("removebutton_x", "removebutton", "BT_Remove", false, false, false).unwrap().bucket, "ok");
        // radiobutton must NOT be caught by the button rule (it's a mapped field type)
        let v = classify("radiobutton_x", "radiobutton", "RB_Choice", false, false, false).unwrap();
        assert_eq!((v.bucket, v.role.as_str()), ("ok", "radiobutton"));
    }

    #[test]
    fn xml_walk_reports_violations_with_counts() {
        // Two named nodes: a conforming textbox and a raw-named panel.
        let xml = r#"<jcr:root xmlns:jcr="j" xmlns:sling="s">
            <panel_1 sling:resourceType="ubs/controls/panel" name="myPanel"/>
            <textbox_1 sling:resourceType="ubs/controls/textbox" name="TXT_First_ab12cd34"/>
        </jcr:root>"#;
        let (viol, counts, _) = check_naming_conventions(xml);
        assert_eq!(counts, [1, 0, 1], "one ok (textbox), one raw (panel)");
        assert_eq!(viol.len(), 1);
        assert_eq!(viol[0].name, "myPanel");
        assert_eq!((viol[0].bucket.as_str(), viol[0].expected.as_str()), ("raw", "PN"));
        assert_eq!(viol[0].role, "panel");
    }

    #[test]
    fn xml_walk_tracks_repeat_ancestry() {
        // A sub-panel is only "repeat-subpanel" when nested under a repeatable.
        let xml = r#"<r xmlns:sling="s">
            <repeatable_1 sling:resourceType="ubs/controls/panel" name="RCP_Rows" maxOccur="4">
                <panel_2 sling:resourceType="ubs/controls/panel" name="PN_Inner"/>
            </repeatable_1>
        </r>"#;
        let (viol, counts, _) = check_naming_conventions(xml);
        // RCP_Rows (repeat-panel, ok) + PN_Inner (repeat-subpanel, ok) → no violations.
        assert_eq!(counts, [2, 0, 0], "both conform, got {viol:?}");
        assert!(viol.is_empty());
    }

    /// AALJ shipped a radio button with no `jcr:title` at all: the label
    /// attacher found no free text block for it, and nothing downstream
    /// asserted that an input ends up labelled.
    #[test]
    fn untitled_input_is_reported() {
        let xml = r#"<r xmlns:sling="s" xmlns:jcr="j">
            <radiobutton_1 sling:resourceType="ubs/controls/radiobutton" name="RB_UsedFor"
                options="[RB_1=business purposes,RB_2=private purposes]"/>
            <textbox_1 sling:resourceType="ubs/controls/textbox" name="TXT_CommValue"/>
            <textbox_2 sling:resourceType="ubs/controls/textbox" name="TXT_Ok" jcr:title="Last name"/>
        </r>"#;
        let (_, _, labels) = check_naming_conventions(xml);
        let names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["RB_UsedFor", "TXT_CommValue"], "got {labels:?}");
        assert!(labels.iter().all(|l| l.kind == "missing" && l.confidence == "high"));
    }

    /// A single-option checkbox carries its label in the option by design
    /// (`converter.rs`, `FieldType::Bool`), so an empty title is not a defect
    /// there — but a choice group whose options carry no labels still is.
    #[test]
    fn single_checkbox_label_may_live_in_its_option() {
        let xml = r#"<r xmlns:sling="s" xmlns:jcr="j">
            <checkbox_1 sling:resourceType="ubs/controls/checkbox" name="CB_RecordCalls"
                options="[1=Recording of telephone calls]"/>
            <checkbox_2 sling:resourceType="ubs/controls/checkbox" name="CB_Bare" options="[1=]"/>
        </r>"#;
        let (_, _, labels) = check_naming_conventions(xml);
        assert_eq!(labels.len(), 1, "only the label-less group is a defect: {labels:?}");
        assert_eq!(labels[0].name, "CB_Bare");
    }

    /// AAEJ shipped a radio button whose title was the parenthetical hint next
    /// to it — `(Mandatory entry only if NMS IDD hit is "True")` — instead of
    /// the question, which stayed behind in a separate static text.
    #[test]
    fn malformed_titles_are_classified() {
        let xml = r#"<r xmlns:sling="s" xmlns:jcr="j">
            <radiobutton_1 sling:resourceType="ubs/controls/radiobutton" name="RB_A1Q3"
                jcr:title="(Mandatory entry only if NMS IDD hit is &quot;True&quot;)" options="[1=Yes,2=No]"/>
            <textbox_1 sling:resourceType="ubs/controls/textbox" name="TXT_Rich"
                jcr:title="&lt;p&gt;Company&lt;/p&gt;"/>
            <textbox_2 sling:resourceType="ubs/controls/textbox" name="TXT_Quoted"
                jcr:title="Classification as &quot;Eligible Counterparty&quot; can be approved."/>
        </r>"#;
        let (_, _, labels) = check_naming_conventions(xml);
        let got: Vec<(&str, &str, &str)> = labels
            .iter()
            .map(|l| (l.name.as_str(), l.kind.as_str(), l.confidence.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("RB_A1Q3", "parenthetical", "high"),
                ("TXT_Rich", "markup", "high"),
                // quotes also occur in genuine reference-form labels.
                ("TXT_Quoted", "quoted", "low"),
            ]
        );
        assert!(
            labels[0].title.contains('"'),
            "the reported title is unescaped: {:?}",
            labels[0].title
        );
    }

    /// Draws and panels legitimately carry no title — only inputs are checked.
    #[test]
    fn non_input_components_are_not_label_checked() {
        let xml = r#"<r xmlns:sling="s" xmlns:jcr="j">
            <panel_1 sling:resourceType="ubs/controls/panel" name="PN_Section"/>
            <textdraw_1 sling:resourceType="ubs/controls/textdraw" name="ST_Question"/>
            <titledraw_1 sling:resourceType="ubs/controls/titledraw" name="TTL_Section"/>
        </r>"#;
        let (_, _, labels) = check_naming_conventions(xml);
        assert!(labels.is_empty(), "got {labels:?}");
    }

    #[test]
    fn review_output_runs_naming_on_rendered_xml() {
        // A convert → review round-trip renders and scans without error.
        let input = sample_input();
        let cfg = AemConfig::test_default("TEST");
        let aem = crate::convert_to_aem(&input, &cfg);
        let report = review_output(&input, &aem, &cfg, "en");
        // naming_violations is populated from the rendered XML (may be empty).
        let _ = &report.naming_violations;
    }
}

