//! Single-page agent-mode UI: one status box that morphs through the whole
//! run — upload → live activity → done — without swapping screens. The activity
//! timeline is collapsed to its latest step by default and expands in place to
//! the full, scrollable history. Feedback lives inside the finished box. The
//! legacy stacked layout remains available behind a settings toggle
//! (`AppSettings::legacy_agent_ui`).

use dioxus::html::HasFileData;
use dioxus::prelude::*;

use super::file_upload::read_upload_files;
use super::spinner::Spinner;
use crate::models::{AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};
use crate::platform::download_file;

/// Which phase of the agent flow is currently shown.
#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Upload,
    Running,
    Done,
}

/// Lifecycle of the on-demand "Upload to AEM" action, surfaced inside the button.
#[derive(Clone, PartialEq)]
enum UploadState {
    Idle,
    Uploading,
    Success,
    Error(String),
}

/// Build a download filename like `forms-package-<code>.zip`, or
/// `forms-package.zip` when the form code is unknown.
fn package_filename(form_code: &Option<String>) -> String {
    match form_code {
        Some(code) => format!("forms-package-{code}.zip"),
        None => "forms-package.zip".to_string(),
    }
}

/// Human-friendly duration, e.g. `"1m 18s"` or `"42s"`.
fn format_elapsed(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Map a file name to a short extension badge: `(css_class, label)`.
fn ext_badge(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".pdf") {
        ("pdf", "PDF")
    } else if lower.ends_with(".zip") {
        ("zip", "ZIP")
    } else if lower.ends_with(".json") {
        ("json", "JSON")
    } else {
        ("", "FILE")
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
    /// AEM upload connection from settings, or `None` if not configured.
    aem_connection: Option<blueprint::AemConnection>,
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
    // Whether the activity timeline is expanded to its full history.
    let mut timeline_open = use_signal(|| false);

    // Auto-select the first profile if none is chosen yet.
    if selected_profile.read().is_none()
        && let Some(first) = profiles.first()
    {
        selected_profile.set(Some(first.clone()));
    }

    // Keep the timeline pinned to the newest step as the agent works (only
    // matters while the history is expanded).
    use_effect(move || {
        let _ = processing_state.read().agent_steps.len();
        if *timeline_open.read() {
            document::eval(
                r#"setTimeout(() => {
                    const el = document.getElementById('agent-flow-end');
                    if (el) el.scrollIntoView({ block: 'end' });
                }, 0);"#,
            );
        }
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
            div { class: "agent-single",
                div { class: "agent-page",
                    match phase {
                        Phase::Upload => rsx! {
                            UploadBox {
                                profiles,
                                selected_profile,
                                ai_available,
                                uploaded_files,
                                is_dragging,
                                drag_depth,
                                on_start: move |files: Vec<(String, Vec<u8>)>| on_ai_process.call(files),
                            }
                        },
                        Phase::Running | Phase::Done => rsx! {
                            RunBox {
                                phase,
                                state: state.clone(),
                                files: uploaded_files.read().clone(),
                                profile: selected_profile.read().clone(),
                                aem_connection: aem_connection.clone(),
                                timeline_open,
                                feedback,
                                on_feedback: move |text: String| on_feedback.call(text),
                                on_new: move |_| {
                                    uploaded_files.set(Vec::new());
                                    feedback.set(String::new());
                                    timeline_open.set(false);
                                    on_reset.call(());
                                },
                            }
                        },
                    }
                }
            }
        }
    }
}

/// The box in its initial state: profile, dropzone, selected files, Start.
#[component]
fn UploadBox(
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
        section { class: "ag-box",
            div { class: "ag-top",
                div { class: "ag-badge upload", "↑" }
                div { class: "ag-top-text",
                    h2 { class: "ag-title", "Convert a form" }
                    div { class: "ag-meta",
                        span { "Drop the files and the agent takes it from here." }
                    }
                }
            }

            div { class: "ag-phases",
                div { class: "ag-phase active",
                    span { class: "pn", "1" }
                    span { class: "pl", "Upload" }
                }
                div { class: "ag-pbar" }
                div { class: "ag-phase",
                    span { class: "pn", "2" }
                    span { class: "pl", "Convert" }
                }
                div { class: "ag-pbar" }
                div { class: "ag-phase",
                    span { class: "pn", "3" }
                    span { class: "pl", "Finish" }
                }
            }

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

                div { class: "dz-icon", "↑" }
                h3 { "Drop files to start the agent" }
                p { class: "upload-hint", "Upload the PDF forms here." }
                div { class: "upload-actions",
                    label {
                        class: "btn btn-secondary btn-sm",
                        r#for: "agent-file-input",
                        "Choose Files"
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
                ul { class: "file-list-compact",
                    for (name , _bytes) in files.iter() {
                        li { "{name}" }
                    }
                }
                div { class: "ag-up-actions",
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

/// The box while the agent runs and once it finishes: header, phase rail,
/// source files, the collapsible activity timeline, and (when done) feedback.
#[component]
fn RunBox(
    phase: Phase,
    state: ProcessingState,
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    aem_connection: Option<blueprint::AemConnection>,
    mut timeline_open: Signal<bool>,
    mut feedback: Signal<String>,
    on_feedback: EventHandler<String>,
    on_new: EventHandler<()>,
) -> Element {
    let done = phase == Phase::Done;
    let open = *timeline_open.read();
    let steps = &state.agent_steps;
    let latest = steps.last();
    let feedback_empty = feedback.read().trim().is_empty();
    // Lifecycle of the on-demand AEM upload, reported inside the button.
    let mut upload_state = use_signal(|| UploadState::Idle);

    rsx! {
        section { class: if done { "ag-box done" } else { "ag-box" },
            // ---- Header ----
            div { class: "ag-top",
                if done {
                    div { class: "ag-badge ok", "✓" }
                } else {
                    div { class: "ag-badge run",
                        Spinner { size: "md" }
                    }
                }
                div { class: "ag-top-text",
                    h2 { class: "ag-title",
                        if done {
                            "Finished"
                        } else {
                            "Agent is working"
                        }
                    }
                    div { class: "ag-meta",
                        if let Some(p) = profile.as_ref() {
                            span {
                                "Profile "
                                b { "{p}" }
                            }
                        }
                        if done {
                            if let Some(secs) = state.elapsed_secs {
                                span {
                                    "in "
                                    b { "{format_elapsed(secs)}" }
                                }
                            }
                        }
                    }
                }
                if done {
                    div { class: "ag-actions",
                        button {
                            class: "btn btn-secondary btn-sm",
                            onclick: move |_| on_new.call(()),
                            "↻ New form"
                        }
                    }
                }
            }

            // ---- Phase rail ----
            div { class: "ag-phases",
                div { class: "ag-phase done",
                    span { class: "pn", "✓" }
                    span { class: "pl", "Upload" }
                }
                div { class: "ag-pbar done" }
                if done {
                    div { class: "ag-phase done",
                        span { class: "pn", "✓" }
                        span { class: "pl", "Convert" }
                    }
                    div { class: "ag-pbar done" }
                    div { class: "ag-phase done",
                        span { class: "pn", "✓" }
                        span { class: "pl", "Finish" }
                    }
                } else {
                    div { class: "ag-phase active",
                        span { class: "pn", "●" }
                        span { class: "pl", "Convert" }
                    }
                    div { class: "ag-pbar" }
                    div { class: "ag-phase",
                        span { class: "pn", "3" }
                        span { class: "pl", "Finish" }
                    }
                }
            }

            // ---- Source files ----
            if !files.is_empty() {
                div { class: "ag-files",
                    for (name , _bytes) in files.iter() {
                        {
                            let (cls, label) = ext_badge(name);
                            rsx! {
                                span { class: "ag-file",
                                    span { class: "ag-file-ext {cls}", "{label}" }
                                    "{name}"
                                }
                            }
                        }
                    }
                }
            }

            // ---- Collapsible activity timeline ----
            div { class: "ag-tl",
                button {
                    class: "ag-tl-bar",
                    onclick: move |_| {
                        let next = !*timeline_open.read();
                        timeline_open.set(next);
                    },
                    if open {
                        span { class: "ag-tl-title", "Activity · {steps.len()} steps" }
                    } else {
                        // Collapsed: show only the latest step.
                        match latest {
                            Some(s) if s.kind == AgentStepKind::Tool => rsx! {
                                span { class: "ag-tl-dot {tl_dot_class(&s.status)}", {tl_dot_inner(&s.status)} }
                                span { class: "ag-tl-latest",
                                    span { class: "nm", "{s.label}" }
                                    if !s.detail.is_empty() {
                                        span { class: "dt", "{s.detail}" }
                                    }
                                }
                            },
                            Some(s) => rsx! {
                                span { class: "ag-tl-dot" }
                                span { class: "ag-tl-latest",
                                    span { class: "nm-thought", "{s.label}" }
                                }
                            },
                            None => rsx! {
                                span { class: "ag-tl-dot",
                                    Spinner { size: "sm" }
                                }
                                span { class: "ag-tl-latest",
                                    span { class: "nm", "Starting agent…" }
                                }
                            },
                        }
                    }
                    span { class: "ag-tl-chevron",
                        if open {
                            "Collapse"
                        } else {
                            "Show full history"
                        }
                        span { class: if open { "chev chev-open" } else { "chev" }, "▾" }
                    }
                }
                if open {
                    div { class: "ag-tl-full",
                        div { class: "af-timeline",
                            if steps.is_empty() {
                                div { class: "af-thought", "Starting agent…" }
                            }
                            for (i , s) in steps.iter().enumerate() {
                                {
                                    match s.kind {
                                        AgentStepKind::Thought => rsx! {
                                            div { key: "{i}", class: "af-thought", "{s.label}" }
                                        },
                                        AgentStepKind::Tool => rsx! {
                                            div { key: "{i}", class: "af-tool",
                                                span { class: "af-node",
                                                    {
                                                        match s.status {
                                                            AgentStepStatus::Running => rsx! {
                                                                Spinner { size: "sm" }
                                                            },
                                                            AgentStepStatus::Done => rsx! {
                                                                span { class: "af-ok", "✓" }
                                                            },
                                                            AgentStepStatus::Error => rsx! {
                                                                span { class: "af-err", "✗" }
                                                            },
                                                        }
                                                    }
                                                }
                                                div { class: "af-tool-body",
                                                    span { class: "af-tool-name", "{s.label}" }
                                                    if !s.detail.is_empty() {
                                                        span { class: "af-tool-detail", "{s.detail}" }
                                                    }
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                            if !state.warnings.is_empty() {
                                div { class: "progress-warnings",
                                    strong { "Warnings:" }
                                    ul {
                                        for warning in state.warnings.iter() {
                                            li { "{warning}" }
                                        }
                                    }
                                }
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

            // ---- Result + feedback (done only) ----
            if done {
                if state.aem_uploaded && let Some(path) = state.aem_form_path.as_ref() {
                    div { class: "ag-aem",
                        span { class: "ag-aem-label", "Uploaded to AEM" }
                        span { class: "ag-aem-path", "{path}" }
                    }
                }

                // ---- Package actions: download CRX, upload to AEM ----
                if let Some(ref aem_data) = state.aem_package {
                    {
                        let st = upload_state.read().clone();
                        let uploading = st == UploadState::Uploading;
                        let no_connection = aem_connection.is_none();
                        let upload_title = match &st {
                            UploadState::Error(msg) => msg.clone(),
                            _ if no_connection => {
                                "Configure the AEM connection in Settings to enable this".to_string()
                            }
                            _ => "Upload and install the package on the configured AEM instance".to_string(),
                        };

                        rsx! {
                            div { class: "ag-result-actions",
                                button {
                                    class: "btn btn-secondary",
                                    title: "Download the AEM content package (CRX) as a ZIP",
                                    onclick: {
                                        let aem_data = aem_data.clone();
                                        let zip_filename = package_filename(&state.form_code);
                                        move |_| download_file(&aem_data, &zip_filename, "application/zip")
                                    },
                                    "⬇ Download CRX package"
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: uploading || no_connection,
                                    title: upload_title,
                                    onclick: {
                                        let aem_data = aem_data.clone();
                                        let connection = aem_connection.clone();
                                        let package_name = state
                                            .form_code
                                            .clone()
                                            .unwrap_or_else(|| "forms-package".to_string());
                                        move |_| {
                                            let Some(conn) = connection.clone() else {
                                                return;
                                            };
                                            let aem_data = aem_data.clone();
                                            let package_name = package_name.clone();
                                            upload_state.set(UploadState::Uploading);
                                            spawn(async move {
                                                match crate::aem_client::upload_and_install_package(
                                                    &conn,
                                                    aem_data,
                                                    &package_name,
                                                )
                                                .await
                                                {
                                                    Ok(()) => upload_state.set(UploadState::Success),
                                                    Err(e) => upload_state.set(UploadState::Error(e)),
                                                }
                                            });
                                        }
                                    },
                                    match st {
                                        UploadState::Uploading => rsx! {
                                            Spinner { size: "sm" }
                                            span { "Uploading…" }
                                        },
                                        UploadState::Success => rsx! { "✓ Uploaded to AEM" },
                                        UploadState::Error(_) => rsx! { "⚠ Upload failed — retry" },
                                        UploadState::Idle => rsx! { "⬆ Upload to AEM" },
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "ag-fb",
                    div { class: "ag-fb-label",
                        "Not quite right? Tell the agent what to change — it re-runs in the same session."
                    }
                    textarea {
                        class: "af-feedback-input",
                        rows: "3",
                        placeholder: "e.g. The phone number field should be optional.",
                        value: "{feedback}",
                        oninput: move |evt| feedback.set(evt.value()),
                    }
                    div { class: "ag-fb-row",
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
                }
            }
        }
    }
}

/// CSS modifier class for the collapsed-bar status dot.
fn tl_dot_class(status: &AgentStepStatus) -> &'static str {
    match status {
        AgentStepStatus::Done => "ok",
        AgentStepStatus::Error => "err",
        AgentStepStatus::Running => "",
    }
}

/// Inner glyph/spinner for the collapsed-bar status dot.
fn tl_dot_inner(status: &AgentStepStatus) -> Element {
    match status {
        AgentStepStatus::Running => rsx! {
            Spinner { size: "sm" }
        },
        AgentStepStatus::Done => rsx! {
            span { class: "af-ok", "✓" }
        },
        AgentStepStatus::Error => rsx! {
            span { class: "af-err", "✗" }
        },
    }
}
