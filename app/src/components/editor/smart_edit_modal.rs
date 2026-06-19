//! Smart edit modal component.
//!
//! Provides a modal dialog where the user enters an instruction,
//! sees a loading spinner while AI processes, then reviews the
//! suggested changes before accepting or rejecting them.

use std::collections::HashMap;

use dioxus::prelude::*;

use blueprint::StructuredNode;

use super::smart_edit;

/// Possible states of the smart edit flow.
#[derive(Clone, Debug)]
pub enum SmartEditPhase {
    /// Waiting for the user to start the run.
    Ready,
    /// AI is processing.
    Loading,
    /// AI returned a response (may or may not parse into nodes).
    Preview {
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
    /// Source PDF bytes (filename → bytes) for the full Smart Edit tool set.
    pub source_pdfs: Vec<(String, Vec<u8>)>,
    /// Anthropic API key for Smart Edit.
    pub api_key: String,
    /// Model identifier for Smart Edit.
    pub model: String,
    /// Called when the user accepts the suggested nodes.
    pub on_accept: EventHandler<Vec<StructuredNode>>,
    /// Called when the user cancels.
    pub on_cancel: EventHandler<()>,
}

impl PartialEq for SmartEditModalProps {
    fn eq(&self, other: &Self) -> bool {
        self.selected_indices == other.selected_indices
            && self.plain_images == other.plain_images
            && self.source_pdfs == other.source_pdfs
            && self.api_key == other.api_key
            && self.model == other.model
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
        let source_pdfs = props.source_pdfs.clone();
        let api_key = props.api_key.clone();
        let model = props.model.clone();
        move |_| {
            phase.set(SmartEditPhase::Loading);

            let content = content.clone();
            let selected_indices = selected_indices.clone();
            let plain_images = plain_images.clone();
            let source_pdfs = source_pdfs.clone();
            let api_key = api_key.clone();
            let model = model.clone();
            spawn(async move {
                match smart_edit::run_smart_edit(
                    &content,
                    &selected_indices,
                    &plain_images,
                    &source_pdfs,
                    &api_key,
                    &model,
                )
                .await
                {
                    Ok(result) => {
                        let parsed = if result.nodes.is_empty() {
                            None
                        } else {
                            Some(result.nodes)
                        };
                        phase.set(SmartEditPhase::Preview {
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
                    h3 {
                        span {
                            class: "smart-edit-header-icon",
                            dangerous_inner_html: r#"<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"/><path d="M20 3v4"/><path d="M22 5h-4"/></svg>"#,
                        }
                        " Smart Edit"
                    }
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
                        SmartEditPhase::Preview { parsed_nodes } => rsx! {
                            if let Some(ref nodes) = parsed_nodes {
                                p { class: "smart-edit-hint smart-edit-success",
                                    "✓ Copilot suggested {nodes.len()} node(s). Accept or try again."
                                }
                            } else {
                                p { class: "smart-edit-hint smart-edit-warning",
                                    "⚠ Could not parse structured nodes from the response."
                                }
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
