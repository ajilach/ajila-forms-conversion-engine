//! Toolbar for the AEM node editor.

use dioxus::prelude::*;

use super::state::{AemConvertTarget, AemEditorAction, NewAemNodeType};
use crate::components::spinner::Spinner;

#[derive(Clone, PartialEq, Props)]
pub struct AemToolbarProps {
    pub selection_count: usize,
    pub node_count: usize,
    pub can_move_up: bool,
    pub can_move_down: bool,
    pub can_indent: bool,
    pub can_outdent: bool,
    /// Parent path (as child indices) to add new nodes under, plus a label.
    pub add_target: Option<(Vec<usize>, String)>,
    pub has_images: bool,
    #[props(default = false)]
    pub is_smart_edit_loading: bool,
    pub has_connection: bool,
    pub on_action: EventHandler<AemEditorAction>,
}

const ADD_KINDS: &[(NewAemNodeType, &str)] = &[
    (NewAemNodeType::Panel, "Panel"),
    (NewAemNodeType::Repeatable, "Repeatable"),
    (NewAemNodeType::TextField, "Text Field"),
    (NewAemNodeType::NumberField, "Number Field"),
    (NewAemNodeType::DatePicker, "Date Picker"),
    (NewAemNodeType::Dropdown, "Dropdown"),
    (NewAemNodeType::RadioButton, "Radio Button"),
    (NewAemNodeType::Checkbox, "Checkbox"),
    (NewAemNodeType::TextDraw, "Text Draw"),
    (NewAemNodeType::TitleDraw, "Title Draw"),
];

const CONVERT_TARGETS: &[(AemConvertTarget, &str)] = &[
    (AemConvertTarget::TextField, "Text Field"),
    (AemConvertTarget::NumberField, "Number Field"),
    (AemConvertTarget::DatePicker, "Date Picker"),
    (AemConvertTarget::Dropdown, "Dropdown"),
    (AemConvertTarget::RadioButton, "Radio Button"),
    (AemConvertTarget::Checkbox, "Checkbox"),
    (AemConvertTarget::TextDraw, "Text Draw"),
    (AemConvertTarget::TitleDraw, "Title Draw"),
];

#[component]
pub fn AemToolbar(props: AemToolbarProps) -> Element {
    let has_selection = props.selection_count > 0;
    let on_action = props.on_action;
    let add_target = props.add_target.clone();

    rsx! {
        div { class: "editor-toolbar",
            button {
                class: "toolbar-btn toolbar-btn-danger",
                disabled: !has_selection,
                onclick: move |_| on_action.call(AemEditorAction::DeleteSelected),
                span { class: "toolbar-icon", "✕" }
                span { class: "toolbar-label", "Delete" }
            }
            button {
                class: "toolbar-btn",
                disabled: !has_selection,
                onclick: move |_| on_action.call(AemEditorAction::DuplicateSelected),
                span { class: "toolbar-icon", "⧉" }
                span { class: "toolbar-label", "Duplicate" }
            }

            div { class: "toolbar-separator" }

            button {
                class: "toolbar-btn",
                disabled: !props.can_move_up,
                onclick: move |_| on_action.call(AemEditorAction::MoveUp),
                span { class: "toolbar-icon", "↑" }
                span { class: "toolbar-label", "Up" }
            }
            button {
                class: "toolbar-btn",
                disabled: !props.can_move_down,
                onclick: move |_| on_action.call(AemEditorAction::MoveDown),
                span { class: "toolbar-icon", "↓" }
                span { class: "toolbar-label", "Down" }
            }
            button {
                class: "toolbar-btn",
                disabled: !props.can_indent,
                onclick: move |_| on_action.call(AemEditorAction::Indent),
                span { class: "toolbar-icon", "→" }
                span { class: "toolbar-label", "Indent" }
            }
            button {
                class: "toolbar-btn",
                disabled: !props.can_outdent,
                onclick: move |_| on_action.call(AemEditorAction::Outdent),
                span { class: "toolbar-icon", "←" }
                span { class: "toolbar-label", "Outdent" }
            }

            div { class: "toolbar-separator" }

            // Add
            div { class: if add_target.is_some() { "toolbar-dropdown" } else { "toolbar-dropdown toolbar-dropdown-disabled" },
                button {
                    class: "toolbar-btn",
                    disabled: add_target.is_none(),
                    span { class: "toolbar-icon", "+" }
                    span { class: "toolbar-label", "Add" }
                    span { class: "toolbar-caret", "▾" }
                }
                if let Some((parent, label)) = add_target.clone() {
                    div { class: "toolbar-dropdown-menu",
                        div { class: "dropdown-header", "Into: {label}" }
                        for (kind, name) in ADD_KINDS.iter().copied() {
                            {
                                let parent = parent.clone();
                                rsx! {
                                    button {
                                        class: "dropdown-item",
                                        onclick: move |_| on_action.call(AemEditorAction::AddNode { parent: parent.clone(), kind }),
                                        {name}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Convert
            div { class: if props.selection_count == 1 { "toolbar-dropdown" } else { "toolbar-dropdown toolbar-dropdown-disabled" },
                button {
                    class: "toolbar-btn",
                    disabled: props.selection_count != 1,
                    span { class: "toolbar-icon", "⟲" }
                    span { class: "toolbar-label", "Convert To" }
                    span { class: "toolbar-caret", "▾" }
                }
                if props.selection_count == 1 {
                    div { class: "toolbar-dropdown-menu",
                        for (target, name) in CONVERT_TARGETS.iter().copied() {
                            button {
                                class: "dropdown-item",
                                onclick: move |_| on_action.call(AemEditorAction::ConvertSelected(target)),
                                {name}
                            }
                        }
                    }
                }
            }

            div { class: "toolbar-separator" }

            // Smart AEM Edit
            button {
                class: "toolbar-btn toolbar-btn-smart",
                disabled: props.is_smart_edit_loading || !props.has_images,
                title: if props.has_images { "AI-assisted editing of the AEM structure" } else { "No rendered images available for Smart AEM Edit" },
                onclick: move |_| on_action.call(AemEditorAction::SmartAemEdit),
                if props.is_smart_edit_loading {
                    Spinner { size: "sm" }
                } else {
                    span { class: "toolbar-icon", "✦" }
                }
                span { class: "toolbar-label", "Smart AEM Edit" }
            }

            // Upload to AEM
            button {
                class: "toolbar-btn",
                disabled: !props.has_connection,
                title: if props.has_connection { "Upload the current package to AEM" } else { "Add a [connection] block to this profile's aem/config.toml" },
                onclick: move |_| on_action.call(AemEditorAction::UploadToAem),
                span { class: "toolbar-icon", "☁" }
                span { class: "toolbar-label", "Upload to AEM" }
            }

            div { class: "toolbar-spacer" }

            if has_selection {
                span { class: "toolbar-info", "{props.selection_count} selected" }
                button {
                    class: "toolbar-btn toolbar-btn-text",
                    onclick: move |_| on_action.call(AemEditorAction::ClearSelection),
                    "Clear"
                }
            } else {
                button {
                    class: "toolbar-btn toolbar-btn-text",
                    disabled: props.node_count == 0,
                    onclick: move |_| on_action.call(AemEditorAction::SelectAll),
                    "Select All"
                }
            }
        }
    }
}
