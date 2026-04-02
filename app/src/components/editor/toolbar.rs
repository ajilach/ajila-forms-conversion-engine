//! Editor toolbar component.
//!
//! Provides action buttons for merge, delete, and add operations.

use dioxus::prelude::*;

use super::state::{EditorAction, NewNodeType, SelectionState};

/// Properties for the editor toolbar.
#[derive(Clone, PartialEq, Props)]
pub struct ToolbarProps {
    /// Current selection state.
    pub selection: SelectionState,
    /// Whether merge is allowed for current selection.
    pub can_merge: bool,
    /// Whether the selected node can be moved up.
    pub can_move_up: bool,
    /// Whether the selected node can be moved down.
    pub can_move_down: bool,
    /// Callback when an action is triggered.
    pub on_action: EventHandler<EditorAction>,
}

/// Toolbar component with action buttons.
#[component]
pub fn EditorToolbar(props: ToolbarProps) -> Element {
    let selection_count = props.selection.count();
    let has_selection = selection_count > 0;
    let can_merge = props.can_merge && selection_count >= 2;

    rsx! {
        div { class: "editor-toolbar",
            // Merge button
            button {
                class: "toolbar-btn",
                disabled: !can_merge,
                title: if can_merge { "Merge selected nodes" } else if selection_count < 2 { "Select at least 2 nodes to merge" } else { "Selected nodes cannot be merged" },
                onclick: move |_| props.on_action.call(EditorAction::MergeSelected),
                span { class: "toolbar-icon", "⊕" }
                span { class: "toolbar-label", "Merge" }
            }

            // Delete button
            button {
                class: "toolbar-btn toolbar-btn-danger",
                disabled: !has_selection,
                title: if has_selection { format!("Delete {} selected node(s)", selection_count) } else { "Select nodes to delete".to_string() },
                onclick: move |_| props.on_action.call(EditorAction::DeleteSelected),
                span { class: "toolbar-icon", "✕" }
                span { class: "toolbar-label", "Delete" }
            }

            // Separator
            div { class: "toolbar-separator" }

            // Move Up button
            button {
                class: "toolbar-btn",
                disabled: !props.can_move_up,
                title: if props.can_move_up { if selection_count > 1 { "Move selected nodes up" } else { "Move selected node up" } } else if !has_selection { "Select nodes to move" } else { "Cannot move up (already at top)" },
                onclick: move |_| props.on_action.call(EditorAction::MoveUp),
                span { class: "toolbar-icon", "↑" }
                span { class: "toolbar-label", "Up" }
            }

            // Move Down button
            button {
                class: "toolbar-btn",
                disabled: !props.can_move_down,
                title: if props.can_move_down { if selection_count > 1 {
                    "Move selected nodes down"
                } else {
                    "Move selected node down"
                } } else if !has_selection { "Select nodes to move" } else { "Cannot move down (already at bottom)" },
                onclick: move |_| props.on_action.call(EditorAction::MoveDown),
                span { class: "toolbar-icon", "↓" }
                span { class: "toolbar-label", "Down" }
            }

            // Separator
            div { class: "toolbar-separator" }

            // Add dropdown
            div { class: "toolbar-dropdown",
                button { class: "toolbar-btn", title: "Add new element",
                    span { class: "toolbar-icon", "+" }
                    span { class: "toolbar-label", "Add" }
                    span { class: "toolbar-caret", "▾" }
                }
                div { class: "toolbar-dropdown-menu",
                    button {
                        class: "dropdown-item",
                        onclick: move |_| {
                            props
                                .on_action
                                .call(EditorAction::AddNode {
                                    parent: vec![],
                                    index: 0,
                                    node_type: NewNodeType::Paragraph,
                                })
                        },
                        "Paragraph"
                    }
                    button {
                        class: "dropdown-item",
                        onclick: move |_| {
                            props
                                .on_action
                                .call(EditorAction::AddNode {
                                    parent: vec![],
                                    index: 0,
                                    node_type: NewNodeType::Heading(2),
                                })
                        },
                        "Heading"
                    }
                    button {
                        class: "dropdown-item",
                        onclick: move |_| {
                            props
                                .on_action
                                .call(EditorAction::AddNode {
                                    parent: vec![],
                                    index: 0,
                                    node_type: NewNodeType::List,
                                })
                        },
                        "List"
                    }
                    button {
                        class: "dropdown-item",
                        onclick: move |_| {
                            props
                                .on_action
                                .call(EditorAction::AddNode {
                                    parent: vec![],
                                    index: 0,
                                    node_type: NewNodeType::Group,
                                })
                        },
                        "Group"
                    }
                }
            }

            // Spacer
            div { class: "toolbar-spacer" }

            // Selection info
            if has_selection {
                span { class: "toolbar-info", "{selection_count} selected" }
            }

            // Clear selection button
            if has_selection {
                button {
                    class: "toolbar-btn toolbar-btn-text",
                    onclick: move |_| props.on_action.call(EditorAction::ClearSelection),
                    "Clear"
                }
            }
        }
    }
}
