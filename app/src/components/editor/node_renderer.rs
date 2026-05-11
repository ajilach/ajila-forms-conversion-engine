//! Node renderer component.
//!
//! Recursively renders the structured node tree with selection and editing support.

use dioxus::prelude::*;
use std::collections::HashMap;

use std::collections::HashSet;

use blueprint::document::ListStyleType;
use blueprint::{FieldId, ListNode, StructuredNode};

use super::metadata_editor::{MetadataEditor, MetadataNodeWrapper, has_editable_metadata};
use super::state::{
    EditorAction, NodePath, PathSegment, SelectionState, node_has_children, node_summary,
    node_type_name,
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

/// Wrapper for a list node.
#[derive(Clone)]
pub struct ListNodeWrapper(pub ListNode);

impl PartialEq for ListNodeWrapper {
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
    /// Paths of nodes to highlight (e.g. changed nodes in a diff preview).
    #[props(default)]
    pub highlight: HashSet<NodePath>,
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
                            highlight: props.highlight.clone(),
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
    /// Paths of nodes to highlight.
    #[props(default)]
    pub highlight: HashSet<NodePath>,
    /// Callback for editor actions.
    pub on_action: EventHandler<EditorAction>,
}

/// Properties for recursive list item rendering.
#[derive(Clone, PartialEq, Props)]
pub struct ListItemsRendererProps {
    /// List node to render.
    pub list: ListNodeWrapper,
    /// Path to the containing list node.
    pub base_path: NodePath,
    /// Current selection state.
    pub selection: SelectionState,
    /// Languages available in the document.
    pub languages: Vec<String>,
    /// Current list nesting depth.
    pub list_depth: usize,
    /// Callback for editor actions.
    pub on_action: EventHandler<EditorAction>,
}

/// Renders list items recursively, including nested sublists.
#[component]
pub fn ListItemsRenderer(props: ListItemsRendererProps) -> Element {
    rsx! {
        div { class: "list-items list-depth-{props.list_depth}",
            for (i , item) in props.list.0.items.iter().enumerate() {
                {
                    let item_path = {
                        let mut p = props.base_path.clone();
                        p.push(PathSegment::ListItem(i));
                        p
                    };
                    let is_selected = props.selection.is_selected(&item_path);
                    let is_editing = props.selection.is_editing(&item_path);
                    let item_class = format!(
                        "list-item pseudo-node {}{}",
                        if is_selected { "selected " } else { "" },
                        if is_editing { "editing" } else { "" },
                    );

                    rsx! {
                        div { key: "{i}", class: "list-item-block",
                            div {
                                class: "{item_class}",
                                onclick: {
                                    let path = item_path.clone();
                                    let on_action = props.on_action;
                                    move |evt: Event<MouseData>| {
                                        evt.stop_propagation();
                                        if evt.modifiers().shift() {
                                            on_action.call(EditorAction::ToggleSelection(path.clone()));
                                        } else {
                                            on_action.call(EditorAction::SelectSingle(path.clone()));
                                        }
                                    }
                                },

                                input {
                                    r#type: "checkbox",
                                    class: "node-checkbox",
                                    checked: is_selected,
                                    onclick: {
                                        let path = item_path.clone();
                                        let on_action = props.on_action;
                                        move |evt| {
                                            evt.stop_propagation();
                                            on_action.call(EditorAction::ToggleSelection(path.clone()));
                                        }
                                    },
                                }

                                span { class: "list-item-marker", "{format_list_marker(props.list.0.list_style, i)}" }

                                if is_editing {
                                    TextEditor {
                                        content: InlineTextWrapper(item.content.clone()),
                                        path: item_path.clone(),
                                        languages: props.languages.clone(),
                                        on_action: props.on_action,
                                    }
                                } else {
                                    span { class: "list-item-text", "{item.as_plain_text()}" }
                                    button {
                                        class: "node-edit-btn",
                                        onclick: {
                                            let path = item_path.clone();
                                            let on_action = props.on_action;
                                            move |evt| {
                                                evt.stop_propagation();
                                                on_action.call(EditorAction::StartEditing(path.clone()));
                                            }
                                        },
                                        "✎"
                                    }
                                }
                            }

                            if let Some(sublist) = &item.sublist {
                                div { class: "list-sublist",
                                    ListItemsRenderer {
                                        list: ListNodeWrapper((**sublist).clone()),
                                        base_path: item_path.clone(),
                                        selection: props.selection.clone(),
                                        languages: props.languages.clone(),
                                        list_depth: props.list_depth + 1,
                                        on_action: props.on_action,
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

fn format_list_marker(style: ListStyleType, index: usize) -> String {
    let one_based = index + 1;
    match style {
        ListStyleType::Disc => "•".to_string(),
        ListStyleType::Circle => "○".to_string(),
        ListStyleType::Square => "■".to_string(),
        ListStyleType::Dash => "–".to_string(),
        ListStyleType::Decimal => format!("{one_based}."),
        ListStyleType::LowerAlpha => format!("{}.", alpha_index(one_based, false)),
        ListStyleType::UpperAlpha => format!("{}.", alpha_index(one_based, true)),
        ListStyleType::LowerRoman => format!("{}.", roman_index(one_based, false)),
        ListStyleType::UpperRoman => format!("{}.", roman_index(one_based, true)),
    }
}

fn alpha_index(mut n: usize, uppercase: bool) -> String {
    let mut chars = Vec::new();
    while n > 0 {
        n -= 1;
        chars.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    chars.reverse();
    let s: String = chars.into_iter().collect();
    if uppercase { s.to_uppercase() } else { s }
}

fn roman_index(mut n: usize, uppercase: bool) -> String {
    if n == 0 {
        return "0".to_string();
    }

    let numerals = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];

    let mut result = String::new();
    for (value, token) in numerals {
        while n >= value {
            result.push_str(token);
            n -= value;
        }
    }

    if uppercase {
        result
    } else {
        result.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_list_marker_supports_ordered_styles() {
        assert_eq!(format_list_marker(ListStyleType::Decimal, 2), "3.");
        assert_eq!(format_list_marker(ListStyleType::LowerAlpha, 0), "a.");
        assert_eq!(format_list_marker(ListStyleType::UpperAlpha, 27), "AB.");
        assert_eq!(format_list_marker(ListStyleType::LowerRoman, 3), "iv.");
        assert_eq!(format_list_marker(ListStyleType::UpperRoman, 8), "IX.");
    }

    #[test]
    fn format_list_marker_supports_unordered_styles() {
        assert_eq!(format_list_marker(ListStyleType::Disc, 0), "•");
        assert_eq!(format_list_marker(ListStyleType::Circle, 0), "○");
        assert_eq!(format_list_marker(ListStyleType::Square, 0), "■");
        assert_eq!(format_list_marker(ListStyleType::Dash, 0), "–");
    }
}

/// Renders a single node with its header and optionally children.
#[component]
pub fn NodeItem(props: NodeItemProps) -> Element {
    let is_selected = props.selection.is_selected(&props.path);
    let is_editing = props.selection.is_editing(&props.path);
    let is_highlighted = props.highlight.contains(&props.path);
    let has_children = node_has_children(&props.node.0);
    let mut expanded = use_signal(|| true);

    let node_class = format!(
        "node-item {} {} {}",
        if is_selected { "selected" } else { "" },
        if is_editing { "editing" } else { "" },
        if is_highlighted { "node-changed" } else { "" },
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
                                    highlight: props.highlight.clone(),
                                    on_action: props.on_action,
                                }
                            }
                        }
                        StructuredNode::List(l) => {
                            rsx! {
                                ListItemsRenderer {
                                    list: ListNodeWrapper(l.clone()),
                                    base_path: props.path.clone(),
                                    selection: props.selection.clone(),
                                    languages: props.languages.clone(),
                                    list_depth: 0,
                                    on_action: props.on_action,
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
                                        highlight: props.highlight.clone(),
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
                                        highlight: props.highlight.clone(),
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
                                        highlight: props.highlight.clone(),
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
                            let highlight = props.highlight.clone();
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
                                                if is_selected { "selected" } else { "" },
                                            );
                                            rsx! {
                                                div {
                                                    class: "{header_class}",
                                                    onclick: {
                                                        let path = header_path.clone();
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
                                                                            highlight: highlight.clone(),
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
                                                if is_selected { "selected" } else { "" },
                                            );
                                            rsx! {
                                                div {
                                                    key: "row-{ri}",
                                                    class: "{row_class}",
                                                    onclick: {
                                                        let path = row_path.clone();
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
                                                                            highlight: highlight.clone(),
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
