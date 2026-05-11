//! Smart edit modal component.
//!
//! Provides a modal dialog where the user enters an instruction,
//! sees a loading spinner while AI processes, then reviews the
//! suggested changes before accepting or rejecting them.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;

use blueprint::StructuredNode;

use super::node_renderer::{NodeRenderer, NodesWrapper};
use super::smart_edit;
use super::state::{EditorAction, PathSegment, SelectionState};

/// Possible states of the smart edit flow.
#[derive(Clone, Debug)]
pub enum SmartEditPhase {
    /// Waiting for the user to start the run.
    Ready,
    /// AI is processing.
    Loading,
    /// AI returned a response (may or may not parse into nodes).
    Preview {
        raw_response: String,
        parsed_nodes: Option<Vec<StructuredNode>>,
    },
    /// An error occurred.
    Error(String),
}

/// Properties for the smart edit modal.
#[derive(Clone, Props)]
pub struct SmartEditModalProps {
    /// Root-level indices of the selected nodes.
    pub selected_indices: Vec<usize>,
    /// Full document content (needed for serialisation).
    pub content: Vec<StructuredNode>,
    /// Plain render images (label → base64 PNG).
    pub plain_images: HashMap<String, String>,
    /// Called when the user accepts the suggested nodes.
    pub on_accept: EventHandler<Vec<StructuredNode>>,
    /// Called when the user cancels.
    pub on_cancel: EventHandler<()>,
}

impl PartialEq for SmartEditModalProps {
    fn eq(&self, other: &Self) -> bool {
        // `content` is intentionally excluded because StructuredNode does not
        // implement PartialEq.  The modal is always shown fresh via the
        // `show_smart_edit` signal, so stale-equality is not a concern.
        self.selected_indices == other.selected_indices
            && self.plain_images == other.plain_images
            && self.on_accept == other.on_accept
            && self.on_cancel == other.on_cancel
    }
}

#[component]
pub fn SmartEditModal(props: SmartEditModalProps) -> Element {
    let mut phase = use_signal(|| SmartEditPhase::Ready);

    // Submit handler: kick off the AI call
    let submit = {
        let content = props.content.clone();
        let selected_indices = props.selected_indices.clone();
        let plain_images = props.plain_images.clone();
        move |_| {
            phase.set(SmartEditPhase::Loading);

            let content = content.clone();
            let selected_indices = selected_indices.clone();
            let plain_images = plain_images.clone();
            spawn(async move {
                match smart_edit::run_smart_edit(&content, &selected_indices, &plain_images).await {
                    Ok(response) => {
                        let parsed = smart_edit::parse_response_nodes(&response).ok();
                        phase.set(SmartEditPhase::Preview {
                            raw_response: response,
                            parsed_nodes: parsed,
                        });
                    }
                    Err(e) => {
                        phase.set(SmartEditPhase::Error(e));
                    }
                }
            });
        }
    };

    let on_accept = props.on_accept;
    let on_cancel = props.on_cancel;

    rsx! {
        div { class: "smart-edit-overlay", onclick: move |_| on_cancel.call(()),
            div {
                class: "smart-edit-modal",
                onclick: move |evt| evt.stop_propagation(),

                // Header
                div { class: "smart-edit-header",
                    h3 { "✨ Smart Edit" }
                    button {
                        class: "modal-close-btn",
                        onclick: move |_| on_cancel.call(()),
                        "×"
                    }
                }

                // Body
                div { class: "smart-edit-body",
                    match phase.read().clone() {
                        SmartEditPhase::Ready => rsx! {
                            p { class: "smart-edit-hint",
                                "Smart Edit uses a built-in prompt that preserves the form's existing text and structure while improving layout quality and multilingual alignment."
                            }
                            div { class: "smart-edit-actions",
                                button {
                                    class: "editor-btn editor-btn-secondary",
                                    onclick: move |_| on_cancel.call(()),
                                    "Cancel"
                                }
                                button {
                                    class: "editor-btn editor-btn-primary",
                                    onclick: {
                                        let mut submit = submit.clone();
                                        move |_| submit(())
                                    },
                                    "Run Smart Edit"
                                }
                            }
                        },
                        SmartEditPhase::Loading => rsx! {
                            div { class: "smart-edit-loading",
                                div { class: "smart-edit-spinner" }
                                p { "Copilot is thinking…" }
                            }
                        },
                        SmartEditPhase::Preview { raw_response, parsed_nodes } => rsx! {
                            if let Some(ref nodes) = parsed_nodes {
                                {
                                    // Compute which nodes changed
                                    let original_nodes: Vec<StructuredNode> = props
                                        .selected_indices
                                        .iter()
                                        .filter_map(|&i| props.content.get(i).cloned())
                                        .collect();
                                    let changed_indices =
                                        smart_edit::compute_changed_indices(&original_nodes, nodes);
                                    let highlight: HashSet<_> = changed_indices
                                        .into_iter()
                                        .map(|i| vec![PathSegment::Child(i)])
                                        .collect();
                                    let changed_count = highlight.len();

                                    rsx! {
                                        p { class: "smart-edit-hint smart-edit-success",
                                            "✓ Copilot suggested {nodes.len()} node(s), {changed_count} changed. Review below."
                                        }
                                        div { class: "smart-edit-tree-preview",
                                            NodeRenderer {
                                                nodes: NodesWrapper(nodes.clone()),
                                                selection: SelectionState::new(),
                                                languages: Vec::new(),
                                                highlight,
                                                on_action: move |_: EditorAction| {},
                                            }
                                        }
                                    }
                                }
                            } else {
                                p { class: "smart-edit-hint smart-edit-warning",
                                    "⚠ Could not parse structured nodes from the response. Raw output shown below."
                                }
                                pre { class: "smart-edit-response", "{raw_response}" }
                            }
                            div { class: "smart-edit-actions",
                                button {
                                    class: "editor-btn editor-btn-secondary",
                                    onclick: move |_| phase.set(SmartEditPhase::Ready),
                                    "← Try Again"
                                }
                                if let Some(nodes) = parsed_nodes {
                                    button {
                                        class: "editor-btn editor-btn-primary",
                                        onclick: move |_| on_accept.call(nodes.clone()),
                                        "Apply Changes"
                                    }
                                }
                            }
                        },
                        SmartEditPhase::Error(msg) => rsx! {
                            p { class: "smart-edit-hint smart-edit-error", "Error: {msg}" }
                            div { class: "smart-edit-actions",
                                button {
                                    class: "editor-btn editor-btn-secondary",
                                    onclick: move |_| phase.set(SmartEditPhase::Ready),
                                    "← Back"
                                }
                                button {
                                    class: "editor-btn editor-btn-secondary",
                                    onclick: move |_| on_cancel.call(()),
                                    "Close"
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}
