use blueprint::{AemNode, convert_to_aem};
use dioxus::prelude::*;

use crate::models::DocumentEnvelope;

/// Wrapper for AemNode that implements PartialEq (always false to force re-render).
#[derive(Clone)]
struct AemNodeWrapper(AemNode);

impl PartialEq for AemNodeWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

/// Wrapper for DocumentEnvelope that implements PartialEq (always false).
#[derive(Clone)]
pub struct AemPreviewEnvelope(pub DocumentEnvelope);

impl PartialEq for AemPreviewEnvelope {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

#[derive(Clone, PartialEq, Props)]
pub struct AemPreviewProps {
    pub envelope: AemPreviewEnvelope,
    pub profile: Option<String>,
    pub on_close: EventHandler<()>,
}

#[component]
pub fn AemPreview(props: AemPreviewProps) -> Element {
    let envelope = &props.envelope.0;
    let profile = &props.profile;

    // Try to build the AEM tree
    let aem_tree = build_aem_tree(envelope, profile.as_deref());

    rsx! {
        div { class: "aem-preview-page",
            div { class: "aem-preview-header",
                h2 { "AEM Structure Preview" }
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| props.on_close.call(()),
                    "✕ Close"
                }
            }
            div { class: "aem-preview-legend",
                span { class: "legend-item legend-panel", "Panel" }
                span { class: "legend-item legend-repeatable", "Repeatable" }
                span { class: "legend-item legend-fragment", "Fragment" }
                span { class: "legend-item legend-field", "Field" }
            }
            div { class: "aem-preview-content",
                match aem_tree {
                    Some(root) => rsx! {
                        AemNodeBox { node: AemNodeWrapper(root) }
                    },
                    None => rsx! {
                        p { class: "aem-preview-error", "No AEM configuration available for this profile." }
                    },
                }
            }
        }
    }
}

fn build_aem_tree(envelope: &DocumentEnvelope, profile: Option<&str>) -> Option<AemNode> {
    let profile_name = profile?;
    if !blueprint::has_aem_config(profile_name) {
        return None;
    }
    let aem_config = blueprint::load_aem_config(profile_name, &envelope.context).ok()?;
    let root = convert_to_aem(&envelope.content, &aem_config);
    Some(root)
}

// ── Recursive node renderer ─────────────────────────────────────────────────

#[derive(Clone, PartialEq, Props)]
struct AemNodeBoxProps {
    node: AemNodeWrapper,
}

#[component]
fn AemNodeBox(props: AemNodeBoxProps) -> Element {
    let node = &props.node.0;
    let (css_class, label, children) = classify_node(node);

    rsx! {
        div { class: "aem-box {css_class}",
            span { class: "aem-box-label", "{label}" }
            if !children.is_empty() {
                div { class: "aem-box-children",
                    for child in children {
                        AemNodeBox { node: AemNodeWrapper(child.clone()) }
                    }
                }
            }
        }
    }
}

fn label_or_name(label: &str, name: &str) -> String {
    if label.is_empty() {
        name.to_owned()
    } else {
        label.to_owned()
    }
}

fn classify_node(node: &AemNode) -> (&'static str, String, Vec<AemNode>) {
    match node {
        AemNode::Root { title, children } => ("aem-root", title.clone(), children.clone()),
        AemNode::Panel {
            name,
            title,
            children,
            ..
        } => ("aem-panel", label_or_name(title, name), children.clone()),
        AemNode::Repeatable {
            name,
            title,
            children,
            min_occur,
            max_occur,
            ..
        } => {
            let label = format!(
                "{} [{min_occur}..{max_occur}]",
                if title.is_empty() { name } else { title }
            );
            ("aem-repeatable", label, children.clone())
        }
        AemNode::Fragment { name, frag_ref, .. } => {
            let short_ref = frag_ref.rsplit('/').next().unwrap_or(frag_ref);
            let label = format!("{name} → {short_ref}");
            ("aem-fragment", label, vec![])
        }
        AemNode::TextField { name, label, .. } => (
            "aem-field aem-textfield",
            label_or_name(label, name),
            vec![],
        ),
        AemNode::NumberField { name, label, .. } => (
            "aem-field aem-numberfield",
            label_or_name(label, name),
            vec![],
        ),
        AemNode::DatePicker { name, label, .. } => (
            "aem-field aem-datepicker",
            label_or_name(label, name),
            vec![],
        ),
        AemNode::Dropdown { name, label, .. } => {
            ("aem-field aem-dropdown", label_or_name(label, name), vec![])
        }
        AemNode::Checkbox { name, .. } => ("aem-field aem-checkbox", name.clone(), vec![]),
        AemNode::RadioButton { name, label, .. } => {
            ("aem-field aem-radio", label_or_name(label, name), vec![])
        }
        AemNode::TextDraw { name, .. } => ("aem-field aem-textdraw", name.clone(), vec![]),
        AemNode::TitleDraw { name, content, .. } => {
            let display = if content.is_empty() {
                name.clone()
            } else {
                content.clone()
            };
            ("aem-field aem-titledraw", display, vec![])
        }
        AemNode::Preface { .. } => ("aem-field aem-preface", "Preface".into(), vec![]),
        AemNode::Appendix { .. } => ("aem-field aem-appendix", "Appendix".into(), vec![]),
        AemNode::Footnote { name, content, .. } => {
            let display = if content.is_empty() {
                name.clone()
            } else {
                content.clone()
            };
            ("aem-field aem-footnote", display, vec![])
        }
    }
}
