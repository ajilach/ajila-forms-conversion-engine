//! XML serialization of an `AemNode` tree into AEM JCR content XML.
//!
//! Uses Tera templates loaded from the profile directory. Each `AemNode` type
//! is rendered by its corresponding `*.xml` template file. The `root.xml`
//! template is the entire XML document — the writer itself generates no XML
//! tags.

use std::collections::HashMap;

use uuid::Uuid;

use super::{
    AemConfig, AemNode, AemOption, ConditionRule, OptionAlignment, Passthrough, TextFieldKind,
};
use crate::aem::template;
use crate::structured::InputValue;
use crate::util::escape_html as xml_escape;

/// No fidelity passthrough (the engine / from-XFA path): every node renders
/// purely from its template, exactly as before.
fn no_passthrough() -> &'static HashMap<Uuid, Passthrough> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<HashMap<Uuid, Passthrough>> = OnceLock::new();
    EMPTY.get_or_init(HashMap::new)
}

// ============================================================================
// Public API
// ============================================================================

/// Serialize an `AemNode` tree (starting from `Root`) into a complete AEM
/// JCR content XML string.
///
/// Each node is rendered by the correspondingly named template from
/// `config.component_templates`. Attributes are post-processed to appear
/// one per line (matching AEM's export style).
pub fn generate_aem_xml(root: &AemNode, config: &AemConfig) -> String {
    generate_aem_xml_with_passthrough(root, config, no_passthrough())
}

/// Like [`generate_aem_xml`], but re-emits each node's captured fidelity
/// [`Passthrough`] (raw attributes + unmodeled child elements), keyed by node
/// uuid. Used when saving a working tree that was loaded from an existing
/// package, so a load→save round-trip preserves attributes/children the typed
/// model does not represent. Pass an empty map for the from-XFA engine path.
pub fn generate_aem_xml_with_passthrough(
    root: &AemNode,
    config: &AemConfig,
    pass: &HashMap<Uuid, Passthrough>,
) -> String {
    // Invert the trigger-field condition rules into a per-panel map so each
    // conditional panel can carry its own AABO-style `fd:visible` SHOW_EXPRESSION.
    let index = RenderIndex::build(root);
    let rendered = render_node(root, config, &index, pass);
    reformat_attributes(&rendered)
}

// ============================================================================
// Panel visibility map (AABO-style `fd:visible` SHOW_EXPRESSION)
// ============================================================================

/// Maps a conditional panel's AEM `name` to the `(trigger_field, value)` pairs
/// that should make it visible. Built by inverting the `ConditionRule`s that
/// the converter wired onto trigger fields (radio/checkbox/dropdown).
type PanelVisibilityMap = HashMap<String, Vec<(String, InputValue)>>;

/// Per-render indices derived from the whole tree before any node is written.
///
/// Both answer a question a single node cannot: which trigger values reveal this
/// panel, and which panels does this choice decide. They are built once, from the
/// root, because a node's own subtree does not contain the answer.
struct RenderIndex {
    visibility: PanelVisibilityMap,
    /// Trigger field name → the panels it decides, for approved configurator
    /// choices only. Absent means "no reset for this field".
    resets: HashMap<String, Vec<ResetTarget>>,
}

impl RenderIndex {
    fn build(root: &AemNode) -> Self {
        Self {
            visibility: collect_panel_visibility(root),
            resets: collect_configurator_resets(root),
        }
    }
}

/// Walk the tree and, for every choice whose wording is an approved
/// configurator, record the panels it decides.
fn collect_configurator_resets(root: &AemNode) -> HashMap<String, Vec<ResetTarget>> {
    let mut map = HashMap::new();
    collect_configurator_resets_rec(root, root, &mut map);
    map
}

fn collect_configurator_resets_rec(
    root: &AemNode,
    node: &AemNode,
    map: &mut HashMap<String, Vec<ResetTarget>>,
) {
    let choice = match node {
        AemNode::RadioButton {
            name,
            options,
            conditions,
            ..
        }
        | AemNode::Dropdown {
            name,
            options,
            conditions,
            ..
        } => Some((name, options, conditions)),
        // A checkbox group is a multi-select; "the option that was chosen
        // instead" is not a well-defined thing to clear behind, and no approved
        // configurator wording is a checkbox in this corpus.
        _ => None,
    };
    if let Some((name, options, conditions)) = choice
        && is_approved_configurator(options)
    {
        let targets = reset_targets_for(root, conditions);
        if !targets.is_empty() {
            map.insert(name.clone(), targets);
        }
    }

    match node {
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => {
            for child in children {
                collect_configurator_resets_rec(root, child, map);
            }
        }
        _ => {}
    }
}

/// Walk the node tree and invert every trigger field's `show` conditions into
/// a `panel_name → [(trigger_field, value), …]` map.
fn collect_panel_visibility(root: &AemNode) -> PanelVisibilityMap {
    let mut map = PanelVisibilityMap::new();
    collect_panel_visibility_rec(root, &mut map);
    map
}

fn collect_panel_visibility_rec(node: &AemNode, map: &mut PanelVisibilityMap) {
    match node {
        AemNode::RadioButton {
            name, conditions, ..
        }
        | AemNode::Checkbox {
            name, conditions, ..
        }
        | AemNode::Dropdown {
            name, conditions, ..
        } => {
            for rule in conditions {
                if rule.show {
                    map.entry(rule.target_panel_name.clone())
                        .or_default()
                        .push((name.clone(), rule.value.clone()));
                }
            }
        }
        _ => {}
    }

    match node {
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => {
            for child in children {
                collect_panel_visibility_rec(child, map);
            }
        }
        _ => {}
    }
}

/// Whether a subtree holds anything a user can fill in.
///
/// This is what decides a wizard step's jump-to-field button. The button jumps
/// the reader back to a field, so a step that is pure text — legal provisions, a
/// declaration, terms to accept — has nothing to jump to and must not offer one
/// (owner directive, 2026-08-10; 177 steps across 102 forms carried one wrongly).
///
/// A fragment counts as input even though its children are not in this package:
/// the fields live in the fragment's own package, and in this corpus these are
/// signature, banking and address fragments, all fillable. A custom element
/// counts for the same reason — its body is opaque profile XML that the engine
/// cannot see into, and every custom element in the profile carries fields.
///
/// Draws, footnotes, prefaces and appendices are static text and count for
/// nothing, which is exactly the case this exists to detect.
fn holds_input(node: &AemNode) -> bool {
    match node {
        AemNode::TextField { .. }
        | AemNode::NumberField { .. }
        | AemNode::DatePicker { .. }
        | AemNode::Dropdown { .. }
        | AemNode::Checkbox { .. }
        | AemNode::RadioButton { .. }
        | AemNode::Fragment { .. }
        | AemNode::Custom { .. } => true,

        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => children.iter().any(holds_input),

        AemNode::TextDraw { .. }
        | AemNode::TitleDraw { .. }
        | AemNode::Preface { .. }
        | AemNode::Appendix { .. }
        | AemNode::FootnotePlaceholder { .. } => false,
    }
}

// ============================================================================
// Configurator reset-on-change (feedback #107)
// ============================================================================

/// Option wordings of a form-configurator choice, as reviewed and approved for
/// the reset (`configurator_reset.py::APPROVED_LABEL_SETS`).
///
/// Identifying the configurator by its wording is a deliberate narrowing, not a
/// shortcut. Structurally, *any* choice that reveals two or more panels has the
/// same defect — switch option, come back, and the first option's panel returns
/// with every field still filled. But plenty of such choices are ordinary
/// questions inside a form (an order type, an occupation) where whether the
/// answer should be wiped is a judgement about that form. So only wordings a
/// person has confirmed get a reset written for them; anything else is left
/// alone. Adding a wording here means giving it the same review these had.
///
/// Compared casefolded, in order, as a whole set: a choice offering one of these
/// sets plus another option does not match.
const APPROVED_CONFIGURATOR_LABEL_SETS: &[&[&str]] = &[
    &["Individual", "Company/Entity"],
    &["Private Person", "Minderjährige", "Firma", "GbR"],
    &["Individual", "Legal Entity"],
    &["Individuo", "Entità giuridica"],
    &["Individuale", "Persona giuridica / Società / Ditta"],
    &["Private Person", "Firma"],
    &["Private Person", "Minderjährige", "Firma"],
    &["Individual", "Legal entity"],
    &["Persona", "Persona giuridica"],
    &["Individual", "Corporate"],
    &["Private Person", "Minderjährige"],
    &["For financial institutions", "For natural persons"],
];

/// One panel a configurator choice decides, and the repeatables inside it.
///
/// The repeatables are reset first: a repeatable has to drop its added rows, not
/// just blank them, so the row count is back to its declared minimum before the
/// remaining fields are cleared.
#[derive(serde::Serialize)]
struct ResetTarget {
    panel: String,
    repeats: Vec<String>,
}

/// Whether a choice's option labels are one of the approved configurator sets.
fn is_approved_configurator(options: &[AemOption]) -> bool {
    let labels: Vec<String> = options.iter().map(|o| o.label.to_lowercase()).collect();
    APPROVED_CONFIGURATOR_LABEL_SETS.iter().any(|set| {
        set.len() == labels.len()
            && set
                .iter()
                .zip(&labels)
                .all(|(approved, actual)| approved.to_lowercase() == *actual)
    })
}

/// The panels a trigger field reveals, in document order, each with the
/// repeatables nested inside it.
///
/// The set is read from the same `ConditionRule`s that drive the panels'
/// `fd:visible` expressions, so the reset covers exactly what the choice shows
/// and hides — no more. Order follows the panels' order in the tree, which is
/// what makes the generated script stable across runs.
fn reset_targets_for(root: &AemNode, conditions: &[ConditionRule]) -> Vec<ResetTarget> {
    let mut wanted: Vec<&str> = Vec::new();
    for rule in conditions.iter().filter(|r| r.show) {
        if !wanted.contains(&rule.target_panel_name.as_str()) {
            wanted.push(&rule.target_panel_name);
        }
    }
    // A choice that reveals fewer than two panels has nothing to clear behind
    // it — there is no "other option" whose data could resurface.
    if wanted.len() < 2 {
        return Vec::new();
    }

    let mut ordered: Vec<ResetTarget> = Vec::new();
    collect_reset_targets(root, &wanted, &mut ordered);
    // A rule may name a panel that is not in the tree, and two panels can share
    // a name. Either way the invariant is about what will actually be reset:
    // below two panels there is no other option whose data could resurface, and
    // a partial reset would be worse than none — it would clear some panels and
    // leave the reader to discover the rest still filled.
    // Not `dedup_by`: two panels sharing a name need not be adjacent in the
    // tree, and only the first occurrence should be reset (resetting twice says
    // nothing, but it does mislead anyone reading the script).
    let mut seen = std::collections::HashSet::new();
    ordered.retain(|t| seen.insert(t.panel.clone()));
    if ordered.len() < 2 {
        return Vec::new();
    }
    ordered
}

fn collect_reset_targets(node: &AemNode, wanted: &[&str], out: &mut Vec<ResetTarget>) {
    if let AemNode::Panel { name, children, .. } = node {
        if wanted.contains(&name.as_str()) {
            out.push(ResetTarget {
                panel: name.clone(),
                repeats: repeatable_panels(children),
            });
            // Panels the choice decides are not nested inside one another, and
            // descending would attribute a nested match to the wrong parent.
            return;
        }
    }
    match node {
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => {
            for child in children {
                collect_reset_targets(child, wanted, out);
            }
        }
        _ => {}
    }
}

/// The instance-managed panel of every repeatable in a subtree, in document
/// order. `repeatable.xml` names it `<repeatable>_repeat`, and that is the node
/// `resetAllPanelInstances` has to be given.
fn repeatable_panels(children: &[AemNode]) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(node: &AemNode, out: &mut Vec<String>) {
        if let AemNode::Repeatable { name, .. } = node {
            out.push(format!("{}_repeat", name));
        }
        match node {
            AemNode::Root { children, .. }
            | AemNode::Panel { children, .. }
            | AemNode::Repeatable { children, .. } => {
                for child in children {
                    walk(child, out);
                }
            }
            _ => {}
        }
    }
    for child in children {
        walk(child, &mut out);
    }
    out
}

// ============================================================================
// Template-based node rendering
// ============================================================================

/// Render a single node using its template from `config.component_templates`.
///
/// If no template exists for the node type, an empty string is returned
/// (the component is omitted from the output).
fn render_node(
    node: &AemNode,
    config: &AemConfig,
    index: &RenderIndex,
    pass: &HashMap<Uuid, Passthrough>,
) -> String {
    // Custom nodes use a separate template lookup.
    if let AemNode::Custom { template_key, .. } = node {
        let template = match config.custom_templates.get(template_key) {
            Some(tmpl) => tmpl,
            None => {
                log::error!(
                    "Custom template '{}' not found in custom_templates",
                    template_key
                );
                return String::new();
            }
        };
        let mut ctx = build_node_context(node, config, index, pass);
        insert_passthrough(&mut ctx, node, pass, template);
        return match template::render_string(template, &ctx) {
            Ok(rendered) => rendered,
            Err(e) => {
                log::error!("Failed to render custom template '{}': {}", template_key, e);
                String::new()
            }
        };
    }

    let template_key = match node {
        AemNode::Root { .. } => "root",
        AemNode::Panel {
            is_conditional: true,
            ..
        } => "conditional",
        AemNode::Panel { .. } => "panel",
        AemNode::TextField { kind, .. } => kind.template_key(),
        AemNode::NumberField { .. } => "numericbox",
        AemNode::DatePicker { .. } => "datepicker",
        AemNode::Dropdown { .. } => "dropdownlist",
        AemNode::Checkbox { .. } => "checkbox",
        AemNode::RadioButton { .. } => "radiobutton",
        AemNode::TextDraw { .. } => "textdraw",
        AemNode::TitleDraw { .. } => "titledraw",
        AemNode::Repeatable { .. } => "repeatable",
        AemNode::Fragment { .. } => "fragment",
        AemNode::Preface { .. } => "preface",
        AemNode::Appendix { .. } => "appendix",
        AemNode::FootnotePlaceholder { .. } => "footnoteplaceholder",
        AemNode::Custom { .. } => unreachable!(),
    };

    let template = match config.component_templates.get(template_key) {
        Some(tmpl) => tmpl,
        // A typed text input falls back to the plain text box when the profile
        // ships no template for its kind. A missing template otherwise means the
        // field is dropped from the output entirely, and losing a field is far
        // worse than losing its validation clause.
        None => match node {
            AemNode::TextField { kind, .. } if *kind != TextFieldKind::Plain => {
                log::warn!(
                    "profile has no '{}' template; falling back to 'textbox'",
                    template_key
                );
                match config.component_templates.get("textbox") {
                    Some(tmpl) => tmpl,
                    None => return String::new(),
                }
            }
            _ => return String::new(),
        },
    };

    let mut ctx = build_node_context(node, config, index, pass);
    insert_passthrough(&mut ctx, node, pass, template);
    match template::render_string(template, &ctx) {
        Ok(rendered) => rendered,
        Err(e) => {
            log::error!("Failed to render template '{}': {}", template_key, e);
            String::new()
        }
    }
}

/// Collect the attribute names a template writes itself, by scanning its text
/// for `name="` tokens (both hard-coded attributes and Tera-guarded ones like
/// `{% if x %}foo="…"`). A loaded node's [`Passthrough`] must NOT re-emit any of
/// these — the template already writes them — or the element would have a
/// duplicate attribute (invalid XML). Everything else the template does not own
/// flows through `extra_attributes`.
///
/// Deriving the set from the template text (rather than a hand-maintained global
/// list) keeps it precise per template: an attribute one template owns (e.g.
/// `dorExclusion` on a field) is not wrongly suppressed on another template that
/// never writes it (e.g. a panel), so that attribute survives via passthrough.
///
/// (Preserving a template-owned value *exactly* when it differs from the
/// template's own output is a separate override step; for engine-origin packages
/// they already match.)
fn template_owned_attrs(template: &str) -> std::collections::HashSet<&str> {
    let bytes = template.as_bytes();
    let mut set = std::collections::HashSet::new();
    let mut i = 0;
    while i < bytes.len() {
        // An attribute is written as `name="`; find each `="` and walk left over
        // the identifier characters to recover the name.
        if bytes[i] == b'=' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
            let mut start = i;
            while start > 0 {
                let c = bytes[start - 1];
                if c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.' | b'-') {
                    start -= 1;
                } else {
                    break;
                }
            }
            if start < i {
                set.insert(&template[start..i]);
            }
        }
        i += 1;
    }
    set
}

/// The node's uuid (for looking up its [`Passthrough`]); `Root` has none.
fn node_uuid(node: &AemNode) -> Option<Uuid> {
    match node {
        AemNode::Root { .. } => None,
        AemNode::Panel { uuid, .. }
        | AemNode::TextField { uuid, .. }
        | AemNode::NumberField { uuid, .. }
        | AemNode::DatePicker { uuid, .. }
        | AemNode::Dropdown { uuid, .. }
        | AemNode::Checkbox { uuid, .. }
        | AemNode::RadioButton { uuid, .. }
        | AemNode::TextDraw { uuid, .. }
        | AemNode::TitleDraw { uuid, .. }
        | AemNode::Repeatable { uuid, .. }
        | AemNode::Fragment { uuid, .. }
        | AemNode::Preface { uuid, .. }
        | AemNode::Appendix { uuid, .. }
        | AemNode::FootnotePlaceholder { uuid, .. }
        | AemNode::Custom { uuid, .. } => Some(*uuid),
    }
}

/// Insert the node's captured fidelity passthrough into its render context as two
/// pre-escaped strings the templates emit verbatim: `extra_attributes` (raw
/// attributes the typed model + template don't own) and `raw_children` (unmodeled
/// child XML). Empty when the node carries no passthrough (engine-built nodes).
fn insert_passthrough(
    ctx: &mut tera::Context,
    node: &AemNode,
    pass: &HashMap<Uuid, Passthrough>,
    template: &str,
) {
    let pt = node_uuid(node).and_then(|u| pass.get(&u));
    let (extra, children) = match pt {
        Some(p) => {
            // Only the node's OWN opening tag owns attributes; nested child
            // elements in the template (e.g. a panel's `panel_title`) must not
            // count. `{{ extra_attributes }}` was inserted immediately before the
            // opening tag's closing `>`, so it bounds the own-tag region.
            let head = template
                .split("{{ extra_attributes }}")
                .next()
                .unwrap_or(template);
            let owned = template_owned_attrs(head);
            let extra: String = p
                .raw_attributes
                .iter()
                .filter(|(k, _)| !owned.contains(k.as_str()))
                .map(|(k, v)| format!(" {}=\"{}\"", k, xml_escape(v)))
                .collect();
            (extra, p.raw_children.join("\n"))
        }
        None => (String::new(), String::new()),
    };
    // Templates that hard-code an empty `<fd:rules/>` must suppress it when the
    // node's passthrough already carries an `fd:rules` element — otherwise every
    // save would append another empty one (unbounded growth) and the real rules
    // would be duplicated.
    ctx.insert("has_passthrough_rules", &children.contains("<fd:rules"));
    ctx.insert("extra_attributes", &extra);
    ctx.insert("raw_children", &children);
}

/// Render all children of a node and concatenate the results.
fn render_children(
    children: &[AemNode],
    config: &AemConfig,
    index: &RenderIndex,
    pass: &HashMap<Uuid, Passthrough>,
) -> String {
    children
        .iter()
        .map(|c| render_node(c, config, index, pass))
        .collect()
}

/// Build a Tera context for a single node.
///
/// The context contains:
/// - Global variables: `xfa.*`, `variables.*`, `author`, `master_language`,
///   `languages`, `expanded_languages`
/// - Node-specific variables depending on the variant
fn build_node_context(
    node: &AemNode,
    config: &AemConfig,
    index: &RenderIndex,
    pass: &HashMap<Uuid, Passthrough>,
) -> tera::Context {
    let mut ctx = tera::Context::new();

    // ── Global context ─────────────────────────────────────────────────
    ctx.insert("xfa", &config.xfa_vars);
    ctx.insert("variables", &config.user_vars);
    ctx.insert("author", &config.author);
    ctx.insert("master_language", &config.master_language);
    // The canonical codes, not the detected ones: a language that reached the
    // tree under a synonym (`es`) must be named on the form under the code the
    // platform files it as (`sp`).
    ctx.insert("languages", &config.canonical_languages().join(","));
    ctx.insert("expanded_languages", &config.expand_languages().join(","));

    // ── Node-specific context ──────────────────────────────────────────
    ctx.insert("element_name", &node.element_name());

    match node {
        AemNode::Root { title, children } => {
            ctx.insert("title", &xml_escape(title));
            ctx.insert("form_code", &config.form_code);
            ctx.insert("children", &render_children(children, config, index, pass));
        }

        AemNode::Panel {
            uuid,
            name,
            title,
            children,
            is_page,
            dor_exclude,
            visible,
            is_conditional,
            dor_num_cols,
            colspan,
            dor_colspan,
            bind_ref,
            frag_ref: _,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("title", &xml_escape(title));
            ctx.insert("is_page", is_page);
            ctx.insert("dor_exclude", dor_exclude);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_num_cols", dor_num_cols);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
            ctx.insert("children", &render_children(children, config, index, pass));
            ctx.insert("has_input", &children.iter().any(holds_input));

            // Conditional panels carry an AABO-style `fd:visible` SHOW_EXPRESSION
            // that toggles both form visibility and DOR inclusion via the UBS
            // `showAFShowDor`/`hideAFHideDor` helpers. Only the structured
            // `(trigger_field, value)` pairs are passed here — the SHOW_EXPRESSION
            // JSON is assembled by the `conditional` template. When present, the
            // expression governs visibility, so the static `visible` attribute is
            // suppressed (see conditional.xml).
            if *is_conditional {
                if let Some(triggers) = index.visibility.get(name) {
                    if !triggers.is_empty() {
                        let trigger_ctx: Vec<HashMap<&str, String>> = triggers
                            .iter()
                            .map(|(field, value)| {
                                HashMap::from([
                                    ("field", field.clone()),
                                    ("value", condition_value_str(value)),
                                ])
                            })
                            .collect();
                        ctx.insert("visibility_triggers", &trigger_ctx);
                    }
                }
            }
        }

        AemNode::TextField {
            uuid,
            name,
            label,
            mandatory,
            visible,
            max_chars,
            colspan,
            dor_colspan,
            bind_ref,
            kind: _,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("max_chars", max_chars);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
        }

        AemNode::NumberField {
            uuid,
            name,
            label,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
        }

        AemNode::DatePicker {
            uuid,
            name,
            label,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
        }

        AemNode::Dropdown {
            uuid,
            name,
            label,
            options,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            field_id: _,
            // Visibility is now emitted on the target panel (AABO-style
            // `fd:visible` SHOW_EXPRESSION), not as a `fd:valueCommit` on the
            // trigger field. See `collect_panel_visibility`.
            conditions: _,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
            insert_options_context(&mut ctx, options);
        }

        AemNode::Checkbox {
            uuid,
            name,
            label,
            options,
            alignment,
            visible,
            colspan,
            dor_colspan,
            field_id: _,
            conditions: _,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
            ctx.insert("alignment", alignment_str(*alignment));
            insert_options_context(&mut ctx, options);
            // text_is_rich: array of booleans indicating rich text options
            let text_is_rich: Vec<bool> = options.iter().map(|o| o.label.contains('<')).collect();
            let text_is_rich_str = format!(
                "[{}]",
                text_is_rich
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            ctx.insert("text_is_rich", &text_is_rich_str);
        }

        AemNode::RadioButton {
            uuid,
            name,
            label,
            options,
            alignment,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            field_id: _,
            conditions: _,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
            ctx.insert("alignment", alignment_str(*alignment));
            insert_options_context(&mut ctx, options);
            // text_is_rich
            let text_is_rich: Vec<bool> = options.iter().map(|o| o.label.contains('<')).collect();
            let text_is_rich_str = format!(
                "[{}]",
                text_is_rich
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            ctx.insert("text_is_rich", &text_is_rich_str);
        }

        AemNode::TextDraw {
            uuid,
            name,
            content,
            dor_exclude,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("content", &xml_escape(content));
            ctx.insert("dor_exclude", dor_exclude);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::TitleDraw {
            uuid,
            name,
            content,
            heading_level,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("content", &xml_escape(content));
            ctx.insert("heading_level", heading_level);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::Repeatable {
            uuid,
            name,
            title,
            children,
            min_occur,
            max_occur,
            bind_ref,
            frag_ref: _,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("title", &xml_escape(title));
            ctx.insert("min_occur", min_occur);
            // AEM spells an unbounded repeat `maxOccur="-1"`; the model carries
            // that as `UNBOUNDED_OCCUR`, so map it back on the way out.
            let max_occur_attr = if *max_occur == AemNode::UNBOUNDED_OCCUR {
                "-1".to_string()
            } else {
                max_occur.to_string()
            };
            ctx.insert("max_occur", &max_occur_attr);
            ctx.insert("children", &render_children(children, config, index, pass));
            ctx.insert("bind_ref", bind_ref);

            // The outer panel is already `RCP_…`; suffix (rather than re-prefix)
            // the inner repeatable panel so it stays under the same prefix
            // without producing a doubled `RCP_RCP_…` name.
            let panel_name = format!("{}_repeat", name);
            ctx.insert("panel_name", &panel_name);
        }

        AemNode::Fragment {
            uuid,
            name,
            title,
            frag_ref,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("title", title);
            ctx.insert("frag_ref", frag_ref);
            ctx.insert("bind_ref", bind_ref);
        }

        AemNode::Preface { uuid, name } | AemNode::Appendix { uuid, name } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
        }

        AemNode::FootnotePlaceholder {
            uuid,
            name,
            colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("colspan", colspan);
        }

        AemNode::Custom {
            uuid,
            name,
            template_key: _,
            label,
            options,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            bind_ref,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("bind_ref", bind_ref);
            insert_options_context(&mut ctx, options);
        }
    }

    // A form-configurator choice must empty every panel it decides, so switching
    // option and coming back never presents one option's panel carrying
    // another's data (feedback #107). Keyed on the node's own name, so only the
    // choices `RenderIndex` approved get the script.
    if let Some(name) = node_name(node)
        && let Some(targets) = index.resets.get(name)
    {
        ctx.insert("reset_targets", targets);
    }

    ctx
}

/// A node's AEM `name`, where it has one. `Root` does not.
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

/// Insert options-related variables into a Tera context.
fn insert_options_context(ctx: &mut tera::Context, options: &[AemOption]) {
    ctx.insert("options_attr", &format_options_attr(options));
    ctx.insert("options_count", &options.len());
    let opt_list: Vec<HashMap<&str, &str>> = options
        .iter()
        .map(|o| {
            let mut m = HashMap::new();
            m.insert("label", o.label.as_str());
            m.insert("value", o.value.as_str());
            m
        })
        .collect();
    ctx.insert("options", &opt_list);
}

// ============================================================================
// Conditional visibility scripts (fd:scripts fd:visible SHOW_EXPRESSION)
// ============================================================================
//
// This matches the reference UBS form (AF_AABO): each dynamically-hidden panel
// carries a `fd:visible` SHOW_EXPRESSION on the panel itself. The expression
// returns the boolean visibility AND, as a side effect, calls the UBS DOR
// helpers so the panel is included/excluded from the Document of Record.
//
// The full SHOW_EXPRESSION JSON is assembled by the `conditional` template from
// the `visibility_triggers` context list; the only Rust-side concern is turning
// each trigger value into its string form for the `==` comparison.

/// Render a single `InputValue` as the string used in a `==` comparison.
fn condition_value_str(value: &InputValue) -> String {
    match value {
        InputValue::Text(s) => s.clone(),
        InputValue::Number(n) => n.to_string(),
        InputValue::Bool(b) => b.to_string(),
    }
}

// ============================================================================
// Attribute helpers
// ============================================================================

fn alignment_str(a: OptionAlignment) -> &'static str {
    match a {
        OptionAlignment::Horizontal => "horizontal",
        OptionAlignment::Vertical => "vertical",
    }
}

/// Escape a string for use inside a JCR comma-separated list.
///
/// Backslashes and commas must be backslash-escaped so that the list can be
/// split unambiguously on unescaped commas when parsed back.
fn jcr_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace(',', "\\,")
}

/// Format options for checkbox/radio/dropdown as `[value1=label1,value2=label2,...]`.
fn format_options_attr(options: &[AemOption]) -> String {
    let inner: Vec<String> = options
        .iter()
        .map(|o| {
            format!(
                "{}={}",
                jcr_escape(&xml_escape(&o.value)),
                jcr_escape(&xml_escape(&o.label)),
            )
        })
        .collect();
    format!("[{}]", inner.join(","))
}

// ============================================================================
// Attribute reformatting (one-per-line)
// ============================================================================

/// Reformat XML so that element attributes appear one per line, indented to
/// align with the first attribute.
///
/// Turns:
/// ```xml
///     <tag attr1="v1" attr2="v2">
/// ```
/// into:
/// ```xml
///     <tag
///         attr1="v1"
///         attr2="v2">
/// ```
///
/// Only elements with more than one attribute are reformatted.
pub(crate) fn reformat_attributes(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() + xml.len() / 4);

    for line in xml.lines() {
        if let Some(reformatted) = try_reformat_line(line) {
            out.push_str(&reformatted);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    out
}

/// Try to reformat a single XML line. Returns `None` if the line should be
/// kept as-is (not an element, or has ≤1 attribute).
fn try_reformat_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();

    // Must start with '<' and not be a closing tag, comment, PI, or declaration
    if !trimmed.starts_with('<')
        || trimmed.starts_with("</")
        || trimmed.starts_with("<?")
        || trimmed.starts_with("<!")
    {
        return None;
    }

    // Find the leading indentation
    let indent = &line[..line.len() - trimmed.len()];

    // Parse the tag name and attributes.
    // Find the end of the opening tag (matching '>' or '/>').
    let (tag_content, suffix) = extract_tag_content(trimmed)?;

    // Split into tag name and attributes
    let first_space = tag_content.find(' ')?;
    let tag_name = &tag_content[1..first_space]; // skip '<'
    let attrs_str = &tag_content[first_space + 1..];

    // Parse attributes
    let attrs = parse_attributes(attrs_str);
    if attrs.len() <= 1 {
        return None;
    }

    // Build reformatted output
    let attr_indent = format!("{}{}", indent, " ".repeat(tag_name.len() + 2)); // +2 for '<' and space
    let mut result = format!("{}<{}", indent, tag_name);
    for (i, attr) in attrs.iter().enumerate() {
        if i == 0 {
            result.push(' ');
        } else {
            result.push('\n');
            result.push_str(&attr_indent);
        }
        result.push_str(attr);
    }
    result.push_str(suffix);

    Some(result)
}

/// Extract the content between '<' ... '>' or '<' ... '/>', returning
/// (content_without_close, suffix). Suffix is ">" or "/>" or "/>".
fn extract_tag_content(trimmed: &str) -> Option<(&str, &str)> {
    if let Some(stripped) = trimmed.strip_suffix("/>") {
        Some((stripped, "/>"))
    } else if let Some(stripped) = trimmed.strip_suffix('>') {
        Some((stripped, ">"))
    } else {
        None
    }
}

/// Parse a string of XML attributes like `attr1="val1" attr2="val2"` into
/// a vec of individual attribute strings.
fn parse_attributes(s: &str) -> Vec<&str> {
    let mut attrs = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    let len = bytes.len();

    while i < len {
        // Skip whitespace
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Start of attribute name
        let start = i;

        // Find '='
        while i < len && bytes[i] != b'=' {
            i += 1;
        }
        if i >= len {
            break;
        }
        i += 1; // skip '='

        // Expect opening quote
        if i >= len {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            break;
        }
        i += 1; // skip opening quote

        // Find closing quote
        while i < len && bytes[i] != quote {
            i += 1;
        }
        if i >= len {
            break;
        }
        i += 1; // skip closing quote

        attrs.push(&s[start..i]);
    }

    attrs
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aem::{
        AemConfig, AemNode, AemOption, ConditionRule, OptionAlignment, TextFieldKind,
    };
    use uuid::Uuid;

    /// Create a test config with minimal templates for testing.
    fn test_config() -> AemConfig {
        let mut config = AemConfig::test_default("TEST");
        config.deterministic_uuids = true;
        // Simple templates for testing — just enough to verify data flow
        config
            .component_templates
            .insert("root".into(), "{{ children }}".into());
        config.component_templates.insert(
            "panel".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ title }}\"{% if not visible %} visible=\"{Boolean}false\"{% endif %}{% if dor_exclude %} dorExclusion=\"true\"{% endif %}{% if dor_num_cols %} dorNumCols=\"{{ dor_num_cols }}\"{% endif %}{% if dor_colspan %} dorColspan=\"{{ dor_colspan }}\"{% endif %}>{{ children }}</{{ element_name }}>".into(),
        );
        config.component_templates.insert(
            "conditional".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ title }}\"{% if not visible and not visibility_triggers %} visible=\"{Boolean}false\"{% endif %}{% if dor_exclude %} dorExclusion=\"true\"{% endif %}{% if dor_num_cols %} dorNumCols=\"{{ dor_num_cols }}\"{% endif %}{% if dor_colspan %} dorColspan=\"{{ dor_colspan }}\"{% endif %}>{{ children }}{% if visibility_triggers %}<fd:scripts fd:visible=\"[{&quot;script&quot;:{&quot;field&quot;:&quot;{{ name }}&quot;\\,&quot;event&quot;:&quot;Visibility&quot;\\,&quot;model&quot;:{&quot;nodeName&quot;:&quot;SHOW_EXPRESSION&quot;}\\,&quot;content&quot;:&quot;if ({% for t in visibility_triggers %}{{ t.field }}.value == \\\\&quot;{{ t.value | escape }}\\\\&quot;{% if not loop.last %} || {% endif %}{% endfor %}) {\\\\n  window.forms.ubs.showAFShowDor(this);\\\\n  true;\\\\n} else {\\\\n  window.forms.ubs.hideAFHideDor(this);\\\\n  false;\\\\n}\\\\n&quot;}\\,&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;\\,&quot;version&quot;:1\\,&quot;enabled&quot;:true}]\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>".into(),
        );
        config.component_templates.insert(
            "textbox".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\"{% if mandatory %} mandatory=\"{Boolean}true\"{% endif %}{% if max_chars %} maxChars=\"{{ max_chars }}\"{% endif %}{% if dor_colspan %} dorColspan=\"{{ dor_colspan }}\"{% endif %}><cq:responsive jcr:primaryType=\"nt:unstructured\"><default jcr:primaryType=\"nt:unstructured\" offset=\"0\" width=\"{{ colspan }}\"/></cq:responsive></{{ element_name }}>".into(),
        );
        config.component_templates.insert(
            "numericbox".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\"/>".into(),
        );
        config.component_templates.insert(
            "datepicker".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\"/>".into(),
        );
        config.component_templates.insert(
            "dropdownlist".into(),
            "<{{ element_name }} guideNodeClass=\"guideDropDownList\" name=\"{{ name }}\" jcr:title=\"{{ label }}\" options=\"{{ options_attr }}\"{% if conditions_script %}>{% if conditions_script %}<fd:scripts fd:valueCommit=\"{{ conditions_script }}\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>{% else %}/>{% endif %}".into(),
        );
        config.component_templates.insert(
            "checkbox".into(),
            "<{{ element_name }} guideNodeClass=\"guideCheckBox\" name=\"{{ name }}\"{% if label %} jcr:title=\"{{ label }}\"{% endif %} options=\"{{ options_attr }}\" alignment=\"{{ alignment }}\"{% if conditions_script %}>{% if conditions_script %}<fd:scripts fd:valueCommit=\"{{ conditions_script }}\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>{% else %}/>{% endif %}".into(),
        );
        config.component_templates.insert(
            "radiobutton".into(),
            "<{{ element_name }} guideNodeClass=\"guideRadioButton\" name=\"{{ name }}\" jcr:title=\"{{ label }}\" options=\"{{ options_attr }}\" alignment=\"{{ alignment }}\"{% if conditions_script %}>{% if conditions_script %}<fd:scripts fd:valueCommit=\"{{ conditions_script }}\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>{% else %}/>{% endif %}".into(),
        );
        config.component_templates.insert(
            "textdraw".into(),
            "<{{ element_name }} guideNodeClass=\"guideTextDraw\" name=\"{{ name }}\" _value=\"{{ content }}\"/>".into(),
        );
        config.component_templates.insert(
            "titledraw".into(),
            "<{{ element_name }} guideNodeClass=\"guideTextDraw\" name=\"{{ name }}\" _value=\"{{ content }}\" headingLevel=\"{{ heading_level }}\"/>".into(),
        );
        config.component_templates.insert(
            "textbox_multiline".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\" multiLine=\"{Boolean}true\"/>".into(),
        );
        config.component_templates.insert(
            "repeatable".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ title }}\" minOccur=\"{{ min_occur }}\" maxOccur=\"{{ max_occur }}\">{{ children }}</{{ element_name }}>".into(),
        );
        config
    }

    fn fixed_uuid() -> Uuid {
        Uuid::new_v5(&Uuid::from_bytes([0; 16]), b"test")
    }

    #[test]
    fn xml_output_renders_textdraw() {
        let root = AemNode::Root {
            title: "Test Form".into(),
            children: vec![AemNode::TextDraw {
                uuid: fixed_uuid(),
                name: "ST_1".into(),
                content: "<p>Hello &amp; world</p>".into(),
                dor_exclude: false,
                colspan: 12,
                dor_colspan: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("guideTextDraw"));
        assert!(xml.contains("ST_1"));
    }

    #[test]
    fn text_field_has_responsive_width() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextField {
                uuid: fixed_uuid(),
                name: "TF_test".into(),
                label: "Test Label".into(),
                mandatory: false,
                visible: true,
                max_chars: Some(100),
                colspan: 6,
                dor_colspan: None,
                bind_ref: None,
                kind: TextFieldKind::Plain,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("cq:responsive"));
        assert!(xml.contains("width=\"6\""));
        assert!(xml.contains("maxChars=\"100\""));
    }

    #[test]
    fn checkbox_options_serialized() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Checkbox {
                uuid: fixed_uuid(),
                name: "CB_test".into(),
                label: String::new(),
                options: vec![
                    AemOption {
                        label: "Yes".into(),
                        value: "1".into(),
                    },
                    AemOption {
                        label: "No".into(),
                        value: "0".into(),
                    },
                ],
                alignment: OptionAlignment::Horizontal,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
                bind_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("options=\"[1=Yes,0=No]\""));
        assert!(xml.contains("alignment=\"horizontal\""));
    }

    #[test]
    fn radio_options_with_commas_are_escaped() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::RadioButton {
                uuid: fixed_uuid(),
                name: "RB_comma".into(),
                label: "Choose".into(),
                options: vec![
                    AemOption {
                        label: "Yes, definitely".into(),
                        value: "1".into(),
                    },
                    AemOption {
                        label: "No, thanks".into(),
                        value: "2".into(),
                    },
                ],
                alignment: OptionAlignment::Vertical,
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
                bind_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        // Commas inside labels must be escaped as \, so the list stays parseable
        assert!(
            xml.contains(r#"options="[1=Yes\, definitely,2=No\, thanks]""#),
            "Commas in option labels must be backslash-escaped. Got:\n{}",
            xml
        );
    }

    #[test]
    fn dropdown_options_with_commas_are_escaped() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Dropdown {
                uuid: fixed_uuid(),
                name: "DD_comma".into(),
                label: "Pick".into(),
                options: vec![
                    AemOption {
                        label: "Option A, first".into(),
                        value: "a".into(),
                    },
                    AemOption {
                        label: "Option B".into(),
                        value: "b".into(),
                    },
                ],
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
                bind_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains(r#"options="[a=Option A\, first,b=Option B]""#),
            "Commas in dropdown labels must be backslash-escaped. Got:\n{}",
            xml
        );
    }

    #[test]
    fn checkbox_options_with_commas_are_escaped() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Checkbox {
                uuid: fixed_uuid(),
                name: "CB_comma".into(),
                label: String::new(),
                options: vec![
                    AemOption {
                        label: "I agree, fully".into(),
                        value: "1".into(),
                    },
                    AemOption {
                        label: "No".into(),
                        value: "0".into(),
                    },
                ],
                alignment: OptionAlignment::Horizontal,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
                bind_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains(r#"options="[1=I agree\, fully,0=No]""#),
            "Commas in checkbox labels must be backslash-escaped. Got:\n{}",
            xml
        );
    }

    #[test]
    fn repeatable_has_min_max_occur() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Repeatable {
                uuid: fixed_uuid(),
                name: "RCP_1".into(),
                title: "Repeat Section".into(),
                children: vec![],
                min_occur: 1,
                max_occur: 10,
                bind_ref: None,
                frag_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("minOccur=\"1\""), "missing minOccur");
        assert!(xml.contains("maxOccur=\"10\""), "missing maxOccur");
        assert!(xml.contains("name=\"RCP_1\""));
    }

    #[test]
    fn dropdown_has_options() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Dropdown {
                uuid: fixed_uuid(),
                name: "DD_test".into(),
                label: "Pick one".into(),
                options: vec![
                    AemOption {
                        label: "A".into(),
                        value: "a".into(),
                    },
                    AemOption {
                        label: "B".into(),
                        value: "b".into(),
                    },
                ],
                mandatory: true,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
                bind_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("guideDropDownList"));
        assert!(xml.contains("options=\"[a=A,b=B]\""));
    }

    #[test]
    fn hidden_panel_emits_visible_false() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Panel {
                uuid: fixed_uuid(),
                name: "COND_Panel".into(),
                title: "Hidden Panel".into(),
                children: vec![],
                is_page: false,
                dor_exclude: true,
                visible: false,
                is_conditional: true,
                dor_num_cols: None,
                colspan: 12,
                dor_colspan: None,
                bind_ref: None,
                frag_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("visible=\"{Boolean}false\""),
            "Hidden panel should have visible={{Boolean}}false. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorExclusion=\"true\""),
            "Hidden panel should have dorExclusion=true. Got:\n{}",
            xml
        );
    }

    /// Build a conditional panel node with the given AEM name (the target of a
    /// trigger field's condition rule).
    fn conditional_panel(name: &str) -> AemNode {
        AemNode::Panel {
            uuid: fixed_uuid(),
            name: name.into(),
            title: String::new(),
            children: vec![],
            is_page: false,
            dor_exclude: true,
            visible: false,
            is_conditional: true,
            dor_num_cols: None,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
            frag_ref: None,
        }
    }

    #[test]
    fn radio_button_with_conditions_emits_panel_visible_script() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![
                AemNode::RadioButton {
                    uuid: fixed_uuid(),
                    name: "RB_TriggerField".into(),
                    label: "Choose".into(),
                    options: vec![
                        AemOption {
                            label: "Yes".into(),
                            value: "yes".into(),
                        },
                        AemOption {
                            label: "No".into(),
                            value: "no".into(),
                        },
                    ],
                    alignment: OptionAlignment::Vertical,
                    mandatory: false,
                    visible: true,
                    colspan: 12,
                    dor_colspan: None,
                    field_id: None,
                    conditions: vec![ConditionRule {
                        target_panel_name: "COND_TargetPanel".into(),
                        value: InputValue::Text("yes".into()),
                        show: true,
                    }],
                    bind_ref: None,
                },
                conditional_panel("COND_TargetPanel"),
            ],
        };
        let xml = generate_aem_xml(&root, &test_config());
        // AABO mechanism: the SHOW_EXPRESSION lives on the target panel, not as
        // a valueCommit on the trigger field.
        assert!(
            !xml.contains("fd:valueCommit"),
            "Trigger field should NOT emit fd:valueCommit anymore. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("fd:visible") && xml.contains("SHOW_EXPRESSION"),
            "Conditional panel should emit a fd:visible SHOW_EXPRESSION. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("showAFShowDor") && xml.contains("hideAFHideDor"),
            "Script should call the UBS DOR helpers. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("RB_TriggerField.value =="),
            "Expression should reference the trigger field by name. Got:\n{}",
            xml
        );
    }

    #[test]
    fn radio_without_conditions_has_no_scripts() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::RadioButton {
                uuid: fixed_uuid(),
                name: "RB_Simple".into(),
                label: "Choose".into(),
                options: vec![AemOption {
                    label: "A".into(),
                    value: "a".into(),
                }],
                alignment: OptionAlignment::Vertical,
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
                bind_ref: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            !xml.contains("fd:scripts"),
            "Radio without conditions should NOT emit fd:scripts. Got:\n{}",
            xml
        );
    }

    #[test]
    fn dropdown_with_conditions_emits_scripts() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Dropdown {
                uuid: fixed_uuid(),
                name: "DD_Trigger".into(),
                label: "Select".into(),
                options: vec![
                    AemOption {
                        label: "Option A".into(),
                        value: "a".into(),
                    },
                    AemOption {
                        label: "Option B".into(),
                        value: "b".into(),
                    },
                ],
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![ConditionRule {
                    target_panel_name: "COND_PanelA".into(),
                    value: InputValue::Text("a".into()),
                    show: true,
                }],
                bind_ref: None,
            }, conditional_panel("COND_PanelA")],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("fd:visible") && xml.contains("SHOW_EXPRESSION"),
            "Conditional panel for a dropdown trigger should emit fd:visible. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("DD_Trigger.value =="),
            "Expression should reference DD_Trigger. Got:\n{}",
            xml
        );
    }

    #[test]
    fn checkbox_with_conditions_emits_scripts() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Checkbox {
                uuid: fixed_uuid(),
                name: "CB_Trigger".into(),
                label: String::new(),
                options: vec![AemOption {
                    label: "Accept".into(),
                    value: "true".into(),
                }],
                alignment: OptionAlignment::Horizontal,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![ConditionRule {
                    target_panel_name: "COND_AcceptPanel".into(),
                    value: InputValue::Bool(true),
                    show: true,
                }],
                bind_ref: None,
            }, conditional_panel("COND_AcceptPanel")],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("fd:visible") && xml.contains("SHOW_EXPRESSION"),
            "Conditional panel for a checkbox trigger should emit fd:visible. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("CB_Trigger.value == \\\\&quot;true\\\\&quot;"),
            "Expression should compare CB_Trigger to the bool value. Got:\n{}",
            xml
        );
    }

    #[test]
    fn visible_script_ors_multiple_trigger_values() {
        use crate::structured::InputValue;

        // A trigger that shows the same panel for two different values
        // (AABO: `RB_FormularAdressat.value == "3" || ... == "4"`).
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![
                AemNode::RadioButton {
                    uuid: fixed_uuid(),
                    name: "RB_Adressat".into(),
                    label: "Choose".into(),
                    options: vec![
                        AemOption {
                            label: "Three".into(),
                            value: "3".into(),
                        },
                        AemOption {
                            label: "Four".into(),
                            value: "4".into(),
                        },
                    ],
                    alignment: OptionAlignment::Vertical,
                    mandatory: false,
                    visible: true,
                    colspan: 12,
                    dor_colspan: None,
                    field_id: None,
                    conditions: vec![
                        ConditionRule {
                            target_panel_name: "COND_Entity".into(),
                            value: InputValue::Text("3".into()),
                            show: true,
                        },
                        ConditionRule {
                            target_panel_name: "COND_Entity".into(),
                            value: InputValue::Text("4".into()),
                            show: true,
                        },
                    ],
                    bind_ref: None,
                },
                conditional_panel("COND_Entity"),
            ],
        };
        let xml = generate_aem_xml(&root, &test_config());
        // The rendered expression OR-s both comparisons (AABO escaping `\\&quot;`).
        assert!(
            xml.contains(
                "RB_Adressat.value == \\\\&quot;3\\\\&quot; || RB_Adressat.value == \\\\&quot;4\\\\&quot;"
            ),
            "Expression should OR both trigger values. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("window.forms.ubs.showAFShowDor(this);")
                && xml.contains("window.forms.ubs.hideAFHideDor(this);"),
            "Script should call both DOR helpers. Got:\n{}",
            xml
        );
    }

    #[test]
    fn visible_json_has_show_expression_structure() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![
                AemNode::Dropdown {
                    uuid: fixed_uuid(),
                    name: "DD_Field".into(),
                    label: "Select".into(),
                    options: vec![AemOption {
                        label: "Val".into(),
                        value: "val".into(),
                    }],
                    mandatory: false,
                    visible: true,
                    colspan: 12,
                    dor_colspan: None,
                    field_id: None,
                    conditions: vec![ConditionRule {
                        target_panel_name: "PN_Cond".into(),
                        value: InputValue::Text("val".into()),
                        show: true,
                    }],
                    bind_ref: None,
                },
                conditional_panel("PN_Cond"),
            ],
        };
        let xml = generate_aem_xml(&root, &test_config());
        // The SHOW_EXPRESSION SCRIPTMODEL envelope, rendered entirely by the template.
        assert!(
            xml.contains("&quot;field&quot;:&quot;PN_Cond&quot;"),
            "field should be the target panel name. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("&quot;event&quot;:&quot;Visibility&quot;"),
            "event should be Visibility. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("&quot;nodeName&quot;:&quot;SHOW_EXPRESSION&quot;"),
            "model nodeName should be SHOW_EXPRESSION. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("SCRIPTMODEL"),
            "envelope should contain SCRIPTMODEL. Got:\n{}",
            xml
        );
    }

    #[test]
    fn dor_colspan_emitted_on_fields_in_grid_panel() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Panel {
                uuid: fixed_uuid(),
                name: "GridPanel".into(),
                title: "Grid Panel".into(),
                is_page: false,
                dor_exclude: false,
                visible: true,
                is_conditional: false,
                dor_num_cols: Some(3),
                colspan: 12,
                dor_colspan: None,
                bind_ref: None,
                frag_ref: None,
                children: vec![
                    AemNode::TextField {
                        uuid: fixed_uuid(),
                        name: "Street".into(),
                        label: "Street".into(),
                        mandatory: false,
                        visible: true,
                        max_chars: None,
                        colspan: 8,
                        dor_colspan: Some(2),
                        bind_ref: None,
                        kind: TextFieldKind::Plain,
                    },
                    AemNode::TextField {
                        uuid: fixed_uuid(),
                        name: "No".into(),
                        label: "No".into(),
                        mandatory: false,
                        visible: true,
                        max_chars: None,
                        colspan: 4,
                        dor_colspan: Some(1),
                        bind_ref: None,
                        kind: TextFieldKind::Plain,
                    },
                ],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("dorNumCols=\"3\""),
            "Panel should have dorNumCols=3. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorColspan=\"2\""),
            "Street field should have dorColspan=2. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorColspan=\"1\""),
            "No field should have dorColspan=1. Got:\n{}",
            xml
        );
    }

    #[test]
    fn dor_colspan_not_emitted_when_none() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextField {
                uuid: fixed_uuid(),
                name: "PlainField".into(),
                label: "Plain".into(),
                mandatory: false,
                visible: true,
                max_chars: None,
                colspan: 12,
                dor_colspan: None,
                bind_ref: None,
                kind: TextFieldKind::Plain,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            !xml.contains("dorColspan"),
            "Field without dor_colspan should not emit dorColspan. Got:\n{}",
            xml
        );
    }

    #[test]
    fn missing_template_omits_component() {
        let mut config = test_config();
        config.component_templates.remove("textdraw");

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextDraw {
                uuid: fixed_uuid(),
                name: "ST_1".into(),
                content: "Hello".into(),
                dor_exclude: false,
                colspan: 12,
                dor_colspan: None,
            }],
        };
        let xml = generate_aem_xml(&root, &config);
        assert!(
            !xml.contains("ST_1"),
            "Component with missing template should be omitted. Got:\n{}",
            xml
        );
    }

    /// Verify the preface template renders a fragment panel with `fragRef`
    /// always pointing to affrg_BankingRelationship1.
    #[test]
    fn preface_renders_entity_based_banking_relationship_fragment() {
        let preface_template = include_str!("../../../profiles/ubs/aem/preface.xml");

        let expected_frag_ref =
            "/content/forms/af/afforms_ubs_fragmentlib/affrg_BankingRelationship1";

        for entity in &["033", "019", "001"] {
            let mut config = test_config();
            config
                .component_templates
                .insert("preface".into(), preface_template.into());
            config
                .xfa_vars
                .insert("formrange_entity".into(), entity.to_string());
            config.user_vars.insert(
                "default_layout".into(),
                "fd/af/layouts/gridFluidLayout2".into(),
            );
            config
                .user_vars
                .insert("dor_field_styling".into(), "Default".into());
            config.user_vars.insert(
                "custom_resource_type_base".into(),
                "ajila-forms-customers/ajila-forms-ubs/components".into(),
            );

            let node = AemNode::Preface {
                uuid: fixed_uuid(),
                name: "PN_Preface_abcdef01".into(),
            };

            let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

            assert!(
                xml.contains(&format!("fragRef=\"{}\"", expected_frag_ref)),
                "entity {}: expected fragRef='{}' in:\n{}",
                entity,
                expected_frag_ref,
                xml
            );
            assert!(
                xml.contains("name=\"PN_BankingRelationship\""),
                "entity {}: expected name='PN_BankingRelationship' in:\n{}",
                entity,
                xml
            );
        }
    }

    /// The banking-relationship preface fragment must be wrapped in a `PN_BR`
    /// panel that is excluded from both the Document of Record and the Summary.
    /// This mirrors the AAJC reference form; the fragment panel inside carries
    /// its own `dorExclusion` as well (see
    /// [`preface_fragment_panel_is_dor_excluded`]).
    #[test]
    fn preface_wraps_banking_relationship_in_excluded_panel() {
        let mut config = test_config();
        config.component_templates.insert(
            "preface".into(),
            include_str!("../../../profiles/ubs/aem/preface.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ajila-forms-customers/ajila-forms-ubs/components".into(),
        );

        let node = AemNode::Preface {
            uuid: fixed_uuid(),
            name: "PN_Preface_abcdef01".into(),
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        // The wrapper panel exists and carries both exclusions.
        assert!(
            xml.contains("name=\"PN_BR\""),
            "expected a wrapping PN_BR panel. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorExclusion=\"true\""),
            "PN_BR wrapper must be DOR-excluded. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("summaryExclusion=\"true\""),
            "PN_BR wrapper must be summary-excluded. Got:\n{}",
            xml
        );
        // The inner fragment panel is nested inside the wrapper.
        let pn_br = xml.find("name=\"PN_BR\"").unwrap();
        let pn_banking = xml.find("name=\"PN_BankingRelationship\"").unwrap();
        assert!(
            pn_br < pn_banking,
            "PN_BankingRelationship must be nested inside PN_BR. Got:\n{}",
            xml
        );
    }

    /// The panel that *bears* the banking fragment must itself carry
    /// `dorExclusion="true"`, not only the `PN_BR` wrapper around it.
    ///
    /// The wrapper alone is what the engine used to emit, and it is not what the
    /// deployed corpus has: every form there carries the exclusion on the
    /// fragment panel too, and the feedback guard reads that node and no other
    /// (PROBLEM-banking-relationship-fragment, `find_panel_noncanonical.py
    /// --require-attr dorExclusion=true`). The assertion is deliberately scoped
    /// to the one tag: an exclusion anywhere else in the XML does not satisfy
    /// the rule, which is exactly how the gap went unnoticed.
    #[test]
    fn preface_fragment_panel_is_dor_excluded() {
        let mut config = test_config();
        config.component_templates.insert(
            "preface".into(),
            include_str!("../../../profiles/ubs/aem/preface.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ajila-forms-customers/ajila-forms-ubs/components".into(),
        );

        let node = AemNode::Preface {
            uuid: fixed_uuid(),
            name: "PN_Preface_abcdef01".into(),
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        let frag_at = xml
            .find("affrg_BankingRelationship1")
            .expect("the preface must emit the banking fragment");
        let tag_start = xml[..frag_at].rfind('<').expect("fragRef inside a tag");
        let tag_end = tag_start
            + xml[tag_start..]
                .find('>')
                .expect("the fragment panel tag must close");
        let tag = &xml[tag_start..tag_end];
        assert!(
            tag.contains("dorExclusion=\"true\""),
            "the fragment panel tag must carry dorExclusion. Got:\n{}",
            tag
        );
    }

    /// The `PN_BR` wrapper must carry `css="ubs-margin-20"` so the banking
    /// block gets the standard vertical spacing (feedback registry
    /// PROBLEM-banking-relationship-margin, UBS directive 2026-07-27).
    #[test]
    fn preface_wrapper_carries_ubs_margin_class() {
        let mut config = test_config();
        config.component_templates.insert(
            "preface".into(),
            include_str!("../../../profiles/ubs/aem/preface.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ajila-forms-customers/ajila-forms-ubs/components".into(),
        );

        let node = AemNode::Preface {
            uuid: fixed_uuid(),
            name: "PN_Preface_abcdef01".into(),
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        let pn_br = xml.find("name=\"PN_BR\"").expect("PN_BR wrapper missing");
        let css = xml
            .find("css=\"ubs-margin-20\"")
            .unwrap_or_else(|| panic!("PN_BR wrapper must carry ubs-margin-20. Got:\n{}", xml));
        assert!(
            css < pn_br,
            "the margin class must sit on the PN_BR wrapper. Got:\n{}",
            xml
        );
    }

    /// Date pickers must not pre-fill today's date: a pre-filled current date
    /// silently becomes wrong data on any form not submitted the same day
    /// (feedback registry PROBLEM-datepicker-current-date-default). The
    /// unchecked state is the attribute being ABSENT, not `false`.
    #[test]
    fn datepicker_does_not_default_to_current_date() {
        let mut config = test_config();
        config.component_templates.insert(
            "datepicker".into(),
            include_str!("../../../profiles/ubs/aem/datepicker.xml").into(),
        );
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ajila-forms-customers/ajila-forms-ubs/components".into(),
        );
        config
            .user_vars
            .insert("css_datepicker".into(), "widget_datepicker".into());
        config
            .user_vars
            .insert("dor_field_styling".into(), "Default".into());

        let node = AemNode::DatePicker {
            uuid: fixed_uuid(),
            name: "DATE_1".into(),
            label: "Date".into(),
            mandatory: false,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        assert!(
            xml.contains("guideNodeClass=\"guideDatePicker\""),
            "expected the real datepicker template to render. Got:\n{}",
            xml
        );
        assert!(
            !xml.contains("defaultToCurrentDate"),
            "date pickers must not carry defaultToCurrentDate. Got:\n{}",
            xml
        );
    }

    /// For a `PN_FormConfigurator` page panel, the full `dorExclusion` must sit
    /// on the generated `…Title` sub-panel, NOT on the parent panel (the parent
    /// only carries `dorExcludeTitle`/`dorExcludeDescription`). This mirrors the
    /// reference packages; the previous template wrongly excluded the whole
    /// configurator subtree from the DOR.
    #[test]
    fn form_configurator_excludes_title_subpanel_not_parent_from_dor() {
        let mut config = test_config();
        config.component_templates.insert(
            "panel".into(),
            include_str!("../../../profiles/ubs/aem/panel.xml").into(),
        );
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ajila-forms-customers/ajila-forms-ubs/components".into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config
            .user_vars
            .insert("dor_field_styling".into(), "default".into());

        let node = AemNode::Panel {
            uuid: fixed_uuid(),
            name: "PN_FormConfigurator_abcdef01".into(),
            title: "Form configurator".into(),
            children: vec![],
            is_page: true,
            dor_exclude: false,
            visible: true,
            is_conditional: false,
            dor_num_cols: None,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
            frag_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        // The parent FormConfigurator panel element (up to the first `>`) must
        // NOT carry dorExclusion; it keeps only dorExcludeTitle/Description.
        let parent_tag = &xml[..xml.find('>').expect("panel open tag")];
        assert!(
            !parent_tag.contains("dorExclusion=\"true\""),
            "FormConfigurator parent panel must NOT be DOR-excluded. Got tag:\n{}",
            parent_tag
        );
        assert!(
            parent_tag.contains("dorExcludeTitle=\"true\""),
            "FormConfigurator parent panel should keep dorExcludeTitle. Got tag:\n{}",
            parent_tag
        );
        // The generated Title sub-panel must carry the full dorExclusion.
        assert!(
            xml.contains("name=\"PN_FormConfigurator_abcdef01Title\""),
            "expected generated Title sub-panel. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorExclusion=\"true\""),
            "FormConfigurator Title sub-panel must be DOR-excluded. Got:\n{}",
            xml
        );
    }

    /// Every panel inside a repeatable must keep the repeatable's own name as its
    /// stem, so the `RCP_` prefix the naming convention requires survives.
    ///
    /// The innermost row panel used to be named from the repeatable's *title*.
    /// Engine-authored repeatables carry no title, so the name came out as the
    /// bare suffix `_inner` — no prefix at all — and a title that did exist
    /// dragged its spaces into a component name (`Portfolio ID_inner` in the
    /// corpus). Both are violations of PROBLEM-naming-conventions, and the title
    /// was never the right source: a name is not display text.
    #[test]
    fn repeatable_inner_panels_keep_the_repeatable_prefix() {
        let mut config = test_config();
        config.component_templates.insert(
            "repeatable".into(),
            include_str!("../../../profiles/ubs/aem/repeatable.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ubs/af/components".into(),
        );
        config
            .user_vars
            .insert("dor_field_styling".into(), "some_styling".into());

        // Titleless, as an engine-authored repeatable is.
        let node = AemNode::Repeatable {
            uuid: fixed_uuid(),
            name: "RCP_Clients".into(),
            title: String::new(),
            children: vec![],
            min_occur: 1,
            max_occur: 5,
            bind_ref: None,
            frag_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        assert!(
            xml.contains("name=\"RCP_Clients_inner\""),
            "the row panel must be named after the repeatable. Got:\n{}",
            xml
        );
        assert!(
            !xml.contains("name=\"_inner\""),
            "a titleless repeatable must not produce a prefixless name. Got:\n{}",
            xml
        );
        // Every name the repeatable emits for itself stays under one prefix.
        for name in ["RCP_Clients", "RCP_Clients_repeat", "RCP_Clients_inner"] {
            assert!(
                xml.contains(&format!("name=\"{name}\"")),
                "expected name={name} in:\n{}",
                xml
            );
        }
    }

    /// The remove-button click script must restore BT_Add.visible on the last
    /// instance whenever the count drops back below max_occur.
    ///
    /// Regression test for: deleting an instance after reaching max left the
    /// add button permanently hidden.
    #[test]
    fn remove_button_script_restores_add_button_when_below_max() {
        let mut config = test_config();
        config.component_templates.insert(
            "repeatable".into(),
            include_str!("../../../profiles/ubs/aem/repeatable.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config
            .user_vars
            .insert("resource_type_base".into(), "fd/af/components".into());
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ubs/af/components".into(),
        );
        config
            .user_vars
            .insert("dor_field_styling".into(), "some_styling".into());

        let node = AemNode::Repeatable {
            uuid: fixed_uuid(),
            name: "Test".into(),
            title: "Repeat Section".into(),
            children: vec![],
            min_occur: 1,
            max_occur: 5,
            bind_ref: None,
            frag_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        assert!(
            xml.contains("BT_Add.visible = true"),
            "Remove script must restore BT_Add.visible when below max. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("len &lt; 5"),
            "Remove script must check len < max_occur. Got:\n{}",
            xml
        );
    }

    #[test]
    fn repeatable_remove_click_script_exact_output() {
        let mut config = test_config();
        config.component_templates.insert(
            "repeatable".into(),
            include_str!("../../../profiles/ubs/aem/repeatable.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config
            .user_vars
            .insert("resource_type_base".into(), "fd/af/components".into());
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ubs/af/components".into(),
        );
        config
            .user_vars
            .insert("dor_field_styling".into(), "some_styling".into());

        let node = AemNode::Repeatable {
            uuid: fixed_uuid(),
            name: "Test".into(),
            title: "Repeat Section".into(),
            children: vec![],
            min_occur: 1,
            max_occur: 5,
            bind_ref: None,
            frag_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        let expected_remove_click = concat!(
            "[{&quot;script&quot;:{&quot;content&quot;:&quot;",
            "Test_repeat.instanceManager.removeInstance(this.parent.index);",
            "\\var len = Test_repeat.instanceManager.instances.length;",
            "\\for (var i = 0; i &lt; len; i++) {",
            "\\Test_repeat.instanceManager.instances[i].BT_Remove.visible = (i === (len - 1) &amp;&amp; len &gt; 1) ? true : false;",
            "\\}",
            "\\if (len &lt; 5) {",
            "\\Test.BT_Add.visible = true;",
            "\\}",
            "&quot;\\,&quot;event&quot;:&quot;Click&quot;\\,&quot;field&quot;:&quot;BT_Remove&quot;}",
            "\\,&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;\\,&quot;version&quot;:1\\,&quot;enabled&quot;:true}]",
        );
        assert!(
            xml.contains(expected_remove_click),
            "remove_click script mismatch.\nExpected to find:\n{}\n\nIn:\n{}",
            expected_remove_click,
            xml
        );
    }

    #[test]
    fn repeatable_add_click_script_exact_output() {
        let mut config = test_config();
        config.component_templates.insert(
            "repeatable".into(),
            include_str!("../../../profiles/ubs/aem/repeatable.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config
            .user_vars
            .insert("resource_type_base".into(), "fd/af/components".into());
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ubs/af/components".into(),
        );
        config
            .user_vars
            .insert("dor_field_styling".into(), "some_styling".into());

        let node = AemNode::Repeatable {
            uuid: fixed_uuid(),
            name: "Test".into(),
            title: "Repeat Section".into(),
            children: vec![],
            min_occur: 1,
            max_occur: 5,
            bind_ref: None,
            frag_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        let expected_add_click = concat!(
            "[{&quot;script&quot;:{&quot;content&quot;:&quot;",
            "Test_repeat.instanceManager.addInstance();",
            "\\var len = Test_repeat.instanceManager.instances.length;",
            "\\for (var i = 0; i &lt; len; i++) {",
            "\\Test_repeat.instanceManager.instances[i].BT_Remove.visible = (i === (len - 1) &amp;&amp; len &gt; 1) ? true : false;",
            "\\}",
            "\\if (len &gt;= 5) {",
            "\\this.visible = false;",
            "\\}",
            "&quot;\\,&quot;event&quot;:&quot;Click&quot;\\,&quot;field&quot;:&quot;BT_Add&quot;}",
            "\\,&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;\\,&quot;version&quot;:1\\,&quot;enabled&quot;:true}]",
        );
        assert!(
            xml.contains(expected_add_click),
            "add_click script mismatch.\nExpected to find:\n{}\n\nIn:\n{}",
            expected_add_click,
            xml
        );
    }

    #[test]
    fn repeatable_add_init_script_exact_output() {
        let mut config = test_config();
        config.component_templates.insert(
            "repeatable".into(),
            include_str!("../../../profiles/ubs/aem/repeatable.xml").into(),
        );
        config.user_vars.insert(
            "default_layout".into(),
            "fd/af/layouts/gridFluidLayout2".into(),
        );
        config
            .user_vars
            .insert("resource_type_base".into(), "fd/af/components".into());
        config.user_vars.insert(
            "custom_resource_type_base".into(),
            "ubs/af/components".into(),
        );
        config
            .user_vars
            .insert("dor_field_styling".into(), "some_styling".into());

        let node = AemNode::Repeatable {
            uuid: fixed_uuid(),
            name: "Test".into(),
            title: "Repeat Section".into(),
            children: vec![],
            min_occur: 1,
            max_occur: 5,
            bind_ref: None,
            frag_ref: None,
        };
        let xml = render_node(&node, &config, &RenderIndex::build(&node), no_passthrough());

        let expected_add_init = concat!(
            "[{&quot;script&quot;:{&quot;content&quot;:&quot;",
            "var len = Test_repeat.instanceManager.instances.length;",
            "\\for (var i = 0; i &lt; len; i++) {",
            "\\Test_repeat.instanceManager.instances[i].BT_Remove.visible = (i === (len - 1) &amp;&amp; len &gt; 1) ? true : false;",
            "\\}",
            "\\if (len &gt;= 5) {",
            "\\this.visible = false;",
            "\\}",
            "&quot;\\,&quot;event&quot;:&quot;Initialize&quot;\\,&quot;field&quot;:&quot;BT_Add&quot;}",
            "\\,&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;\\,&quot;version&quot;:1\\,&quot;enabled&quot;:true}]",
        );
        assert!(
            xml.contains(expected_add_init),
            "add_init script mismatch.\nExpected to find:\n{}\n\nIn:\n{}",
            expected_add_init,
            xml
        );
    }
}
