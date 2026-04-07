//! Metadata editor component.
//!
//! Provides a form for editing node metadata like heading level, repeatable
//! min/max occurrences, grid columns, field type, etc.

use dioxus::prelude::*;

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
            let current_kind = match &f.input_type {
                FieldType::Text { .. } => FieldInputKind::Text,
                FieldType::Number { .. } => FieldInputKind::Number,
                FieldType::Date => FieldInputKind::Date,
                FieldType::Email => FieldInputKind::Email,
                FieldType::Tel => FieldInputKind::Tel,
                FieldType::Bool => FieldInputKind::Checkbox,
                FieldType::Select { .. } => FieldInputKind::Dropdown,
                FieldType::Radio { .. } => FieldInputKind::Radio,
            };
            let current_value = match current_kind {
                FieldInputKind::Text => "text",
                FieldInputKind::Number => "number",
                FieldInputKind::Date => "date",
                FieldInputKind::Email => "email",
                FieldInputKind::Tel => "tel",
                FieldInputKind::Checkbox => "checkbox",
                FieldInputKind::Dropdown => "dropdown",
                FieldInputKind::Radio => "radio",
            };

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
                                        let kind = match evt.value().as_str() {
                                            "text" => FieldInputKind::Text,
                                            "number" => FieldInputKind::Number,
                                            "date" => FieldInputKind::Date,
                                            "email" => FieldInputKind::Email,
                                            "tel" => FieldInputKind::Tel,
                                            "checkbox" => FieldInputKind::Checkbox,
                                            "dropdown" => FieldInputKind::Dropdown,
                                            "radio" => FieldInputKind::Radio,
                                            _ => return,
                                        };
                                        on_action
                                            .call(EditorAction::UpdateMetadata {
                                                path: path.clone(),
                                                metadata: NodeMetadata::FieldInputType(kind),
                                            });
                                    }
                                },
                                option { value: "text", "Text" }
                                option { value: "number", "Number" }
                                option { value: "date", "Date" }
                                option { value: "email", "Email" }
                                option { value: "tel", "Phone" }
                                option { value: "checkbox", "Checkbox" }
                                option { value: "dropdown", "Dropdown" }
                                option { value: "radio", "Radio" }
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
