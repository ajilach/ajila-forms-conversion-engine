//! Node renderer component.
//!
//! Recursively renders the structured node tree with selection and editing support.

use dioxus::prelude::*;
use std::collections::HashMap;

use blueprint::{FieldId, StructuredNode};

use super::metadata_editor::{MetadataEditor, MetadataNodeWrapper, has_editable_metadata};
use super::state::{
    EditorAction, NodePath, PathSegment, SelectionState,
    node_has_children, node_summary, node_type_name,
};
use super::text_editor::{InlineTextWrapper, TextEditor};

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
                        p.push(PathSegment::Child(idx));
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
                            on_action: props.on_action,
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
                    let on_action = props.on_action;
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
                        let on_action = props.on_action;
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
                            let on_action = props.on_action;
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
                            let on_action = props.on_action;
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
                                on_action: props.on_action,
                            }
                        }
                    }
                    StructuredNode::Heading(h) => {
                        rsx! {
                            TextEditor {
                                content: InlineTextWrapper(h.content.clone()),
                                path: props.path.clone(),
                                languages: props.languages.clone(),
                                on_action: props.on_action,
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
                                    on_action: props.on_action,
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
                    on_action: props.on_action,
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
                                    on_action: props.on_action,
                                }
                            }
                        }
                        StructuredNode::List(l) => {
                            // Render list items as pseudo-nodes (selectable, movable, deletable)
                            let list_path = props.path.clone();
                            let selection = props.selection.clone();
                            let languages = props.languages.clone();
                            let on_action = props.on_action;
                            rsx! {
                                div { class: "list-items",
                                    for (i , item) in l.items.iter().enumerate() {
                                        {
                                            let item_path = {
                                                let mut p = list_path.clone();
                                                p.push(PathSegment::ListItem(i));
                                                p
                                            };
                                            let is_selected = selection.is_selected(&item_path);
                                            let is_editing = selection.is_editing(&item_path);
                                            let item_class = format!(
                                                "list-item pseudo-node {}{}",
                                                if is_selected { "selected " } else { "" },
                                                if is_editing { "editing" } else { "" }
                                            );
                                            rsx! {
                                                div {
                                                    key: "{i}",
                                                    class: "{item_class}",
                                                    onclick: {
                                                        let path = item_path.clone();
                                                        let on_action = on_action;
                                                        move |evt: Event<MouseData>| {
                                                            evt.stop_propagation();
                                                            if evt.modifiers().shift() {
                                                                on_action.call(EditorAction::ToggleSelection(path.clone()));
                                                            } else {
                                                                on_action.call(EditorAction::SelectSingle(path.clone()));
                                                            }
                                                        }
                                                    },

                                                    // Selection checkbox
                                                    input {
                                                        r#type: "checkbox",
                                                        class: "node-checkbox",
                                                        checked: is_selected,
                                                        onclick: {
                                                            let path = item_path.clone();
                                                            let on_action = on_action;
                                                            move |evt| {
                                                                evt.stop_propagation();
                                                                on_action.call(EditorAction::ToggleSelection(path.clone()));
                                                            }
                                                        },
                                                    }

                                                    // Item marker
                                                    span { class: "list-item-marker", "{i + 1}." }

                                                    // Item content or editor
                                                    if is_editing {
                                                        TextEditor {
                                                            content: InlineTextWrapper(item.content.clone()),
                                                            path: item_path.clone(),
                                                            languages: languages.clone(),
                                                            on_action,
                                                        }
                                                    } else {
                                                        span { class: "list-item-text", "{item.as_plain_text()}" }
                                                        button {
                                                            class: "node-edit-btn",
                                                            onclick: {
                                                                let path = item_path.clone();
                                                                let on_action = on_action;
                                                                move |evt| {
                                                                    evt.stop_propagation();
                                                                    on_action.call(EditorAction::StartEditing(path.clone()));
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
                                        on_action: props.on_action,
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
                                        on_action: props.on_action,
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
                                        on_action: props.on_action,
                                    }
                                }
                            }
                        }
                        StructuredNode::Table(t) => {
                            let table_path = props.path.clone();
                            let selection = props.selection.clone();
                            let on_action = props.on_action;
                            let languages = props.languages.clone();
                            let field_labels = props.field_labels.clone();
                            let depth = props.depth;
                            rsx! {
                                div { class: "table-content",
                                    // Render header if present
                                    if let Some(header) = &t.header {
                                        {
                                            let header_path = {
                                                let mut p = table_path.clone();
                                                p.push(PathSegment::TableHeader);
                                                p
                                            };
                                            let is_selected = selection.is_selected(&header_path);
                                            let header_class = format!(
                                                "table-row table-header pseudo-node {}",
                                                if is_selected { "selected" } else { "" }
                                            );
                                            rsx! {
                                                div {
                                                    class: "{header_class}",
                                                    onclick: {
                                                        let path = header_path.clone();
                                                        let on_action = on_action;
                                                        move |evt: Event<MouseData>| {
                                                            evt.stop_propagation();
                                                            if evt.modifiers().shift() {
                                                                on_action.call(EditorAction::ToggleSelection(path.clone()));
                                                            } else {
                                                                on_action.call(EditorAction::SelectSingle(path.clone()));
                                                            }
                                                        }
                                                    },

                                                    // Selection checkbox
                                                    input {
                                                        r#type: "checkbox",
                                                        class: "node-checkbox",
                                                        checked: is_selected,
                                                        onclick: {
                                                            let path = header_path.clone();
                                                            let on_action = on_action;
                                                            move |evt| {
                                                                evt.stop_propagation();
                                                                on_action.call(EditorAction::ToggleSelection(path.clone()));
                                                            }
                                                        },
                                                    }

                                                    span { class: "table-row-label", "Header" }

                                                    // Render header cells
                                                    div { class: "table-cells",
                                                        for (ci , cell) in header.cells.iter().enumerate() {
                                                            {
                                                                let cell_path = {
                                                                    let mut p = header_path.clone();
                                                                    p.push(PathSegment::TableCell(ci));
                                                                    p
                                                                };
                                                                rsx! {
                                                                    div { class: "table-cell",
                                                                        span { class: "table-cell-label", "Cell {ci + 1}" }
                                                                        NodeRenderer {
                                                                            nodes: NodesWrapper(vec![cell.clone()]),
                                                                            selection: selection.clone(),
                                                                            languages: languages.clone(),
                                                                            field_labels: field_labels.clone(),
                                                                            base_path: cell_path,
                                                                            depth: depth + 2,
                                                                            on_action,
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Render data rows
                                    for (ri , row) in t.rows.iter().enumerate() {
                                        {
                                            let row_path = {
                                                let mut p = table_path.clone();
                                                p.push(PathSegment::TableRow(ri));
                                                p
                                            };
                                            let is_selected = selection.is_selected(&row_path);
                                            let row_class = format!(
                                                "table-row pseudo-node {}",
                                                if is_selected { "selected" } else { "" }
                                            );
                                            rsx! {
                                                div {
                                                    key: "row-{ri}",
                                                    class: "{row_class}",
                                                    onclick: {
                                                        let path = row_path.clone();
                                                        let on_action = on_action;
                                                        move |evt: Event<MouseData>| {
                                                            evt.stop_propagation();
                                                            if evt.modifiers().shift() {
                                                                on_action.call(EditorAction::ToggleSelection(path.clone()));
                                                            } else {
                                                                on_action.call(EditorAction::SelectSingle(path.clone()));
                                                            }
                                                        }
                                                    },

                                                    // Selection checkbox
                                                    input {
                                                        r#type: "checkbox",
                                                        class: "node-checkbox",
                                                        checked: is_selected,
                                                        onclick: {
                                                            let path = row_path.clone();
                                                            let on_action = on_action;
                                                            move |evt| {
                                                                evt.stop_propagation();
                                                                on_action.call(EditorAction::ToggleSelection(path.clone()));
                                                            }
                                                        },
                                                    }

                                                    span { class: "table-row-label", "Row {ri + 1}" }

                                                    // Render row cells
                                                    div { class: "table-cells",
                                                        for (ci , cell) in row.cells.iter().enumerate() {
                                                            {
                                                                let cell_path = {
                                                                    let mut p = row_path.clone();
                                                                    p.push(PathSegment::TableCell(ci));
                                                                    p
                                                                };
                                                                rsx! {
                                                                    div { class: "table-cell",
                                                                        span { class: "table-cell-label", "Cell {ci + 1}" }
                                                                        NodeRenderer {
                                                                            nodes: NodesWrapper(vec![cell.clone()]),
                                                                            selection: selection.clone(),
                                                                            languages: languages.clone(),
                                                                            field_labels: field_labels.clone(),
                                                                            base_path: cell_path,
                                                                            depth: depth + 2,
                                                                            on_action,
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
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
