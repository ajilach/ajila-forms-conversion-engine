//! Simplified agent-mode UI: a single full-height flow that shows exactly one
//! phase at a time — upload → live narrative timeline → done — with no extra
//! buttons. The legacy stacked layout remains available behind a settings
//! toggle (`AppSettings::legacy_agent_ui`).

use dioxus::html::HasFileData;
use dioxus::prelude::*;

use super::file_upload::read_upload_files;
use super::spinner::Spinner;
use crate::models::{AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};

/// Which phase of the agent flow is currently shown.
#[derive(PartialEq)]
enum Phase {
    Upload,
    Running,
    Done,
}

/// Human-friendly duration, e.g. `"1m 18s"` or `"42s"`.
fn format_elapsed(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[component]
pub fn AgentFlow(
    processing_state: Signal<ProcessingState>,
    is_processing: Signal<bool>,
    profiles: Vec<String>,
    mut selected_profile: Signal<Option<String>>,
    /// Whether agent processing is available (an API key is configured).
    ai_available: bool,
    /// Start a fresh agent run from the uploaded files.
    on_ai_process: EventHandler<Vec<(String, Vec<u8>)>>,
    /// Re-run the agent in the same session with the user's feedback.
    on_feedback: EventHandler<String>,
    /// Discard the finished result and return to a clean upload state.
    on_reset: EventHandler<()>,
) -> Element {
    let mut uploaded_files = use_signal(Vec::<(String, Vec<u8>)>::new);
    let is_dragging = use_signal(|| false);
    let drag_depth = use_signal(|| 0usize);
    let mut feedback = use_signal(String::new);

    // Auto-select the first profile if none is chosen yet.
    if selected_profile.read().is_none()
        && let Some(first) = profiles.first()
    {
        selected_profile.set(Some(first.clone()));
    }

    // Keep the timeline pinned to the newest step as the agent works.
    use_effect(move || {
        let _ = processing_state.read().agent_steps.len();
        document::eval(
            r#"setTimeout(() => {
                const el = document.getElementById('agent-flow-end');
                if (el) el.scrollIntoView({ block: 'end' });
            }, 0);"#,
        );
    });

    let state = processing_state.read();
    let processing = *is_processing.read();
    let phase = if state.step == ProcessingStep::Complete {
        Phase::Done
    } else if processing || state.step != ProcessingStep::Idle {
        Phase::Running
    } else {
        Phase::Upload
    };

    rsx! {
        div { class: "agent-flow",
            match phase {
                Phase::Upload => rsx! {
                    UploadPhase {
                        profiles,
                        selected_profile,
                        ai_available,
                        uploaded_files,
                        is_dragging,
                        drag_depth,
                        on_start: move |files: Vec<(String, Vec<u8>)>| on_ai_process.call(files),
                    }
                },
                Phase::Running => rsx! {
                    RunningPhase { state: state.clone() }
                },
                Phase::Done => rsx! {
                    DonePhase {
                        elapsed: state.elapsed_secs,
                        aem_uploaded: state.aem_uploaded,
                        aem_form_path: state.aem_form_path.clone(),
                        feedback,
                        on_feedback: move |text: String| on_feedback.call(text),
                        on_new: move |_| {
                            uploaded_files.set(Vec::new());
                            feedback.set(String::new());
                            on_reset.call(());
                        },
                    }
                },
            }
        }
    }
}

#[component]
fn UploadPhase(
    profiles: Vec<String>,
    mut selected_profile: Signal<Option<String>>,
    ai_available: bool,
    mut uploaded_files: Signal<Vec<(String, Vec<u8>)>>,
    mut is_dragging: Signal<bool>,
    mut drag_depth: Signal<usize>,
    on_start: EventHandler<Vec<(String, Vec<u8>)>>,
) -> Element {
    let files = uploaded_files.read().clone();
    let has_pdf = files
        .iter()
        .any(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"));
    let start_disabled = files.is_empty() || !has_pdf || !ai_available;
    let start_title = if !ai_available {
        "Configure an API key in Settings to enable agent processing."
    } else if files.is_empty() {
        "Drop or choose a file to begin."
    } else if !has_pdf {
        "Upload at least one PDF to start the agent."
    } else {
        "Let the agent convert and upload the form."
    };

    rsx! {
        div { class: "agent-flow-center",
            div { class: "agent-flow-upload",

                if !profiles.is_empty() {
                    div { class: "profile-selector",
                        label { r#for: "agent-profile-select", "Profile" }
                        select {
                            id: "agent-profile-select",
                            onchange: move |evt: Event<FormData>| selected_profile.set(Some(evt.value())),
                            for name in profiles.iter() {
                                option {
                                    value: "{name}",
                                    selected: selected_profile.read().as_deref() == Some(name.as_str()),
                                    "{name}"
                                }
                            }
                        }
                    }
                }

                div {
                    class: if *is_dragging.read() { "upload-dropzone upload-dropzone-dragging agent-dropzone" } else { "upload-dropzone agent-dropzone" },
                    ondragenter: move |evt: Event<DragData>| {
                        evt.prevent_default();
                        let next = *drag_depth.read() + 1;
                        drag_depth.set(next);
                        is_dragging.set(true);
                    },
                    ondragover: move |evt: Event<DragData>| evt.prevent_default(),
                    ondragleave: move |evt: Event<DragData>| {
                        evt.prevent_default();
                        let next = (*drag_depth.read()).saturating_sub(1);
                        drag_depth.set(next);
                        if next == 0 {
                            is_dragging.set(false);
                        }
                    },
                    ondrop: move |evt: Event<DragData>| {
                        evt.prevent_default();
                        drag_depth.set(0);
                        is_dragging.set(false);
                        let dropped = evt.files();
                        async move {
                            let data = read_upload_files(dropped).await;
                            if !data.is_empty() {
                                uploaded_files.set(data);
                            }
                        }
                    },

                    h2 { "Drop a file to start the agent" }
                    p { class: "upload-hint",
                        "Upload a PDF form (optionally with an AEM package ZIP or structured JSON) and the agent takes it from here."
                    }

                    div { class: "upload-actions",
                        label {
                            class: "btn btn-secondary btn-sm",
                            r#for: "agent-file-input",
                            "Choose File"
                        }
                    }
                    input {
                        id: "agent-file-input",
                        class: "upload-input-hidden",
                        r#type: "file",
                        multiple: true,
                        accept: ".pdf,.zip,.json",
                        onchange: move |evt: Event<FormData>| {
                            let chosen = evt.files();
                            async move {
                                let data = read_upload_files(chosen).await;
                                if !data.is_empty() {
                                    uploaded_files.set(data);
                                }
                            }
                        },
                    }
                }

                if !files.is_empty() {
                    div { class: "agent-upload-selected",
                        ul { class: "file-list-compact",
                            for (name , _bytes) in files.iter() {
                                li { "{name}" }
                            }
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: start_disabled,
                            title: start_title,
                            onclick: {
                                let files = files.clone();
                                move |_| on_start.call(files.clone())
                            },
                            "Start"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RunningPhase(state: ProcessingState) -> Element {
    let current = state
        .agent_steps
        .iter()
        .rev()
        .find(|s| s.kind == AgentStepKind::Tool && s.status == AgentStepStatus::Running)
        .map(|s| s.label.clone());

    rsx! {
        div { class: "agent-flow-head",
            div { class: "agent-flow-col agent-flow-head-row",
                Spinner { size: "sm" }
                div { class: "agent-flow-head-text",
                    span { class: "agent-flow-title", "Agent is working" }
                    if let Some(label) = current.as_ref() {
                        span { class: "agent-flow-sub", "{label}" }
                    }
                }
            }
        }
        div { class: "agent-flow-timeline",
            div { class: "agent-flow-col",
                div { class: "af-timeline",
                    if state.agent_steps.is_empty() {
                        div { class: "af-thought", "Starting agent…" }
                    }
                    for (i , s) in state.agent_steps.iter().enumerate() {
                        {match s.kind {
                            AgentStepKind::Thought => rsx! {
                                div { key: "{i}", class: "af-thought", "{s.label}" }
                            },
                            AgentStepKind::Tool => rsx! {
                                div { key: "{i}", class: "af-tool",
                                    span { class: "af-node",
                                        {match s.status {
                                            AgentStepStatus::Running => rsx! { Spinner { size: "sm" } },
                                            AgentStepStatus::Done => rsx! { span { class: "af-ok", "✓" } },
                                            AgentStepStatus::Error => rsx! { span { class: "af-err", "✗" } },
                                        }}
                                    }
                                    div { class: "af-tool-body",
                                        span { class: "af-tool-name", "{s.label}" }
                                        if !s.detail.is_empty() {
                                            span { class: "af-tool-detail", "{s.detail}" }
                                        }
                                    }
                                }
                            },
                        }}
                    }

                    if let Some(error) = &state.error {
                        div { class: "progress-error",
                            strong { "Error: " }
                            "{error}"
                        }
                    }

                    div { id: "agent-flow-end" }
                }
            }
        }
    }
}

#[component]
fn DonePhase(
    elapsed: Option<u64>,
    aem_uploaded: bool,
    aem_form_path: Option<String>,
    mut feedback: Signal<String>,
    on_feedback: EventHandler<String>,
    on_new: EventHandler<()>,
) -> Element {
    let feedback_empty = feedback.read().trim().is_empty();

    rsx! {
        div { class: "agent-flow-center",
            div { class: "agent-flow-done",
                div { class: "af-check", "✓" }
                div { class: "af-done-title",
                    span { class: "af-done-finished", "Finished" }
                    if let Some(secs) = elapsed {
                        span { class: "af-done-time", "{format_elapsed(secs)}" }
                    }
                }

                if aem_uploaded {
                    div { class: "af-done-aem",
                        span { class: "af-done-aem-label", "Uploaded to AEM" }
                        if let Some(path) = aem_form_path.as_ref() {
                            span { class: "af-done-aem-path", "{path}" }
                        }
                    }
                } else {
                    div { class: "af-done-aem",
                        span { class: "af-done-aem-label", "Blueprint generated" }
                    }
                }

                div { class: "af-feedback",
                    label { class: "af-feedback-label", "Not quite right? Tell the agent what to change:" }
                    textarea {
                        class: "af-feedback-input",
                        rows: "3",
                        placeholder: "e.g. The phone number field should be optional.",
                        value: "{feedback}",
                        oninput: move |evt| feedback.set(evt.value()),
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: feedback_empty,
                        onclick: move |_| {
                            let text = feedback.read().trim().to_string();
                            if !text.is_empty() {
                                feedback.set(String::new());
                                on_feedback.call(text);
                            }
                        },
                        "Send feedback"
                    }
                }

                div { class: "af-done-actions",
                    button {
                        class: "btn btn-secondary btn-sm",
                        onclick: move |_| on_new.call(()),
                        "Convert new form"
                    }
                }
            }
        }
    }
}
