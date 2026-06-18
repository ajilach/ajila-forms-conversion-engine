//! Recursive renderer for the AEM node tree.

use dioxus::prelude::*;

use blueprint::AemNode;

use super::metadata_editor::AemMetadataEditor;
use super::state::{AemEditorAction, AemPath, AemSelectionState, children_ref, is_container};
use super::text_editor::AemTextEditor;

/// Wrapper so an `AemNode` can be passed as a component prop (no `PartialEq`).
#[derive(Clone)]
pub struct AemNodeWrapper(pub AemNode);

impl PartialEq for AemNodeWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

/// Classify a node into (css class, badge text, display label).
fn classify(node: &AemNode) -> (&'static str, &'static str, String) {
    let label_or_name = |label: &str, name: &str| {
        if label.trim().is_empty() {
            name.to_string()
        } else {
            label.to_string()
        }
    };
    match node {
        AemNode::Root { title, .. } => ("aem-n-root", "ROOT", title.clone()),
        AemNode::Panel { name, title, .. } => {
            ("aem-n-panel", "Panel", label_or_name(title, name))
        }
        AemNode::Repeatable {
            name,
            title,
            min_occur,
            max_occur,
            ..
        } => (
            "aem-n-repeatable",
            "Repeatable",
            format!("{} [{min_occur}..{max_occur}]", label_or_name(title, name)),
        ),
        AemNode::TextField { name, label, .. } => {
            ("aem-n-field", "Text Field", label_or_name(label, name))
        }
        AemNode::NumberField { name, label, .. } => {
            ("aem-n-field", "Number Field", label_or_name(label, name))
        }
        AemNode::DatePicker { name, label, .. } => {
            ("aem-n-field", "Date Picker", label_or_name(label, name))
        }
        AemNode::Dropdown { name, label, .. } => {
            ("aem-n-field", "Dropdown", label_or_name(label, name))
        }
        AemNode::Checkbox { name, label, .. } => {
            ("aem-n-field", "Checkbox", label_or_name(label, name))
        }
        AemNode::RadioButton { name, label, .. } => {
            ("aem-n-field", "Radio Button", label_or_name(label, name))
        }
        AemNode::TextDraw { name, content, .. } => {
            ("aem-n-static", "Static Text", label_or_name(content, name))
        }
        AemNode::TitleDraw {
            name,
            content,
            heading_level,
            ..
        } => (
            "aem-n-static",
            "Heading",
            format!("H{heading_level}: {}", label_or_name(content, name)),
        ),
        AemNode::Fragment { name, frag_ref, .. } => {
            let short = frag_ref.rsplit('/').next().unwrap_or(frag_ref);
            ("aem-n-fragment", "Fragment", format!("{name} → {short}"))
        }
        AemNode::Preface { .. } => ("aem-n-static", "Preface", "Preface".into()),
        AemNode::Appendix { .. } => ("aem-n-static", "Appendix", "Appendix".into()),
        AemNode::FootnotePlaceholder { name, .. } => {
            ("aem-n-static", "Footnotes", format!("Footnotes ({name})"))
        }
        AemNode::Custom {
            name,
            label,
            template_key,
            ..
        } => (
            "aem-n-custom",
            "Custom",
            format!("[{template_key}] {}", label_or_name(label, name)),
        ),
    }
}

/// Whether a node exposes inline-editable text.
fn has_editable_text(node: &AemNode) -> bool {
    super::state::editable_text(node).is_some()
}

/// Whether a node has any editable metadata worth a properties panel.
/// Every non-root node carries at least a `name`, so all are editable.
fn has_metadata(node: &AemNode) -> bool {
    !matches!(node, AemNode::Root { .. })
}

#[derive(Clone, PartialEq, Props)]
pub struct AemNodeItemProps {
    pub node: AemNodeWrapper,
    pub path: AemPath,
    pub selection: AemSelectionState,
    pub depth: usize,
    pub on_action: EventHandler<AemEditorAction>,
}

/// Render a single node (header + optional editors + children).
#[component]
pub fn AemNodeItem(props: AemNodeItemProps) -> Element {
    let node = &props.node.0;
    let path = props.path.clone();
    let (css_class, badge, label) = classify(node);
    let is_selected = props.selection.is_selected(&path);
    let is_editing = props.selection.is_editing(&path);
    let is_editing_meta = props.selection.is_editing_metadata(&path);
    let container = is_container(node);
    let child_count = children_ref(node).map(|c| c.len()).unwrap_or(0);

    let mut expanded = use_signal(|| true);

    let on_action = props.on_action;
    let path_for_select = path.clone();
    let path_for_label = path.clone();
    let path_for_edit = path.clone();
    let path_for_meta = path.clone();

    rsx! {
        div {
            class: if is_selected { "aem-node-item {css_class} selected" } else { "aem-node-item {css_class}" },
            style: "margin-left: {props.depth * 16}px;",

            div { class: "aem-node-header",
                if container && child_count > 0 {
                    button {
                        class: "aem-node-toggle",
                        onclick: move |_| {
                            let cur = *expanded.read();
                            expanded.set(!cur);
                        },
                        if *expanded.read() { "▾" } else { "▸" }
                    }
                }
                input {
                    r#type: "checkbox",
                    checked: is_selected,
                    onclick: move |_| on_action.call(AemEditorAction::ToggleSelection(path_for_select.clone())),
                }
                span { class: "aem-node-badge", "{badge}" }
                span {
                    class: "aem-node-label",
                    onclick: move |_| on_action.call(AemEditorAction::SelectSingle(path_for_label.clone())),
                    "{label}"
                }
                if child_count > 0 {
                    span { class: "aem-node-count", "{child_count}" }
                }
                div { class: "aem-node-actions",
                    if has_editable_text(node) {
                        button {
                            class: "aem-node-btn",
                            title: "Edit text",
                            onclick: move |_| on_action.call(AemEditorAction::StartEditing(path_for_edit.clone())),
                            "✎"
                        }
                    }
                    if has_metadata(node) {
                        button {
                            class: "aem-node-btn",
                            title: "Edit properties",
                            onclick: move |_| on_action.call(AemEditorAction::StartEditingMetadata(path_for_meta.clone())),
                            "⚙"
                        }
                    }
                }
            }

            if is_editing && has_editable_text(node) {
                AemTextEditor {
                    initial: super::state::editable_text(node).unwrap_or_default(),
                    path: path.clone(),
                    uuid: super::state::node_uuid(node),
                    on_action,
                }
            }

            if is_editing_meta && has_metadata(node) {
                AemMetadataEditor {
                    node: AemNodeWrapper(node.clone()),
                    path: path.clone(),
                    on_action,
                }
            }

            if container && *expanded.read() {
                div { class: "aem-node-children",
                    if let Some(children) = children_ref(node) {
                        for (i, child) in children.iter().enumerate() {
                            {
                                let mut child_path = path.clone();
                                child_path.push(i);
                                rsx! {
                                    AemNodeItem {
                                        key: "{i}",
                                        node: AemNodeWrapper(child.clone()),
                                        path: child_path,
                                        selection: props.selection.clone(),
                                        depth: props.depth + 1,
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
