//! Node renderer component.
//!
//! Recursively renders the structured node tree with selection and editing support.

use dioxus::prelude::*;
use std::collections::HashMap;

use blueprint::{FieldId, StructuredNode};

use super::metadata_editor::{MetadataEditor, MetadataNodeWrapper, has_editable_metadata};
use super::state::{
    EditorAction, NodePath, SelectionState, node_has_children, node_summary, node_type_name,
};
use super::text_editor::{InlineTextWrapper, ListItemEditor, TextEditor};

/// Wrapper for Vec<StructuredNode> that implements PartialEq.
#[derive(Clone)]
pub struct NodesWrapper(pub Vec<StructuredNode>);

impl PartialEq for NodesWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false // Always re-render
    }
}

/// Wrapper for a single StructuredNode.
#[derive(Clone)]
pub struct NodeWrapper(pub StructuredNode);

impl PartialEq for NodeWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false // Always re-render
    }
}

/// Wrapper for field labels map that implements PartialEq.
#[derive(Clone, Default)]
pub struct FieldLabelsWrapper(pub HashMap<FieldId, String>);

impl PartialEq for FieldLabelsWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false // Always re-render
    }
}

/// Properties for the node renderer.
#[derive(Clone, PartialEq, Props)]
pub struct NodeRendererProps {
    /// The nodes to render.
    pub nodes: NodesWrapper,
    /// Current selection state.
    pub selection: SelectionState,
    /// Languages available in the document.
    pub languages: Vec<String>,
    /// Map from field ID to field label.
    #[props(default)]
    pub field_labels: FieldLabelsWrapper,
    /// Base path for these nodes (empty for root).
    #[props(default)]
    pub base_path: NodePath,
    /// Nesting depth for indentation.
    #[props(default)]
    pub depth: usize,
    /// Callback for editor actions.
    pub on_action: EventHandler<EditorAction>,
}

/// Renders a list of structured nodes.
#[component]
pub fn NodeRenderer(props: NodeRendererProps) -> Element {
    rsx! {
        div { class: "node-list", style: "padding-left: {props.depth * 16}px",
            for (idx , node) in props.nodes.0.iter().enumerate() {
                {
                    let path = {
                        let mut p = props.base_path.clone();
                        p.push(idx);
                        p
                    };
                    rsx! {
                        NodeItem {
                            key: "{idx}",
                            node: NodeWrapper(node.clone()),
                            path,
                            selection: props.selection.clone(),
                            languages: props.languages.clone(),
                            field_labels: props.field_labels.clone(),
                            depth: props.depth,
                            on_action: props.on_action.clone(),
                        }
                    }
                }
            }
        }
    }
}

/// Properties for a single node item.
#[derive(Clone, PartialEq, Props)]
pub struct NodeItemProps {
    /// The node to render.
    pub node: NodeWrapper,
    /// Path to this node.
    pub path: NodePath,
    /// Current selection state.
    pub selection: SelectionState,
    /// Languages available in the document.
    pub languages: Vec<String>,
    /// Map from field ID to field label.
    #[props(default)]
    pub field_labels: FieldLabelsWrapper,
    /// Nesting depth.
    pub depth: usize,
    /// Callback for editor actions.
    pub on_action: EventHandler<EditorAction>,
}

/// Renders a single node with its header and optionally children.
#[component]
pub fn NodeItem(props: NodeItemProps) -> Element {
    let is_selected = props.selection.is_selected(&props.path);
    let is_editing = props.selection.is_editing(&props.path);
    let has_children = node_has_children(&props.node.0);
    let mut expanded = use_signal(|| true);

    let node_class = format!(
        "node-item {} {}",
        if is_selected { "selected" } else { "" },
        if is_editing { "editing" } else { "" }
    );

    let type_name = node_type_name(&props.node.0);
    let summary = node_summary(&props.node.0);
    let has_metadata = has_editable_metadata(&props.node.0);
    let is_editing_metadata = props.selection.is_editing_metadata(&props.path);

    // Check if this node type supports text editing
    let can_edit_text = match &props.node.0 {
        StructuredNode::Paragraph(_) | StructuredNode::Heading(_) => true,
        StructuredNode::Field(f) => f.label.is_some(),
        _ => false,
    };

    rsx! {
        div { class: "{node_class}",

            // Node header
            div {
                class: "node-header",
                onclick: {
                    let path = props.path.clone();
                    let on_action = props.on_action.clone();
                    move |evt: Event<MouseData>| {
                        if evt.modifiers().shift() {
                            on_action.call(EditorAction::ToggleSelection(path.clone()));
                        } else {
                            on_action.call(EditorAction::SelectSingle(path.clone()));
                        }
                    }
                },

                // Expand/collapse toggle for nodes with children
                if has_children {
                    button {
                        class: "node-toggle",
                        onclick: move |evt| {
                            evt.stop_propagation();
                            let current = *expanded.read();
                            expanded.set(!current);
                        },
                        if *expanded.read() {
                            "▼"
                        } else {
                            "▶"
                        }
                    }
                } else {
                    span { class: "node-toggle-placeholder" }
                }

                // Selection checkbox
                input {
                    r#type: "checkbox",
                    class: "node-checkbox",
                    checked: is_selected,
                    onclick: {
                        let path = props.path.clone();
                        let on_action = props.on_action.clone();
                        move |evt| {
                            evt.stop_propagation();
                            on_action.call(EditorAction::ToggleSelection(path.clone()));
                        }
                    },
                }

                // Type badge
                span { class: "node-type-badge node-type-{type_name.to_lowercase()}",
                    "{type_name}"
                }

                // Summary text
                span { class: "node-summary", "{summary}" }

                // Edit button for text nodes
                if can_edit_text && !is_editing {
                    button {
                        class: "node-edit-btn",
                        onclick: {
                            let path = props.path.clone();
                            let on_action = props.on_action.clone();
                            move |evt| {
                                evt.stop_propagation();
                                on_action.call(EditorAction::StartEditing(path.clone()));
                            }
                        },
                        "✎"
                    }
                }

                // Metadata edit button (gear icon)
                if has_metadata && !is_editing_metadata {
                    button {
                        class: "node-edit-btn node-metadata-btn",
                        title: "Edit properties",
                        onclick: {
                            let path = props.path.clone();
                            let on_action = props.on_action.clone();
                            move |evt| {
                                evt.stop_propagation();
                                on_action.call(EditorAction::StartEditingMetadata(path.clone()));
                            }
                        },
                        "⚙"
                    }
                }
            }

            // Text editor (when editing text)
            if is_editing {
                match &props.node.0 {
                    StructuredNode::Paragraph(p) => {
                        rsx! {
                            TextEditor {
                                content: InlineTextWrapper(p.content.clone()),
                                path: props.path.clone(),
                                languages: props.languages.clone(),
                                on_action: props.on_action.clone(),
                            }
                        }
                    }
                    StructuredNode::Heading(h) => {
                        rsx! {
                            TextEditor {
                                content: InlineTextWrapper(h.content.clone()),
                                path: props.path.clone(),
                                languages: props.languages.clone(),
                                on_action: props.on_action.clone(),
                            }
                        }
                    }
                    StructuredNode::Field(f) => {
                        if let Some(label) = &f.label {
                            rsx! {
                                TextEditor {
                                    content: InlineTextWrapper(label.clone()),
                                    path: props.path.clone(),
                                    languages: props.languages.clone(),
                                    on_action: props.on_action.clone(),
                                }
                            }
                        } else {
                            rsx! {}
                        }
                    }
                    _ => rsx! {},
                }
            }

            // Metadata editor (when editing metadata)
            if is_editing_metadata {
                MetadataEditor {
                    node: MetadataNodeWrapper(props.node.0.clone()),
                    path: props.path.clone(),
                    on_action: props.on_action.clone(),
                }
            }

            // Children (when expanded and has children)
            if has_children && *expanded.read() {
                div { class: "node-children",
                    match &props.node.0 {
                        StructuredNode::Group(g) => {
                            rsx! {
                                NodeRenderer {
                                    nodes: NodesWrapper(g.children.clone()),
                                    selection: props.selection.clone(),
                                    languages: props.languages.clone(),
                                    field_labels: props.field_labels.clone(),
                                    base_path: props.path.clone(),
                                    depth: props.depth + 1,
                                    on_action: props.on_action.clone(),
                                }
                            }
                        }
                        StructuredNode::List(l) => {
                            // Render list items as editable entries
                            rsx! {
                                div { class: "list-items",
                                    for (i , item) in l.items.iter().enumerate() {
                                        {
                                            let is_editing_item = props.selection.is_editing_list_item(&props.path, i);
                                            let path = props.path.clone();
                                            let on_action = props.on_action.clone();
                                            let languages = props.languages.clone();
                                            rsx! {
                                                div { key: "{i}", class: if is_editing_item { "list-item editing" } else { "list-item" },
                                                    span { class: "list-item-marker", "{i + 1}." }
                                                    if is_editing_item {
                                                        ListItemEditor {
                                                            content: InlineTextWrapper(item.clone()),
                                                            list_path: path.clone(),
                                                            item_index: i,
                                                            languages: languages.clone(),
                                                            on_action: on_action.clone(),
                                                        }
                                                    } else {
                                                        span { class: "list-item-text", "{item.as_plain_text()}" }
                                                        button {
                                                            class: "node-edit-btn",
                                                            onclick: {
                                                                let path = path.clone();
                                                                let on_action = on_action.clone();
                                                                move |evt| {
                                                                    evt.stop_propagation();
                                                                    on_action.call(EditorAction::StartEditingListItem(path.clone(), i));
                                                                }
                                                            },
                                                            "✎"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        StructuredNode::Repeatable(r) => {
                            // Render the repeatable's item template
                            rsx! {
                                div { class: "repeatable-content",
                                    span { class: "repeatable-label",
                                        "Template (min: {r.min_occurrences}, max: {r.max_occurrences.map(|m| m.to_string()).unwrap_or(\"∞\".to_string())})"
                                    }
                                    NodeRenderer {
                                        nodes: NodesWrapper(vec![(*r.item).clone()]),
                                        selection: props.selection.clone(),
                                        languages: props.languages.clone(),
                                        field_labels: props.field_labels.clone(),
                                        base_path: props.path.clone(),
                                        depth: props.depth + 1,
                                        on_action: props.on_action.clone(),
                                    }
                                }
                            }
                        }
                        StructuredNode::Conditional(c) => {
                            // Render the conditional's content
                            let field_label = props
                                .field_labels
                                .0
                                .get(&c.condition.field_name)
                                .cloned()
                                .unwrap_or_else(|| c.condition.field_name.to_string());
                            rsx! {
                                div { class: "conditional-content",
                                    span { class: "conditional-label", "When {field_label} = {c.condition.value:?}" }
                                    NodeRenderer {
                                        nodes: NodesWrapper(vec![(*c.content).clone()]),
                                        selection: props.selection.clone(),
                                        languages: props.languages.clone(),
                                        field_labels: props.field_labels.clone(),
                                        base_path: props.path.clone(),
                                        depth: props.depth + 1,
                                        on_action: props.on_action.clone(),
                                    }
                                }
                            }
                        }
                        StructuredNode::GridLayout(g) => {
                            rsx! {
                                div { class: "grid-content",
                                    span { class: "grid-label", "{g.columns} columns" }
                                    NodeRenderer {
                                        nodes: NodesWrapper(g.elements.iter().map(|e| e.node.clone()).collect()),
                                        selection: props.selection.clone(),
                                        languages: props.languages.clone(),
                                        field_labels: props.field_labels.clone(),
                                        base_path: props.path.clone(),
                                        depth: props.depth + 1,
                                        on_action: props.on_action.clone(),
                                    }
                                }
                            }
                        }
                        StructuredNode::Table(t) => {
                            rsx! {
                                div { class: "table-preview",
                                    "Table with {t.rows.len()} rows"
                                    if let Some(header) = &t.header {
                                        " and {header.cells.len()} columns"
                                    }
                                }
                            }
                        }
                        _ => rsx! {},
                    }
                }
            }
        }
    }
}
