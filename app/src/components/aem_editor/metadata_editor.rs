//! Properties editor for a single AEM node.
//!
//! Renders only the controls relevant to the node's variant and dispatches an
//! [`AemEditorAction::UpdateMetadata`] for each change.

use dioxus::prelude::*;

use blueprint::{AemNode, AemOption, ConditionRule, InputValue, OptionAlignment};

use super::node_renderer::AemNodeWrapper;
use super::state::{AemEditorAction, AemMetadata, AemPath};

#[derive(Clone, PartialEq, Props)]
pub struct AemMetadataEditorProps {
    pub node: AemNodeWrapper,
    pub path: AemPath,
    pub on_action: EventHandler<AemEditorAction>,
}

#[component]
pub fn AemMetadataEditor(props: AemMetadataEditorProps) -> Element {
    let node = props.node.0.clone();
    let path = props.path.clone();
    let on_action = props.on_action;

    let emit = move |meta: AemMetadata| {
        on_action.call(AemEditorAction::UpdateMetadata {
            path: path.clone(),
            metadata: meta,
        });
    };

    // Snapshot current values for control defaults.
    let cur_name = super::state::node_name(&node).unwrap_or_default();
    let cur_options = super::state::node_options(&node);
    let cur_conditions = super::state::node_conditions(&node);

    rsx! {
        div { class: "aem-metadata-editor",
            // Name (all non-root nodes carry a name)
            if !matches!(node, AemNode::Root { .. }) {
                div { class: "aem-meta-row",
                    label { "Name" }
                    input {
                        r#type: "text",
                        value: "{cur_name}",
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| emit(AemMetadata::Name(evt.value()))
                        },
                    }
                }
            }

            // Visible
            if let Some(v) = visible_of(&node) {
                div { class: "aem-meta-row",
                    label { "Visible" }
                    input {
                        r#type: "checkbox",
                        checked: v,
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| emit(AemMetadata::Visible(evt.checked()))
                        },
                    }
                }
            }

            // Mandatory
            if let Some(v) = mandatory_of(&node) {
                div { class: "aem-meta-row",
                    label { "Mandatory" }
                    input {
                        r#type: "checkbox",
                        checked: v,
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| emit(AemMetadata::Mandatory(evt.checked()))
                        },
                    }
                }
            }

            // Page panel
            if let AemNode::Panel { is_page, .. } = node {
                div { class: "aem-meta-row",
                    label { "Page / wizard step" }
                    input {
                        r#type: "checkbox",
                        checked: is_page,
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| emit(AemMetadata::IsPage(evt.checked()))
                        },
                    }
                }
            }

            // DOR exclude
            if let Some(v) = dor_exclude_of(&node) {
                div { class: "aem-meta-row",
                    label { "Exclude from DOR" }
                    input {
                        r#type: "checkbox",
                        checked: v,
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| emit(AemMetadata::DorExclude(evt.checked()))
                        },
                    }
                }
            }

            // Colspan
            if let Some(v) = colspan_of(&node) {
                div { class: "aem-meta-row",
                    label { "Column span (1–12)" }
                    input {
                        r#type: "number",
                        min: "1",
                        max: "12",
                        value: "{v}",
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| {
                                if let Ok(n) = evt.value().parse::<u32>() {
                                    emit(AemMetadata::Colspan(n.clamp(1, 12)));
                                }
                            }
                        },
                    }
                }
            }

            // Max chars (TextField)
            if let AemNode::TextField { max_chars, .. } = node {
                div { class: "aem-meta-row",
                    label { "Max characters (blank = none)" }
                    input {
                        r#type: "number",
                        min: "0",
                        value: max_chars.map(|c| c.to_string()).unwrap_or_default(),
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| {
                                let v = evt.value();
                                let parsed = if v.trim().is_empty() { None } else { v.parse::<usize>().ok() };
                                emit(AemMetadata::MaxChars(parsed));
                            }
                        },
                    }
                }
            }

            // Heading level (TitleDraw)
            if let AemNode::TitleDraw { heading_level, .. } = node {
                div { class: "aem-meta-row",
                    label { "Heading level (1–6)" }
                    input {
                        r#type: "number",
                        min: "1",
                        max: "6",
                        value: "{heading_level}",
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| {
                                if let Ok(n) = evt.value().parse::<u8>() {
                                    emit(AemMetadata::HeadingLevel(n.clamp(1, 6)));
                                }
                            }
                        },
                    }
                }
            }

            // Repeatable occurrences
            if let AemNode::Repeatable { min_occur, max_occur, .. } = node {
                div { class: "aem-meta-row",
                    label { "Occurrences (min / max)" }
                    input {
                        r#type: "number",
                        min: "0",
                        value: "{min_occur}",
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| {
                                if let Ok(n) = evt.value().parse::<u32>() {
                                    emit(AemMetadata::Occurrences { min: n, max: max_occur });
                                }
                            }
                        },
                    }
                    input {
                        r#type: "number",
                        min: "1",
                        value: "{max_occur}",
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| {
                                if let Ok(n) = evt.value().parse::<u32>() {
                                    emit(AemMetadata::Occurrences { min: min_occur, max: n });
                                }
                            }
                        },
                    }
                }
            }

            // Alignment (Checkbox / RadioButton)
            if let Some(a) = alignment_of(&node) {
                div { class: "aem-meta-row",
                    label { "Option alignment" }
                    select {
                        value: if matches!(a, OptionAlignment::Horizontal) { "horizontal" } else { "vertical" },
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| {
                                let al = if evt.value() == "horizontal" { OptionAlignment::Horizontal } else { OptionAlignment::Vertical };
                                emit(AemMetadata::Alignment(al));
                            }
                        },
                        option { value: "vertical", "Vertical" }
                        option { value: "horizontal", "Horizontal" }
                    }
                }
            }

            // Fragment reference
            if let AemNode::Fragment { frag_ref, .. } = node {
                div { class: "aem-meta-row",
                    label { "Fragment ref" }
                    input {
                        r#type: "text",
                        value: "{frag_ref}",
                        onchange: {
                            let emit = emit.clone();
                            move |evt: Event<FormData>| emit(AemMetadata::FragRef(evt.value()))
                        },
                    }
                }
            }

            // Options (Dropdown / Checkbox / Radio / Custom)
            if let Some(options) = cur_options {
                AemOptionsEditor { options, on_change: { let emit = emit.clone(); move |opts| emit(AemMetadata::Options(opts)) } }
            }

            // Visibility conditions (Dropdown / Checkbox / Radio)
            if let Some(conditions) = cur_conditions {
                AemConditionsEditor { conditions, on_change: { let emit = emit.clone(); move |c| emit(AemMetadata::Conditions(c)) } }
            }

            div { class: "aem-metadata-actions",
                button {
                    class: "aem-text-editor-btn",
                    onclick: move |_| on_action.call(AemEditorAction::StopEditing),
                    "Done"
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Props)]
struct AemOptionsEditorProps {
    options: Vec<AemOption>,
    on_change: EventHandler<Vec<AemOption>>,
}

#[component]
fn AemOptionsEditor(props: AemOptionsEditorProps) -> Element {
    let mut rows = use_signal(|| props.options.clone());
    let on_change = props.on_change;

    rsx! {
        div { class: "aem-options-editor",
            label { class: "aem-meta-row", "Options" }
            for (i, opt) in rows.read().clone().into_iter().enumerate() {
                div { class: "aem-option-row", key: "{i}",
                    input {
                        r#type: "text",
                        placeholder: "label",
                        value: "{opt.label}",
                        onchange: move |evt: Event<FormData>| {
                            let mut v = rows.read().clone();
                            if let Some(o) = v.get_mut(i) { o.label = evt.value(); }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                    }
                    input {
                        r#type: "text",
                        placeholder: "value",
                        value: "{opt.value}",
                        onchange: move |evt: Event<FormData>| {
                            let mut v = rows.read().clone();
                            if let Some(o) = v.get_mut(i) { o.value = evt.value(); }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                    }
                    button {
                        class: "aem-node-btn",
                        onclick: move |_| {
                            let mut v = rows.read().clone();
                            if i < v.len() { v.remove(i); }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                        "✕"
                    }
                }
            }
            button {
                class: "aem-text-editor-btn",
                onclick: move |_| {
                    let mut v = rows.read().clone();
                    v.push(AemOption { label: String::new(), value: String::new() });
                    rows.set(v.clone());
                    on_change.call(v);
                },
                "+ Add option"
            }
        }
    }
}

fn input_value_to_string(v: &InputValue) -> String {
    match v {
        InputValue::Text(s) => s.clone(),
        InputValue::Number(d) => d.to_string(),
        InputValue::Bool(b) => b.to_string(),
    }
}

#[derive(Clone, PartialEq, Props)]
struct AemConditionsEditorProps {
    conditions: Vec<ConditionRule>,
    on_change: EventHandler<Vec<ConditionRule>>,
}

#[component]
fn AemConditionsEditor(props: AemConditionsEditorProps) -> Element {
    let mut rows = use_signal(|| props.conditions.clone());
    let on_change = props.on_change;

    rsx! {
        div { class: "aem-conditions-editor",
            label { class: "aem-meta-row", "Visibility conditions" }
            p { class: "aem-meta-hint", "When this field equals the value, show/hide the target panel (by AEM name)." }
            for (i, rule) in rows.read().clone().into_iter().enumerate() {
                div { class: "aem-option-row", key: "c{i}",
                    input {
                        r#type: "text",
                        placeholder: "when value equals…",
                        value: "{input_value_to_string(&rule.value)}",
                        onchange: move |evt: Event<FormData>| {
                            let mut v = rows.read().clone();
                            if let Some(r) = v.get_mut(i) { r.value = InputValue::Text(evt.value()); }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                    }
                    input {
                        r#type: "text",
                        placeholder: "target panel name",
                        value: "{rule.target_panel_name}",
                        onchange: move |evt: Event<FormData>| {
                            let mut v = rows.read().clone();
                            if let Some(r) = v.get_mut(i) { r.target_panel_name = evt.value(); }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                    }
                    select {
                        value: if rule.show { "show" } else { "hide" },
                        onchange: move |evt: Event<FormData>| {
                            let mut v = rows.read().clone();
                            if let Some(r) = v.get_mut(i) { r.show = evt.value() == "show"; }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                        option { value: "show", "show" }
                        option { value: "hide", "hide" }
                    }
                    button {
                        class: "aem-node-btn",
                        onclick: move |_| {
                            let mut v = rows.read().clone();
                            if i < v.len() { v.remove(i); }
                            rows.set(v.clone());
                            on_change.call(v);
                        },
                        "✕"
                    }
                }
            }
            button {
                class: "aem-text-editor-btn",
                onclick: move |_| {
                    let mut v = rows.read().clone();
                    v.push(ConditionRule { target_panel_name: String::new(), value: InputValue::Text(String::new()), show: true });
                    rows.set(v.clone());
                    on_change.call(v);
                },
                "+ Add condition"
            }
        }
    }
}

// ── Field readers ─────────────────────────────────────────────────────────

fn visible_of(node: &AemNode) -> Option<bool> {
    use AemNode::*;
    match node {
        Panel { visible, .. }
        | TextField { visible, .. }
        | NumberField { visible, .. }
        | DatePicker { visible, .. }
        | Dropdown { visible, .. }
        | Checkbox { visible, .. }
        | RadioButton { visible, .. }
        | Custom { visible, .. } => Some(*visible),
        _ => None,
    }
}

fn mandatory_of(node: &AemNode) -> Option<bool> {
    use AemNode::*;
    match node {
        TextField { mandatory, .. }
        | NumberField { mandatory, .. }
        | DatePicker { mandatory, .. }
        | Dropdown { mandatory, .. }
        | RadioButton { mandatory, .. }
        | Custom { mandatory, .. } => Some(*mandatory),
        _ => None,
    }
}

fn dor_exclude_of(node: &AemNode) -> Option<bool> {
    use AemNode::*;
    match node {
        Panel { dor_exclude, .. } | TextDraw { dor_exclude, .. } => Some(*dor_exclude),
        _ => None,
    }
}

fn colspan_of(node: &AemNode) -> Option<u32> {
    use AemNode::*;
    match node {
        Panel { colspan, .. }
        | TextField { colspan, .. }
        | NumberField { colspan, .. }
        | DatePicker { colspan, .. }
        | Dropdown { colspan, .. }
        | Checkbox { colspan, .. }
        | RadioButton { colspan, .. }
        | TextDraw { colspan, .. }
        | TitleDraw { colspan, .. }
        | FootnotePlaceholder { colspan, .. }
        | Custom { colspan, .. } => Some(*colspan),
        _ => None,
    }
}

fn alignment_of(node: &AemNode) -> Option<OptionAlignment> {
    use AemNode::*;
    match node {
        Checkbox { alignment, .. } | RadioButton { alignment, .. } => Some(*alignment),
        _ => None,
    }
}
