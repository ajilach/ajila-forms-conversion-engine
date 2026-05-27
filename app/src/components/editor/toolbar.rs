//! Editor toolbar component.
//!
//! Provides action buttons for merge, delete, and add operations.

use dioxus::prelude::*;

use super::state::{AddOption, ConvertTarget, EditorAction, SelectionState};
use crate::components::spinner::Spinner;

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
    /// Whether the selected node can be indented (moved into adjacent container).
    pub can_indent: bool,
    /// Whether the selected node can be outdented (moved out of its container).
    pub can_outdent: bool,
    /// Available conversion targets for current selection.
    pub available_conversions: Vec<ConvertTarget>,
    /// Available add options for current selection.
    pub add_options: Vec<AddOption>,
    /// Whether plain images are available (needed for Smart Edit).
    pub has_images: bool,
    /// Whether a Smart Edit call is currently in progress.
    #[props(default = false)]
    pub is_smart_edit_loading: bool,
    /// Total number of root-level nodes (for Select All).
    pub node_count: usize,
    /// Callback when an action is triggered.
    pub on_action: EventHandler<EditorAction>,
}

/// Toolbar component with action buttons.
#[component]
pub fn EditorToolbar(props: ToolbarProps) -> Element {
    let selection_count = props.selection.count();
    let has_selection = selection_count > 0;
    let can_merge = props.can_merge && selection_count >= 2;
    let has_conversions = !props.available_conversions.is_empty();

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

            // Indent button (move into container)
            button {
                class: "toolbar-btn",
                disabled: !props.can_indent,
                title: if props.can_indent { "Move into the container above" } else if !has_selection { "Select a node to indent" } else { "Cannot indent (no container above)" },
                onclick: move |_| props.on_action.call(EditorAction::Indent),
                span { class: "toolbar-icon", "→" }
                span { class: "toolbar-label", "Indent" }
            }

            // Outdent button (move out of container)
            button {
                class: "toolbar-btn",
                disabled: !props.can_outdent,
                title: if props.can_outdent { "Move out of container" } else if !has_selection { "Select a node to outdent" } else { "Cannot outdent (not inside a container)" },
                onclick: move |_| props.on_action.call(EditorAction::Outdent),
                span { class: "toolbar-icon", "←" }
                span { class: "toolbar-label", "Outdent" }
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
                    for option in props.add_options.iter() {
                        {
                            let parent = option.parent.clone();
                            let index = option.index;
                            let node_type = option.node_type.clone();
                            let label = option.label;
                            rsx! {
                                button {
                                    class: "dropdown-item",
                                    onclick: move |_| {
                                        props
                                            .on_action
                                            .call(EditorAction::AddNode {
                                                parent: parent.clone(),
                                                index,
                                                node_type: node_type.clone(),
                                            })
                                    },
                                    {label}
                                }
                            }
                        }
                    }
                }
            }

            // Convert To dropdown
            div { class: if has_conversions { "toolbar-dropdown" } else { "toolbar-dropdown toolbar-dropdown-disabled" },
                button {
                    class: "toolbar-btn",
                    disabled: !has_conversions,
                    title: if has_conversions { "Convert selected element(s)" } else { "Select element(s) to convert" },
                    span { class: "toolbar-icon", "⟲" }
                    span { class: "toolbar-label", "Convert To" }
                    span { class: "toolbar-caret", "▾" }
                }
                if has_conversions {
                    div { class: "toolbar-dropdown-menu",
                        for target in props.available_conversions.iter() {
                            {
                                let target = *target;
                                rsx! {
                                    button {
                                        class: "dropdown-item",
                                        onclick: move |_| { props.on_action.call(EditorAction::ConvertSelected(target)) },
                                        {conversion_label(target)}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Separator
            div { class: "toolbar-separator" }

            // Smart Edit button
            button {
                class: "toolbar-btn toolbar-btn-smart",
                disabled: props.is_smart_edit_loading,
                title: "AI-assisted editing via GitHub Copilot",
                onclick: move |_| props.on_action.call(EditorAction::SmartEdit),
                if props.is_smart_edit_loading {
                    Spinner { size: "sm" }
                } else {
                    span { class: "toolbar-icon", "✨" }
                }
                span { class: "toolbar-label", "Smart Edit" }
            }

            // Spacer
            div { class: "toolbar-spacer" }

            // Selection info
            if has_selection {
                span { class: "toolbar-info", "{selection_count} selected" }
            }

            // Select All / Clear toggle
            if has_selection {
                button {
                    class: "toolbar-btn toolbar-btn-text",
                    onclick: move |_| props.on_action.call(EditorAction::ClearSelection),
                    "Clear"
                }
            } else {
                button {
                    class: "toolbar-btn toolbar-btn-text",
                    disabled: props.node_count == 0,
                    onclick: move |_| props.on_action.call(EditorAction::SelectAll),
                    "Select All"
                }
            }
        }
    }
}

/// Get the display label for a conversion target.
fn conversion_label(target: ConvertTarget) -> &'static str {
    match target {
        ConvertTarget::Paragraph => "Paragraph",
        ConvertTarget::Paragraphs => "Paragraphs",
        ConvertTarget::Heading(_) => "Heading",
        ConvertTarget::List => "List",
        ConvertTarget::Field => "Field",
    }
}
