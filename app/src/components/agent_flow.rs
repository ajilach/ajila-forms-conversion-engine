//! The app's only conversion UI: one status box that morphs through the whole
//! run — upload → live activity → done — without swapping screens. The activity
//! timeline is collapsed to its latest step by default and expands in place to
//! the full, scrollable history. The finished box carries the run's outputs and
//! the feedback field that re-runs the agent in the same session.
//!
//! [`AgentFlow`] owns the flow state and picks a [`Screen`]; everything below it
//! is a leaf that renders one band of the box.

use dioxus::html::HasFileData;
use dioxus::prelude::*;

use super::spinner::{Spinner, SpinnerSize};
use crate::files::download_file;
use crate::models::{
    AbortFlag, AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep,
    RetryAction,
};
use crate::upload::read_upload_files;

/// What the box shows: either the upload form, or a run in one of its states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Screen {
    Upload,
    Run(RunStatus),
}

/// How a run is doing. `Paused` is a live run waiting on the user's answer to a
/// failed request — the header, the phase rail and the badge all switch on it,
/// so it is one value rather than a phase plus a flag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunStatus {
    Running,
    Paused,
    Done,
    /// The run ended on an error (including the user giving up on a paused,
    /// retryable request) — the box reports it and offers a fresh start.
    Failed,
}

impl RunStatus {
    /// Modifier class and glyph for the status badge. `None` glyph means the
    /// badge shows a spinner instead.
    fn badge(self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::Running => ("run", None),
            Self::Paused => ("warn", Some("⏸")),
            Self::Done => ("ok", Some("✓")),
            Self::Failed => ("err", Some("✗")),
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Running => "Agent is working",
            Self::Paused => "Agent paused",
            Self::Done => "Finished",
            Self::Failed => "Agent stopped",
        }
    }

    /// Whether the run reached the end successfully.
    fn is_done(self) -> bool {
        self == Self::Done
    }
}

/// Derive what to show from the run state. The `Complete` step wins over
/// everything; a stopped run that recorded an error, or that the user aborted,
/// has ended; anything else with work in flight is a run in progress.
fn screen_for(state: &ProcessingState, processing: bool) -> Screen {
    if state.step == ProcessingStep::Complete {
        Screen::Run(RunStatus::Done)
    } else if !processing && (state.error.is_some() || state.aborted) {
        Screen::Run(RunStatus::Failed)
    } else if processing || state.step != ProcessingStep::Idle {
        Screen::Run(if state.retry_pending {
            RunStatus::Paused
        } else {
            RunStatus::Running
        })
    } else {
        Screen::Upload
    }
}

/// Lifecycle of the on-demand "Upload to AEM" action, surfaced inside the button.
#[derive(Clone, Debug, Eq, PartialEq)]
enum UploadState {
    Idle,
    Uploading,
    Success,
    Error(String),
}

/// Build a download filename like `forms-package-<code>.zip`, falling back to
/// `forms-package.zip` when the form code is unknown.
fn filename(prefix: &str, form_code: Option<&str>, ext: &str) -> String {
    match form_code {
        Some(code) => format!("{prefix}-{code}.{ext}"),
        None => format!("{prefix}.{ext}"),
    }
}

/// Render the activity timeline as a Markdown transcript of the run.
fn agent_log_markdown(steps: &[AgentStep]) -> String {
    let mut out = String::from("# Agent Conversion Log\n\n");
    for step in steps {
        match step.kind {
            AgentStepKind::Thought => {
                out.push_str(&format!("> {}\n\n", step.label.replace('\n', "\n> ")));
            }
            AgentStepKind::Tool => {
                let icon = step.status.glyph();
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
        ("file", "FILE")
    }
}

#[component]
pub fn AgentFlow(
    processing_state: Signal<ProcessingState>,
    is_processing: ReadSignal<bool>,
    profiles: Vec<String>,
    selected_profile: Signal<Option<String>>,
    selected_target: Signal<blueprint::OutputTarget>,
    /// Stops the running conversion when the Abort button is pressed.
    abort: AbortFlag,
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
    let mut feedback = use_signal(String::new);
    // Whether the activity timeline is expanded to its full history.
    let mut timeline_open = use_signal(|| false);

    let screen = screen_for(&processing_state.read(), is_processing());

    rsx! {
        div { class: "agent-flow",
            div { class: "agent-single",
                div { class: "agent-page",
                    match screen {
                        Screen::Upload => rsx! {
                            UploadBox {
                                profiles,
                                selected_profile,
                                selected_target,
                                ai_available,
                                uploaded_files,
                                on_start: move |files: Vec<(String, Vec<u8>)>| on_ai_process.call(files),
                            }
                        },
                        Screen::Run(status) => rsx! {
                            RunBox {
                                status,
                                state: processing_state,
                                files: uploaded_files,
                                profile: selected_profile.read().clone(),
                                aem_connection,
                                abort,
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
    on_start: EventHandler<Vec<(String, Vec<u8>)>>,
) -> Element {
    // A drop target fires enter/leave for every child element it crosses, so the
    // highlight follows a depth counter rather than the last event seen.
    let mut drag_depth = use_signal(|| 0usize);
    let is_dragging = use_memo(move || drag_depth() > 0);

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
                super::OutputTargetSelector {
                    profile: selected_profile.read().clone(),
                    selected_target,
                    disabled: false,
                }
            }

            div {
                class: if is_dragging() { "upload-dropzone upload-dropzone-dragging agent-dropzone" } else { "upload-dropzone agent-dropzone" },
                ondragenter: move |evt: Event<DragData>| {
                    evt.prevent_default();
                    drag_depth += 1;
                },
                ondragover: move |evt: Event<DragData>| evt.prevent_default(),
                ondragleave: move |evt: Event<DragData>| {
                    evt.prevent_default();
                    let next = drag_depth().saturating_sub(1);
                    drag_depth.set(next);
                },
                ondrop: move |evt: Event<DragData>| {
                    evt.prevent_default();
                    drag_depth.set(0);
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
                        onclick: move |_| on_start.call(files.clone()),
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
    status: RunStatus,
    state: ReadSignal<ProcessingState>,
    files: ReadSignal<Vec<(String, Vec<u8>)>>,
    profile: Option<String>,
    aem_connection: Option<blueprint::AemConnection>,
    abort: AbortFlag,
    timeline_open: Signal<bool>,
    feedback: Signal<String>,
    on_feedback: EventHandler<String>,
    /// Resume a paused run by re-sending the request that failed.
    on_retry: EventHandler<()>,
    /// Abandon a paused run instead of retrying it.
    on_give_up: EventHandler<()>,
    on_new: EventHandler<()>,
) -> Element {
    let done = status.is_done();
    let box_class = match status {
        RunStatus::Done => "ag-box done",
        RunStatus::Failed => "ag-box failed",
        _ => "ag-box",
    };

    rsx! {
        section { class: box_class,
            RunHeader {
                status,
                profile,
                elapsed_secs: state.read().elapsed_secs,
                abort,
                on_new,
            }
            PhaseRail { status }
            SourceFiles { files }
            ActivityTimeline { state, timeline_open }

            // ---- Failed request: retry (or give up) without losing the run ----
            if status == RunStatus::Paused {
                RetryPrompt { error: state.read().error.clone(), on_retry, on_give_up }
            } else if let Some(error) = state.read().error.as_ref() {
                div { class: "progress-error",
                    strong { "Error: " }
                    "{error}"
                }
            } else if state.read().aborted {
                div { class: "progress-note", "Stopped at your request." }
            }

            // Non-fatal problems the run reported — a Redacto dump that could not
            // be built, a cross-language merge that failed. Without this the run
            // looks clean while an output is silently missing.
            if !state.read().warnings.is_empty() {
                div { class: "progress-warnings",
                    strong { "Warnings:" }
                    ul {
                        for warning in state.read().warnings.iter() {
                            li { "{warning}" }
                        }
                    }
                }
            }

            // ---- Result + feedback (done only) ----
            if done {
                if state.read().aem_uploaded && let Some(path) = state.read().aem_form_path.as_ref() {
                    div { class: "ag-aem",
                        span { class: "ag-aem-label", "Uploaded to AEM" }
                        span { class: "ag-aem-path", "{path}" }
                    }
                }
                ResultActions { state, aem_connection }
                DownloadRow { state }
                FeedbackBox { feedback, on_feedback }
            }
        }
    }
}

/// Status badge, title, profile/duration meta, and the "New form" escape hatch.
#[component]
fn RunHeader(
    status: RunStatus,
    profile: Option<String>,
    elapsed_secs: Option<u64>,
    abort: AbortFlag,
    on_new: EventHandler<()>,
) -> Element {
    let (badge_class, glyph) = status.badge();

    rsx! {
        div { class: "ag-top",
            div { class: "ag-badge {badge_class}",
                match glyph {
                    Some(g) => rsx! { "{g}" },
                    None => rsx! {
                        Spinner {}
                    },
                }
            }
            div { class: "ag-top-text",
                h2 { class: "ag-title", "{status.title()}" }
                div { class: "ag-meta",
                    if let Some(p) = profile.as_ref() {
                        span {
                            "Profile "
                            b { "{p}" }
                        }
                    }
                    if let Some(secs) = elapsed_secs.filter(|_| status.is_done()) {
                        span {
                            "in "
                            b { "{format_elapsed(secs)}" }
                        }
                    }
                }
            }
            div { class: "ag-actions",
                match status {
                    // A live run can be stopped. The flag is polled at the run's
                    // next checkpoint — including mid-response — so the button
                    // reports that it asked rather than that it finished.
                    RunStatus::Running | RunStatus::Paused => rsx! {
                        AbortButton { abort }
                    },
                    RunStatus::Done | RunStatus::Failed => rsx! {
                        button {
                            class: "btn btn-secondary btn-sm",
                            onclick: move |_| on_new.call(()),
                            "↻ New form"
                        }
                    },
                }
            }
        }
    }
}

/// Upload → Convert → Finish. Upload is always behind us by the time this
/// renders; only the Convert step reflects the run status.
#[component]
fn PhaseRail(status: RunStatus) -> Element {
    let (convert_class, convert_glyph) = match status {
        RunStatus::Running => ("ag-phase active", "●"),
        RunStatus::Paused => ("ag-phase paused", "⏸"),
        RunStatus::Done => ("ag-phase done", "✓"),
        RunStatus::Failed => ("ag-phase failed", "✗"),
    };
    let done = status.is_done();

    rsx! {
        div { class: "ag-phases",
            div { class: "ag-phase done",
                span { class: "pn", "✓" }
                span { class: "pl", "Upload" }
            }
            div { class: "ag-pbar done" }
            div { class: convert_class,
                span { class: "pn", "{convert_glyph}" }
                span { class: "pl", "Convert" }
            }
            div { class: if done { "ag-pbar done" } else { "ag-pbar" } }
            div { class: if done { "ag-phase done" } else { "ag-phase" },
                span { class: "pn", if done { "✓" } else { "3" } }
                span { class: "pl", "Finish" }
            }
        }
    }
}

/// The uploaded source files as extension-badged chips.
#[component]
fn SourceFiles(files: ReadSignal<Vec<(String, Vec<u8>)>>) -> Element {
    if files.read().is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "ag-files",
            for (name , _bytes) in files.read().iter() {
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
}

/// The run's activity: collapsed to the latest step, or expanded to the full
/// scrollable history with the context-window indicator.
#[component]
fn ActivityTimeline(
    state: ReadSignal<ProcessingState>,
    mut timeline_open: Signal<bool>,
) -> Element {
    // Keep the timeline pinned to the newest step as the agent works. The memo
    // makes the dependency explicit — the effect must re-run on a new step, not
    // on every unrelated change to the run state.
    let step_count = use_memo(move || state.read().agent_steps.len());
    use_effect(move || {
        let _ = step_count();
        if timeline_open() {
            document::eval(
                r#"setTimeout(() => {
                    const el = document.getElementById('agent-flow-end');
                    if (el) el.scrollIntoView({ block: 'end' });
                }, 0);"#,
            );
        }
    });

    let open = timeline_open();
    let state = state.read();
    let steps = &state.agent_steps;

    rsx! {
        div { class: "ag-tl",
            button {
                class: "ag-tl-bar",
                onclick: move |_| timeline_open.toggle(),
                if open {
                    span { class: "ag-tl-title", "Activity · {steps.len()} steps" }
                    ContextGauge {
                        used: state.context_used_tokens,
                        window: state.context_window,
                    }
                } else {
                    // Collapsed: show only the latest step.
                    match steps.last() {
                        Some(s) if s.kind == AgentStepKind::Tool => rsx! {
                            span { class: "ag-tl-dot {s.status.dot_class()}", {status_glyph(s.status)} }
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
                                Spinner { size: SpinnerSize::Sm }
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
                                            span { class: "af-node", {status_glyph(s.status)} }
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
    }
}

/// The glyph (or spinner) that stands for a step's status.
fn status_glyph(status: AgentStepStatus) -> Element {
    match status {
        AgentStepStatus::Running => rsx! {
            Spinner { size: SpinnerSize::Sm }
        },
        AgentStepStatus::Done => rsx! {
            span { class: "af-ok", "{status.glyph()}" }
        },
        AgentStepStatus::Error => rsx! {
            span { class: "af-err", "{status.glyph()}" }
        },
    }
}

/// How much of the model's context window the run has filled. Renders nothing
/// until the agent reports a window.
#[component]
fn ContextGauge(used: usize, window: usize) -> Element {
    if window == 0 {
        return rsx! {};
    }

    let used = used.min(window);
    let pct = (used as f32 / window as f32 * 100.0).round() as u32;
    let fill = if pct >= 90 {
        "var(--danger)"
    } else if pct >= 75 {
        "var(--warn)"
    } else {
        "var(--accent)"
    };
    let ring = format!(
        "background: conic-gradient({fill} {}deg, var(--border) 0);",
        pct * 36 / 10
    );

    rsx! {
        span {
            class: "ag-ctx",
            title: "Context window · {used} / {window} tokens ({pct}%)",
            span { class: "ag-ctx-ring", style: "{ring}" }
            span { class: "ag-ctx-pct", "{pct}%" }
        }
    }
}

/// A failed request paused the run: offer to re-send it, or to give up.
#[component]
fn RetryPrompt(
    error: Option<String>,
    on_retry: EventHandler<()>,
    on_give_up: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "ag-retry",
            div { class: "ag-retry-title", "The request to Claude failed — the run is paused." }
            if let Some(error) = error.as_ref() {
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
    }
}

/// Stop a live run.
///
/// Its own component so the "asked already" state lives exactly as long as the
/// run does: the button is mounted only while the run is live, so a finished run
/// drops it and the next run starts with a fresh button. Keeping the state in
/// the header instead would carry a stale "Stopping…" into a feedback re-run.
#[component]
fn AbortButton(abort: AbortFlag) -> Element {
    let mut asked = use_signal(|| false);

    rsx! {
        button {
            class: "btn btn-secondary btn-sm",
            disabled: asked(),
            title: "Stop this conversion. Whatever the agent has built so far is kept.",
            onclick: move |_| {
                abort.abort();
                asked.set(true);
            },
            if asked() { "Stopping…" } else { "✕ Abort" }
        }
    }
}

/// Act on the finished result: download the package, install it on AEM.
///
/// Both actions are the AEM target's, so a Redacto run renders nothing here
/// rather than an empty row taking up the space above the downloads.
#[component]
fn ResultActions(
    state: ReadSignal<ProcessingState>,
    aem_connection: Option<blueprint::AemConnection>,
) -> Element {
    let mut upload_state = use_signal(|| UploadState::Idle);
    let run = state.read();

    if !Artifact::Package.is_offered(&run) {
        return rsx! {};
    }

    rsx! {
        div { class: "ag-result-actions",
            DownloadButton {
                class: "btn btn-secondary",
                artifact: Artifact::Package,
                state,
            }
            if let Some(package) = run.aem_package.as_ref() {
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
                                let package = package.clone();
                                let connection = aem_connection;
                                let package_name = run
                                    .form_code
                                    .clone()
                                    .unwrap_or_else(|| "forms-package".to_string());
                                move |_| {
                                    let Some(conn) = connection.clone() else {
                                        return;
                                    };
                                    let package = package.clone();
                                    let package_name = package_name.clone();
                                    upload_state.set(UploadState::Uploading);
                                    spawn(async move {
                                        match crate::aem_client::upload_and_install_package(
                                            &conn,
                                            package,
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
                                    Spinner { size: SpinnerSize::Sm }
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
    }
}

/// One artefact a finished run offers for download.
///
/// A variant, not the payload: `ProcessingState` holds the multi-MB AEM package,
/// so a component that took the bytes as a prop would deep-copy them on every
/// render and then byte-compare them again for prop memoisation. The bytes are
/// materialised only when the button is actually pressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Artifact {
    Package,
    RedactoSql,
    Xsd,
    AgentLog,
}

impl Artifact {
    /// The by-products offered next to the primary result, in display order.
    const BYPRODUCTS: &'static [Self] = &[Self::RedactoSql, Self::Xsd, Self::AgentLog];

    fn label(self) -> &'static str {
        match self {
            Self::Package => "⬇ Download CRX package",
            Self::RedactoSql => "Redacto SQL",
            Self::Xsd => "XSD schema",
            Self::AgentLog => "Agent log",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Package => "Download the AEM content package (CRX) as a ZIP",
            Self::RedactoSql => {
                "The Redacto PostgreSQL dump (document, components and text assets)"
            }
            Self::Xsd => "The XML Schema Definition for the converted form",
            Self::AgentLog => "The agent's full activity timeline as a Markdown transcript",
        }
    }

    /// `(filename prefix, extension)`, paired here so a payload cannot end up
    /// under another artefact's name.
    fn naming(self) -> (&'static str, &'static str) {
        match self {
            Self::Package => ("forms-package", "zip"),
            Self::RedactoSql => ("redacto", "sql"),
            Self::Xsd => ("schema", "xsd"),
            Self::AgentLog => ("agent-log", "md"),
        }
    }

    /// Whether this artefact belongs to `target`'s result panel.
    ///
    /// Presence alone is not the rule: an artefact that only makes sense for the
    /// other target must stay hidden even if the run happens to have produced it.
    /// The log belongs to every run.
    fn belongs_to(self, target: blueprint::OutputTarget) -> bool {
        match self {
            Self::Package | Self::Xsd => target == blueprint::OutputTarget::Aem,
            Self::RedactoSql => target == blueprint::OutputTarget::Redacto,
            Self::AgentLog => true,
        }
    }

    /// This artefact's bytes, or `None` if the run did not produce it.
    fn bytes(self, state: &ProcessingState) -> Option<Vec<u8>> {
        match self {
            Self::Package => state.aem_package.clone(),
            Self::RedactoSql => state.redacto_sql.as_ref().map(|s| s.clone().into_bytes()),
            Self::Xsd => state.xsd_schema.as_ref().map(|s| s.clone().into_bytes()),
            Self::AgentLog => (!state.agent_steps.is_empty())
                .then(|| agent_log_markdown(&state.agent_steps).into_bytes()),
        }
    }

    /// Whether the run produced it *and* it belongs to that run's target.
    fn is_offered(self, state: &ProcessingState) -> bool {
        if !self.belongs_to(state.target) {
            return false;
        }
        match self {
            Self::Package => state.aem_package.is_some(),
            Self::RedactoSql => state.redacto_sql.is_some(),
            Self::Xsd => state.xsd_schema.is_some(),
            Self::AgentLog => !state.agent_steps.is_empty(),
        }
    }
}

/// Take a copy of the by-products the run's target offers. Outside the
/// AEM-package guard, since a Redacto run produces no package but still yields a
/// dump and the log.
#[component]
fn DownloadRow(state: ReadSignal<ProcessingState>) -> Element {
    rsx! {
        div { class: "ag-downloads",
            span { class: "ag-downloads-label", "Also download" }
            for artifact in Artifact::BYPRODUCTS.iter().copied() {
                if artifact.is_offered(&state.read()) {
                    DownloadButton {
                        key: "{artifact:?}",
                        class: "btn btn-secondary btn-sm",
                        artifact,
                        state,
                    }
                }
            }
        }
    }
}

/// A button that saves one artefact to the user's Downloads folder.
#[component]
fn DownloadButton(
    class: &'static str,
    artifact: Artifact,
    state: ReadSignal<ProcessingState>,
) -> Element {
    // A failed save has to be visible: the user pressed a button and would
    // otherwise be left looking for a file that was never written.
    let mut error = use_signal(|| None::<String>);

    rsx! {
        button {
            class,
            title: artifact.title(),
            onclick: move |_| {
                let state = state.read();
                let (prefix, ext) = artifact.naming();
                let name = filename(prefix, state.form_code.as_deref(), ext);
                error
                    .set(
                        match artifact.bytes(&state) {
                            Some(bytes) => download_file(&bytes, &name).err(),
                            None => Some("That output is no longer available.".to_string()),
                        },
                    );
            },
            "{artifact.label()}"
        }
        if let Some(error) = error.read().as_ref() {
            span { class: "ag-save-error", "{error}" }
        }
    }
}

/// Tell the agent what to change; it re-runs in the same session.
#[component]
fn FeedbackBox(mut feedback: Signal<String>, on_feedback: EventHandler<String>) -> Element {
    rsx! {
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
                    disabled: feedback.read().trim().is_empty(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
            filename("forms-package", Some("AAEV"), "zip"),
            "forms-package-AAEV.zip"
        );
        assert_eq!(filename("redacto", None, "sql"), "redacto.sql");
    }

    /// The four states the box can be in are derived from three separate fields,
    /// so pin the mapping down — a wrong screen strands the user.
    #[test]
    fn the_screen_follows_the_run_state() {
        let idle = ProcessingState::default();
        assert_eq!(screen_for(&idle, false), Screen::Upload);

        let running = ProcessingState {
            step: ProcessingStep::Running,
            ..Default::default()
        };
        assert_eq!(screen_for(&running, true), Screen::Run(RunStatus::Running));

        let paused = ProcessingState {
            retry_pending: true,
            error: Some("boom".into()),
            ..running.clone()
        };
        assert_eq!(screen_for(&paused, true), Screen::Run(RunStatus::Paused));

        // The run stopped and recorded an error: failed, not still running.
        let failed = ProcessingState {
            error: Some("boom".into()),
            ..running
        };
        assert_eq!(screen_for(&failed, false), Screen::Run(RunStatus::Failed));

        // A completed run reports Done even if it also collected an error.
        let complete = ProcessingState {
            step: ProcessingStep::Complete,
            error: Some("boom".into()),
            ..Default::default()
        };
        assert_eq!(screen_for(&complete, false), Screen::Run(RunStatus::Done));
    }

    /// An aborted run records no error, so without its own arm the box would sit
    /// on "Agent is working" forever after the run had already stopped.
    #[test]
    fn an_aborted_run_reaches_a_terminal_screen() {
        let aborting = ProcessingState {
            step: ProcessingStep::Running,
            aborted: true,
            ..Default::default()
        };

        // The flag is set the moment the button is pressed, but the run is still
        // unwinding — it must keep reporting as running until it has stopped.
        assert_eq!(
            screen_for(&aborting, true),
            Screen::Run(RunStatus::Running),
            "the box must not claim the run ended while it is still unwinding"
        );

        assert_eq!(
            screen_for(&aborting, false),
            Screen::Run(RunStatus::Failed),
            "once the run has stopped the box has to leave the running state"
        );
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

    /// A state holding every artefact, so the target rule is what decides which
    /// ones the panel offers.
    fn state_with_everything(target: blueprint::OutputTarget) -> ProcessingState {
        ProcessingState {
            step: ProcessingStep::Complete,
            target,
            aem_package: Some(vec![1, 2, 3]),
            xsd_schema: Some("<xsd/>".into()),
            redacto_sql: Some("INSERT ...".into()),
            agent_steps: vec![step(
                AgentStepKind::Tool,
                "build_aem_package",
                "",
                AgentStepStatus::Done,
            )],
            ..Default::default()
        }
    }

    /// Presence is not the rule. An AEM run must not offer the Redacto dump even
    /// when one exists, and vice versa — otherwise the panel advertises an
    /// artefact from the target the user did not pick.
    #[test]
    fn each_target_offers_only_its_own_artifacts() {
        let aem = state_with_everything(blueprint::OutputTarget::Aem);
        assert!(Artifact::Package.is_offered(&aem));
        assert!(Artifact::Xsd.is_offered(&aem));
        assert!(!Artifact::RedactoSql.is_offered(&aem));

        let redacto = state_with_everything(blueprint::OutputTarget::Redacto);
        assert!(Artifact::RedactoSql.is_offered(&redacto));
        assert!(!Artifact::Package.is_offered(&redacto));
        assert!(!Artifact::Xsd.is_offered(&redacto));
    }

    /// The log is the record of the run itself, so it survives either target.
    #[test]
    fn the_agent_log_is_offered_for_every_target() {
        for target in blueprint::OutputTarget::ALL {
            assert!(
                Artifact::AgentLog.is_offered(&state_with_everything(target)),
                "{target:?}"
            );
        }
    }

    /// A target that produced nothing offers nothing: the rule gates on top of
    /// presence, it does not replace it.
    #[test]
    fn an_artifact_the_run_never_produced_is_not_offered() {
        let empty = ProcessingState {
            step: ProcessingStep::Complete,
            target: blueprint::OutputTarget::Aem,
            ..Default::default()
        };

        assert!(!Artifact::Package.is_offered(&empty));
        assert!(!Artifact::Xsd.is_offered(&empty));
        assert!(!Artifact::AgentLog.is_offered(&empty));
    }
}
