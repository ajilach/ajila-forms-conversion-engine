//! The app's only conversion UI: one status box that morphs through the whole
//! run — upload → live activity → done — without swapping screens. The activity
//! timeline is collapsed to its latest step by default and expands in place to
//! the full, scrollable history. The finished box carries the run's outputs and
//! the feedback field that re-runs the agent in the same session.

use dioxus::html::HasFileData;
use dioxus::prelude::*;

use super::spinner::Spinner;
use crate::models::{AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep, RetryAction};
use crate::platform::{download_file, show_html_preview};
use crate::upload::read_upload_files;

/// Which phase of the agent flow is currently shown.
#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Upload,
    Running,
    Done,
    /// The run ended on an error (including the user giving up on a paused,
    /// retryable request) — the box reports it and offers a fresh start.
    Failed,
}

/// Lifecycle of the on-demand "Upload to AEM" action, surfaced inside the button.
#[derive(Clone, PartialEq)]
enum UploadState {
    Idle,
    Uploading,
    Success,
    Error(String),
}

/// Build a download filename like `forms-package-<code>.zip`, falling back to
/// `forms-package.zip` when the form code is unknown.
fn filename(prefix: &str, form_code: &Option<String>, ext: &str) -> String {
    match form_code {
        Some(code) => format!("{prefix}-{code}.{ext}"),
        None => format!("{prefix}.{ext}"),
    }
}

/// Render the activity timeline as a Markdown transcript of the run.
fn agent_log_markdown(steps: &[crate::models::AgentStep]) -> String {
    let mut out = String::from("# Agent Conversion Log\n\n");
    for step in steps {
        match step.kind {
            AgentStepKind::Thought => {
                out.push_str(&format!("> {}\n\n", step.label.replace('\n', "\n> ")));
            }
            AgentStepKind::Tool => {
                let icon = match step.status {
                    AgentStepStatus::Done => "✓",
                    AgentStepStatus::Error => "✗",
                    AgentStepStatus::Running => "…",
                };
                if step.detail.is_empty() {
                    out.push_str(&format!("- {icon} `{}`\n", step.label));
                } else {
                    out.push_str(&format!("- {icon} `{}` — {}\n", step.label, step.detail));
                }
            }
        }
    }
    out
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
    selected_target: Signal<blueprint::OutputTarget>,
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
    } else if !processing && state.error.is_some() {
        Phase::Failed
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
            selected_target,
                                ai_available,
                                uploaded_files,
                                is_dragging,
                                drag_depth,
                                on_start: move |files: Vec<(String, Vec<u8>)>| on_ai_process.call(files),
                            }
                        },
                        Phase::Running | Phase::Done | Phase::Failed => rsx! {
                            RunBox {
                                phase,
                                state: state.clone(),
                                files: uploaded_files.read().clone(),
                                profile: selected_profile.read().clone(),
                                aem_connection: aem_connection.clone(),
                                timeline_open,
                                feedback,
                                on_feedback: move |text: String| on_feedback.call(text),
                                // Answer a paused run's retry prompt; the agent loop
                                // polls these on the shared processing state.
                                on_retry: move |_| {
                                    processing_state.write().retry_action = Some(RetryAction::Retry);
                                },
                                on_give_up: move |_| {
                                    processing_state.write().retry_action = Some(RetryAction::Cancel);
                                },
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
    selected_target: Signal<blueprint::OutputTarget>,
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
    // An AEM content-package ZIP can be attached as an editable template; a run
    // needs at least a PDF or a template.
    let has_template = files
        .iter()
        .any(|(_, bytes)| blueprint::detect_aem_zip(bytes));
    let start_disabled = files.is_empty() || (!has_pdf && !has_template) || !ai_available;
    let start_title = if !ai_available {
        "Configure an API key in Settings to enable agent processing."
    } else if files.is_empty() {
        "Drop or choose a file to begin."
    } else if !has_pdf && !has_template {
        "Upload at least one PDF, or an AEM content-package ZIP template, to start the agent."
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
                crate::components::OutputTargetSelector {
                    profile: selected_profile.read().clone(),
                    selected_target,
                    disabled: false,
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
                p { class: "upload-hint",
                    "Upload the PDF forms here — optionally add an AEM content-package ZIP as an editable template."
                }
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
                    accept: ".pdf,.zip",
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
/// A failed request pauses the run instead of ending it, and is surfaced here as
/// a Retry / Give up prompt.
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
    /// Resume a paused run by re-sending the request that failed.
    on_retry: EventHandler<()>,
    /// Abandon a paused run instead of retrying it.
    on_give_up: EventHandler<()>,
    on_new: EventHandler<()>,
) -> Element {
    let done = phase == Phase::Done;
    let failed = phase == Phase::Failed;
    // The run is alive but waiting on the user's answer to a failed request.
    let paused = state.retry_pending;
    let open = *timeline_open.read();
    let steps = &state.agent_steps;
    let latest = steps.last();

    // Context-window fill indicator (shown next to the step count when expanded).
    let ctx_window = state.context_window;
    let ctx_used = state.context_used_tokens.min(ctx_window);
    let ctx_pct = if ctx_window > 0 {
        (ctx_used as f32 / ctx_window as f32 * 100.0).round() as u32
    } else {
        0
    };
    let ctx_fill = if ctx_pct >= 90 {
        "var(--danger, #c2185b)"
    } else if ctx_pct >= 75 {
        "var(--warn, #dc9e26)"
    } else {
        "var(--accent)"
    };
    let ctx_ring_style = format!(
        "background: conic-gradient({ctx_fill} {}deg, var(--border) 0);",
        ctx_pct * 36 / 10
    );
    let ctx_title = format!("Context window · {ctx_used} / {ctx_window} tokens ({ctx_pct}%)");
    let feedback_empty = feedback.read().trim().is_empty();
    // Lifecycle of the on-demand AEM upload, reported inside the button.
    let mut upload_state = use_signal(|| UploadState::Idle);

    rsx! {
        section {
            class: if done { "ag-box done" } else if failed { "ag-box failed" } else { "ag-box" },
            // ---- Header ----
            div { class: "ag-top",
                if done {
                    div { class: "ag-badge ok", "✓" }
                } else if failed {
                    div { class: "ag-badge err", "✗" }
                } else if paused {
                    div { class: "ag-badge warn", "⏸" }
                } else {
                    div { class: "ag-badge run",
                        Spinner { size: "md" }
                    }
                }
                div { class: "ag-top-text",
                    h2 { class: "ag-title",
                        if done {
                            "Finished"
                        } else if failed {
                            "Agent stopped"
                        } else if paused {
                            "Agent paused"
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
                if done || failed {
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
                } else if failed {
                    div { class: "ag-phase failed",
                        span { class: "pn", "✗" }
                        span { class: "pl", "Convert" }
                    }
                    div { class: "ag-pbar" }
                    div { class: "ag-phase",
                        span { class: "pn", "3" }
                        span { class: "pl", "Finish" }
                    }
                } else {
                    div { class: if paused { "ag-phase paused" } else { "ag-phase active" },
                        span { class: "pn", if paused { "⏸" } else { "●" } }
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
                        if ctx_window > 0 {
                            span { class: "ag-ctx", title: "{ctx_title}",
                                span { class: "ag-ctx-ring", style: "{ctx_ring_style}" }
                                span { class: "ag-ctx-pct", "{ctx_pct}%" }
                            }
                        }
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
                            div { id: "agent-flow-end" }
                        }
                    }
                }
            }

            // ---- Failed request: retry (or give up) without losing the run ----
            if paused {
                div { class: "ag-retry",
                    div { class: "ag-retry-title", "The request to Claude failed — the run is paused." }
                    if let Some(error) = &state.error {
                        div { class: "ag-retry-msg", "{error}" }
                    }
                    div { class: "ag-retry-hint",
                        "Everything the agent has built so far is still in memory. Retry re-sends only \
                         the step that failed."
                    }
                    div { class: "ag-retry-actions",
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| on_retry.call(()),
                            "↻ Retry"
                        }
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| on_give_up.call(()),
                            "Give up"
                        }
                    }
                }
            } else if let Some(error) = &state.error {
                div { class: "progress-error",
                    strong { "Error: " }
                    "{error}"
                }
            }

            // Non-fatal problems the run reported — a Redacto dump that could not
            // be built, a cross-language merge that failed. Without this the run
            // looks clean while an output is silently missing.
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

            // ---- Result + feedback (done only) ----
            if done {
                if state.aem_uploaded && let Some(path) = state.aem_form_path.as_ref() {
                    div { class: "ag-aem",
                        span { class: "ag-aem-label", "Uploaded to AEM" }
                        span { class: "ag-aem-path", "{path}" }
                    }
                }

                // ---- Row A: act on the result ----
                div { class: "ag-result-actions",
                    if let Some(ref aem_data) = state.aem_package {
                        button {
                            class: "btn btn-secondary",
                            title: "Download the AEM content package (CRX) as a ZIP",
                            onclick: {
                                let aem_data = aem_data.clone();
                                let zip_filename = filename("forms-package", &state.form_code, "zip");
                                move |_| download_file(&aem_data, &zip_filename)
                            },
                            "⬇ Download CRX package"
                        }
                    }
                    if let Some(ref html) = state.html_preview {
                        button {
                            class: "btn btn-secondary",
                            title: "Render the converted document as a standalone HTML page and open it in the browser",
                            onclick: {
                                let html = html.clone();
                                let preview_filename = filename("preview", &state.form_code, "html");
                                move |_| show_html_preview(html.clone(), &preview_filename)
                            },
                            "◹ HTML preview"
                        }
                    }
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

                // ---- Row B: take a copy. Outside the AEM-package guard, since a
                // Redacto run produces no package but still yields a dump, the
                // structure and the log ----
                div { class: "ag-downloads",
                    span { class: "ag-downloads-label", "Also download" }
                    if let Some(ref sql_data) = state.redacto_sql {
                        button {
                            class: "btn btn-secondary btn-sm",
                            title: "The Redacto PostgreSQL dump (document, components and text assets)",
                            onclick: {
                                let sql_data = sql_data.clone();
                                let sql_filename = filename("redacto", &state.form_code, "sql");
                                move |_| download_file(sql_data.as_bytes(), &sql_filename)
                            },
                            "Redacto SQL"
                        }
                    }
                    if let Some(ref json_data) = state.merged_json {
                        button {
                            class: "btn btn-secondary btn-sm",
                            title: "The structured document the outputs were generated from",
                            onclick: {
                                let json_data = json_data.clone();
                                let json_filename = filename("structure", &state.form_code, "json");
                                move |_| download_file(json_data.as_bytes(), &json_filename)
                            },
                            "Structure JSON"
                        }
                    }
                    if let Some(ref xsd_data) = state.xsd_schema {
                        button {
                            class: "btn btn-secondary btn-sm",
                            title: "The XML Schema Definition for the converted form",
                            onclick: {
                                let xsd_data = xsd_data.clone();
                                let xsd_filename = filename("schema", &state.form_code, "xsd");
                                move |_| download_file(xsd_data.as_bytes(), &xsd_filename)
                            },
                            "XSD schema"
                        }
                    }
                    if !state.agent_steps.is_empty() {
                        button {
                            class: "btn btn-secondary btn-sm",
                            title: "The agent's full activity timeline as a Markdown transcript",
                            onclick: {
                                let steps = state.agent_steps.clone();
                                let log_filename = filename("agent-log", &state.form_code, "md");
                                move |_| {
                                    let md = agent_log_markdown(&steps);
                                    download_file(md.as_bytes(), &log_filename);
                                }
                            },
                            "Agent log"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentStep;

    fn step(kind: AgentStepKind, label: &str, detail: &str, status: AgentStepStatus) -> AgentStep {
        AgentStep {
            id: String::new(),
            kind,
            label: label.to_string(),
            detail: detail.to_string(),
            status,
        }
    }

    #[test]
    fn filename_falls_back_when_the_form_code_is_unknown() {
        assert_eq!(
            filename("forms-package", &Some("AAEV".into()), "zip"),
            "forms-package-AAEV.zip"
        );
        assert_eq!(filename("redacto", &None, "sql"), "redacto.sql");
    }

    /// The log is the only durable record of a run once the window is closed, so
    /// every step kind has to survive the transcript.
    #[test]
    fn agent_log_renders_thoughts_and_tool_calls() {
        let md = agent_log_markdown(&[
            step(
                AgentStepKind::Thought,
                "Analysing",
                "",
                AgentStepStatus::Done,
            ),
            step(
                AgentStepKind::Tool,
                "build_aem_package",
                "12 components",
                AgentStepStatus::Done,
            ),
            step(
                AgentStepKind::Tool,
                "upload_to_aem",
                "",
                AgentStepStatus::Error,
            ),
        ]);

        assert!(md.starts_with("# Agent Conversion Log\n\n"), "{md}");
        assert!(md.contains("> Analysing\n"), "{md}");
        assert!(
            md.contains("- ✓ `build_aem_package` — 12 components\n"),
            "{md}"
        );
        // A detail-less tool call must not leave a dangling em dash.
        assert!(md.contains("- ✗ `upload_to_aem`\n"), "{md}");
    }

    /// A multi-line thought has to stay inside the blockquote, otherwise the
    /// continuation lines render as body text.
    #[test]
    fn agent_log_keeps_multiline_thoughts_quoted() {
        let md = agent_log_markdown(&[step(
            AgentStepKind::Thought,
            "First line\nSecond line",
            "",
            AgentStepStatus::Done,
        )]);

        assert!(md.contains("> First line\n> Second line\n"), "{md}");
    }
}
