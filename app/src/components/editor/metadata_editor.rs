//! Metadata editor component.
//!
//! Provides a form for editing node metadata like heading level, repeatable
//! min/max occurrences, grid columns, field type, etc.

use dioxus::prelude::*;

use blueprint::structured::{InputValue, NameValue, TranslatableString};
use blueprint::{FieldType, StructuredNode};

use super::state::{EditorAction, FieldInputKind, NodeMetadata, NodePath};

/// Wrapper for StructuredNode that implements PartialEq.
#[derive(Clone)]
pub struct MetadataNodeWrapper(pub StructuredNode);

impl PartialEq for MetadataNodeWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false // Always re-render
    }
}

/// Properties for the metadata editor.
#[derive(Clone, PartialEq, Props)]
pub struct MetadataEditorProps {
    /// The node being edited.
    pub node: MetadataNodeWrapper,
    /// Path to the node.
    pub path: NodePath,
    /// Callback when metadata is updated.
    pub on_action: EventHandler<EditorAction>,
}

/// Metadata editor component.
#[component]
pub fn MetadataEditor(props: MetadataEditorProps) -> Element {
    match &props.node.0 {
        StructuredNode::Heading(h) => {
            let current_level = h.level.as_u8();
            rsx! {
                div { class: "metadata-editor",
                    div { class: "metadata-editor-header",
                        span { class: "metadata-editor-title", "Edit Heading" }
                        button {
                            class: "metadata-editor-close",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "×"
                        }
                    }
                    div { class: "metadata-editor-content",
                        div { class: "metadata-field",
                            label { class: "metadata-label", "Heading Level" }
                            select {
                                class: "metadata-select",
                                value: "{current_level}",
                                onchange: {
                                    let path = props.path.clone();
                                    let on_action = props.on_action;
                                    move |evt: Event<FormData>| {
                                        if let Ok(level) = evt.value().parse::<u8>() {
                                            on_action
                                                .call(EditorAction::UpdateMetadata {
                                                    path: path.clone(),
                                                    metadata: NodeMetadata::HeadingLevel(level),
                                                });
                                        }
                                    }
                                },
                                option { value: "1", "H1 - Title" }
                                option { value: "2", "H2 - Section" }
                                option { value: "3", "H3 - Subsection" }
                                option { value: "4", "H4" }
                                option { value: "5", "H5" }
                                option { value: "6", "H6" }
                            }
                        }
                    }
                    div { class: "metadata-editor-actions",
                        button {
                            class: "metadata-btn metadata-btn-done",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "Done"
                        }
                    }
                }
            }
        }
        StructuredNode::Repeatable(r) => {
            let min = r.min_occurrences;
            let max = r.max_occurrences;
            let mut min_signal = use_signal(|| min);
            let mut max_signal = use_signal(|| max);
            let mut max_unlimited = use_signal(|| max.is_none());

            rsx! {
                div { class: "metadata-editor",
                    div { class: "metadata-editor-header",
                        span { class: "metadata-editor-title", "Edit Repeatable" }
                        button {
                            class: "metadata-editor-close",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "×"
                        }
                    }
                    div { class: "metadata-editor-content",
                        div { class: "metadata-field",
                            label { class: "metadata-label", "Minimum Occurrences" }
                            input {
                                class: "metadata-input",
                                r#type: "number",
                                min: "0",
                                value: "{*min_signal.read()}",
                                oninput: move |evt: Event<FormData>| {
                                    if let Ok(v) = evt.value().parse::<u32>() {
                                        min_signal.set(v);
                                    }
                                },
                            }
                        }
                        div { class: "metadata-field",
                            label { class: "metadata-label", "Maximum Occurrences" }
                            div { class: "metadata-field-row",
                                input {
                                    class: "metadata-input",
                                    r#type: "number",
                                    min: "1",
                                    disabled: *max_unlimited.read(),
                                    value: "{max_signal.read().unwrap_or(1)}",
                                    oninput: move |evt: Event<FormData>| {
                                        if let Ok(v) = evt.value().parse::<u32>() {
                                            max_signal.set(Some(v));
                                        }
                                    },
                                }
                                label { class: "metadata-checkbox-label",
                                    input {
                                        r#type: "checkbox",
                                        checked: *max_unlimited.read(),
                                        onchange: move |evt: Event<FormData>| {
                                            let checked = evt.value() == "true";
                                            max_unlimited.set(checked);
                                            if checked {
                                                max_signal.set(None);
                                            } else {
                                                max_signal.set(Some(1));
                                            }
                                        },
                                    }
                                    "Unlimited"
                                }
                            }
                        }
                    }
                    div { class: "metadata-editor-actions",
                        button {
                            class: "metadata-btn metadata-btn-done",
                            onclick: {
                                let path = props.path.clone();
                                let on_action = props.on_action;
                                move |_| {
                                    on_action
                                        .call(EditorAction::UpdateMetadata {
                                            path: path.clone(),
                                            metadata: NodeMetadata::Repeatable {
                                                min: *min_signal.read(),
                                                max: *max_signal.read(),
                                            },
                                        });
                                    on_action.call(EditorAction::StopEditing);
                                }
                            },
                            "Save"
                        }
                    }
                }
            }
        }
        StructuredNode::GridLayout(g) => {
            let cols = g.columns;
            let mut cols_signal = use_signal(|| cols);

            rsx! {
                div { class: "metadata-editor",
                    div { class: "metadata-editor-header",
                        span { class: "metadata-editor-title", "Edit Grid Layout" }
                        button {
                            class: "metadata-editor-close",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "×"
                        }
                    }
                    div { class: "metadata-editor-content",
                        div { class: "metadata-field",
                            label { class: "metadata-label", "Number of Columns" }
                            input {
                                class: "metadata-input",
                                r#type: "number",
                                min: "1",
                                max: "12",
                                value: "{*cols_signal.read()}",
                                oninput: move |evt: Event<FormData>| {
                                    if let Ok(v) = evt.value().parse::<usize>() {
                                        cols_signal.set(v.clamp(1, 12));
                                    }
                                },
                            }
                        }
                    }
                    div { class: "metadata-editor-actions",
                        button {
                            class: "metadata-btn metadata-btn-done",
                            onclick: {
                                let path = props.path.clone();
                                let on_action = props.on_action;
                                move |_| {
                                    on_action
                                        .call(EditorAction::UpdateMetadata {
                                            path: path.clone(),
                                            metadata: NodeMetadata::GridColumns(*cols_signal.read()),
                                        });
                                    on_action.call(EditorAction::StopEditing);
                                }
                            },
                            "Save"
                        }
                    }
                }
            }
        }
        StructuredNode::Field(f) => {
            // Determine current field type
            let current_kind = field_input_kind_from_field_type(&f.input_type);
            let current_value = field_input_kind_value(&current_kind);

            let has_options = matches!(
                &f.input_type,
                FieldType::Radio { .. } | FieldType::Select { .. }
            );
            let initial_options = match &f.input_type {
                FieldType::Radio { options } | FieldType::Select { options } => options.clone(),
                _ => vec![],
            };
            let mut options_signal = use_signal(|| initial_options);

            rsx! {
                div { class: "metadata-editor",
                    div { class: "metadata-editor-header",
                        span { class: "metadata-editor-title", "Edit Field" }
                        button {
                            class: "metadata-editor-close",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "×"
                        }
                    }
                    div { class: "metadata-editor-content",
                        div { class: "metadata-field",
                            label { class: "metadata-label", "Field Type" }
                            select {
                                class: "metadata-select",
                                value: "{current_value}",
                                onchange: {
                                    let path = props.path.clone();
                                    let on_action = props.on_action;
                                    move |evt: Event<FormData>| {
                                        let Some(kind) = parse_field_input_kind(evt.value().as_str()) else {
                                            return;
                                        };
                                        let new_has_options = matches!(
                                            kind,
                                            FieldInputKind::Dropdown | FieldInputKind::Radio
                                        );
                                        if !new_has_options {
                                            options_signal.set(vec![]);
                                        }
                                        on_action
                                            .call(EditorAction::UpdateMetadata {
                                                path: path.clone(),
                                                metadata: NodeMetadata::FieldInputType(kind),
                                            });
                                    }
                                },
                                option { value: "text", "Text" }
                                option { value: "textarea", "Textarea" }
                                option { value: "number", "Number" }
                                option { value: "date", "Date" }
                                option { value: "email", "Email" }
                                option { value: "tel", "Phone" }
                                option { value: "checkbox", "Checkbox" }
                                option { value: "dropdown", "Dropdown" }
                                option { value: "radio", "Radio" }
                            }
                        }
                        div { class: "metadata-field",
                            label { class: "metadata-checkbox-label",
                                input {
                                    r#type: "checkbox",
                                    checked: f.required,
                                    onchange: {
                                        let path = props.path.clone();
                                        let on_action = props.on_action;
                                        move |evt: Event<FormData>| {
                                            let checked = evt.value() == "true";
                                            on_action
                                                .call(EditorAction::UpdateMetadata {
                                                    path: path.clone(),
                                                    metadata: NodeMetadata::FieldRequired(checked),
                                                });
                                        }
                                    },
                                }
                                "Required"
                            }
                        }
                        if has_options {
                            div { class: "metadata-field",
                                label { class: "metadata-label", "Options" }
                                div { class: "options-list",
                                    for (idx , option) in options_signal.read().iter().enumerate() {
                                        {
                                            let name_str = option.name.as_str().to_string();
                                            let value_str = match &option.value {
                                                InputValue::Text(s) => s.clone(),
                                                InputValue::Number(n) => n.to_string(),
                                                InputValue::Bool(b) => b.to_string(),
                                            };
                                            rsx! {
                                                div { class: "option-row",
                                                    input {
                                                        class: "metadata-input option-name-input",
                                                        r#type: "text",
                                                        placeholder: "Label",
                                                        value: "{name_str}",
                                                        oninput: move |evt: Event<FormData>| {
                                                            let mut opts = options_signal.read().clone();
                                                            if let Some(opt) = opts.get_mut(idx) {
                                                                opt.name = TranslatableString::Plain(evt.value());
                                                            }
                                                            options_signal.set(opts);
                                                        },
                                                    }
                                                    input {
                                                        class: "metadata-input option-value-input",
                                                        r#type: "text",
                                                        placeholder: "Value",
                                                        value: "{value_str}",
                                                        oninput: move |evt: Event<FormData>| {
                                                            let mut opts = options_signal.read().clone();
                                                            if let Some(opt) = opts.get_mut(idx) {
                                                                opt.value = InputValue::Text(evt.value());
                                                            }
                                                            options_signal.set(opts);
                                                        },
                                                    }
                                                    button {
                                                        class: "option-remove-btn",
                                                        title: "Remove option",
                                                        onclick: move |_| {
                                                            let mut opts = options_signal.read().clone();
                                                            opts.remove(idx);
                                                            options_signal.set(opts);
                                                        },
                                                        "✕"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    button {
                                        class: "option-add-btn",
                                        onclick: move |_| {
                                            let mut opts = options_signal.read().clone();
                                            opts.push(NameValue {
                                                name: TranslatableString::Plain(String::new()),
                                                value: InputValue::Text(String::new()),
                                            });
                                            options_signal.set(opts);
                                        },
                                        "+ Add Option"
                                    }
                                }
                            }
                        }
                    }
                    div { class: "metadata-editor-actions",
                        button {
                            class: "metadata-btn metadata-btn-done",
                            onclick: {
                                let path = props.path.clone();
                                let on_action = props.on_action;
                                move |_| {
                                    if has_options {
                                        let opts: Vec<_> = options_signal
                                            .read()
                                            .iter()
                                            .filter(|o| !o.name.as_str().is_empty())
                                            .cloned()
                                            .collect();
                                        on_action
                                            .call(EditorAction::UpdateMetadata {
                                                path: path.clone(),
                                                metadata: NodeMetadata::FieldOptions(opts),
                                            });
                                    }
                                    on_action.call(EditorAction::StopEditing);
                                }
                            },
                            if has_options {
                                "Save"
                            } else {
                                "Done"
                            }
                        }
                    }
                }
            }
        }
        _ => {
            // No editable metadata for this node type
            rsx! {
                div { class: "metadata-editor",
                    div { class: "metadata-editor-header",
                        span { class: "metadata-editor-title", "Node Properties" }
                        button {
                            class: "metadata-editor-close",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "×"
                        }
                    }
                    div { class: "metadata-editor-content",
                        p { class: "metadata-info", "No editable properties for this node type." }
                    }
                    div { class: "metadata-editor-actions",
                        button {
                            class: "metadata-btn metadata-btn-done",
                            onclick: {
                                let on_action = props.on_action;
                                move |_| on_action.call(EditorAction::StopEditing)
                            },
                            "Close"
                        }
                    }
                }
            }
        }
    }
}

fn field_input_kind_from_field_type(input_type: &FieldType) -> FieldInputKind {
    match input_type {
        FieldType::Text { .. } => FieldInputKind::Text,
        FieldType::Textarea { .. } => FieldInputKind::Textarea,
        FieldType::Number { .. } => FieldInputKind::Number,
        FieldType::Date => FieldInputKind::Date,
        FieldType::Email => FieldInputKind::Email,
        FieldType::Tel => FieldInputKind::Tel,
        FieldType::Bool => FieldInputKind::Checkbox,
        FieldType::Select { .. } => FieldInputKind::Dropdown,
        FieldType::Radio { .. } => FieldInputKind::Radio,
    }
}

fn field_input_kind_value(kind: &FieldInputKind) -> &'static str {
    match kind {
        FieldInputKind::Text => "text",
        FieldInputKind::Textarea => "textarea",
        FieldInputKind::Number => "number",
        FieldInputKind::Date => "date",
        FieldInputKind::Email => "email",
        FieldInputKind::Tel => "tel",
        FieldInputKind::Checkbox => "checkbox",
        FieldInputKind::Dropdown => "dropdown",
        FieldInputKind::Radio => "radio",
    }
}

fn parse_field_input_kind(value: &str) -> Option<FieldInputKind> {
    match value {
        "text" => Some(FieldInputKind::Text),
        "textarea" => Some(FieldInputKind::Textarea),
        "number" => Some(FieldInputKind::Number),
        "date" => Some(FieldInputKind::Date),
        "email" => Some(FieldInputKind::Email),
        "tel" => Some(FieldInputKind::Tel),
        "checkbox" => Some(FieldInputKind::Checkbox),
        "dropdown" => Some(FieldInputKind::Dropdown),
        "radio" => Some(FieldInputKind::Radio),
        _ => None,
    }
}

/// Check if a node has editable metadata.
pub fn has_editable_metadata(node: &StructuredNode) -> bool {
    matches!(
        node,
        StructuredNode::Heading(_)
            | StructuredNode::Repeatable(_)
            | StructuredNode::GridLayout(_)
            | StructuredNode::Field(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textarea_field_type_maps_to_textarea_input_kind() {
        assert_eq!(
            field_input_kind_from_field_type(&FieldType::Textarea { max_length: None }),
            FieldInputKind::Textarea
        );
    }

    #[test]
    fn textarea_input_kind_roundtrips_through_value_parser() {
        let parsed = parse_field_input_kind("textarea");
        assert_eq!(parsed, Some(FieldInputKind::Textarea));
        assert_eq!(
            field_input_kind_value(&FieldInputKind::Textarea),
            "textarea"
        );
    }
}
