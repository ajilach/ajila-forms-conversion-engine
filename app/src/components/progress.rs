use dioxus::prelude::*;

use super::spinner::Spinner;
use crate::models::{AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};

#[component]
pub fn ProgressDisplay(
    state: ProcessingState,
    on_image_click: EventHandler<(String, Vec<String>)>,
) -> Element {
    // AI processing shows the staged pipeline up to state rendering, then a
    // single "AI Generation" step takes over (same StepIndicator style, fewer
    // steps).
    // Agent Processing: a live activity log of the agent's thoughts and tool
    // calls (spinner while running, ✓/✗ when done) instead of staged steps.
    if state.ai_mode {
        let mut collapsed = use_signal(|| true);

        let tool_steps: Vec<_> = state
            .agent_steps
            .iter()
            .filter(|s| s.kind == AgentStepKind::Tool)
            .collect();
        let error_count = tool_steps
            .iter()
            .filter(|s| s.status == AgentStepStatus::Error)
            .count();
        let is_running = tool_steps
            .iter()
            .any(|s| s.status == AgentStepStatus::Running);
        let total = tool_steps.len();

        let summary = if state.agent_steps.is_empty() {
            "Starting agent…".to_string()
        } else if is_running {
            let last = tool_steps.last().map(|s| s.label.as_str()).unwrap_or("…");
            format!("{total} steps — running {last}…")
        } else if error_count > 0 {
            format!("{total} steps, {error_count} error(s)")
        } else {
            format!("{total} steps completed")
        };

        return rsx! {
            div { class: "progress-container",
                div { class: "agent-header",
                    h2 { "Agent Processing" }
                    button {
                        class: "agent-toggle",
                        onclick: move |_| {
                            let c = *collapsed.read();
                            collapsed.set(!c);
                        },
                        if *collapsed.read() { "▶ Show steps" } else { "▼ Hide steps" }
                    }
                }
                div { class: "agent-summary", "{summary}" }

                if !*collapsed.read() {
                    div { class: "agent-activity",
                        if state.agent_steps.is_empty() {
                            div { class: "agent-thought", "Starting agent…" }
                        }
                        for (i, s) in state.agent_steps.iter().enumerate() {
                            {match s.kind {
                                AgentStepKind::Thought => rsx! {
                                    div { key: "{i}", class: "agent-thought", "{s.label}" }
                                },
                                AgentStepKind::Tool => rsx! {
                                    div { key: "{i}", class: "agent-tool",
                                        span { class: "agent-tool-status",
                                            {match s.status {
                                                AgentStepStatus::Running => rsx! { Spinner { size: "sm" } },
                                                AgentStepStatus::Done => rsx! { span { class: "agent-ok", "✓" } },
                                                AgentStepStatus::Error => rsx! { span { class: "agent-err", "✗" } },
                                            }}
                                        }
                                        span { class: "agent-tool-name", "{s.label}" }
                                        if !s.detail.is_empty() {
                                            span { class: "agent-tool-detail", "{s.detail}" }
                                        }
                                    }
                                },
                            }}
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
            }
        };
    }

    rsx! {
        div { class: "progress-container",

            h2 { "Progress" }

            div { class: "progress-steps",

                StepIndicator {
                    name: "1. Parsing",
                    is_current: state.step == ProcessingStep::Parsing,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::ExhaustiveSearching
                        | ProcessingStep::Flattening
                        | ProcessingStep::Structuring
                        | ProcessingStep::Merging
                        | ProcessingStep::Complete
                    ),
                    progress: if state.step == ProcessingStep::Parsing { state.step_progress } else { None },
                }

                StepIndicator {
                    name: "2. Exhaustive Searching",
                    is_current: state.step == ProcessingStep::ExhaustiveSearching,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::Flattening
                        | ProcessingStep::Structuring
                        | ProcessingStep::Merging
                        | ProcessingStep::Complete
                    ),
                    progress: if state.step == ProcessingStep::ExhaustiveSearching { state.step_progress } else { None },
                }

                StepIndicator {
                    name: "3. Flattening",
                    is_current: state.step == ProcessingStep::Flattening,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::Structuring | ProcessingStep::Merging | ProcessingStep::Complete
                    ),
                    progress: if state.step == ProcessingStep::Flattening { state.step_progress } else { None },
                }

                // Show plain images after flattening
                if !state.plain_images.is_empty() {
                    super::image_grid::ImageGrid {
                        title: "Plain State Images",
                        images: state.plain_images.clone(),
                        on_image_click,
                    }
                }

                StepIndicator {
                    name: "4. Structuring",
                    is_current: state.step == ProcessingStep::Structuring,
                    is_complete: matches!(state.step, ProcessingStep::Merging | ProcessingStep::Complete),
                    progress: if state.step == ProcessingStep::Structuring { state.step_progress } else { None },
                }

                // Show labelled images after structuring
                if !state.labelled_images.is_empty() {
                    super::image_grid::ImageGrid {
                        title: "Labelled State Images",
                        images: state.labelled_images.clone(),
                        on_image_click,
                    }
                }

                StepIndicator {
                    name: "5. Merging",
                    is_current: state.step == ProcessingStep::Merging,
                    is_complete: state.step == ProcessingStep::Complete,
                    progress: if state.step == ProcessingStep::Merging { state.step_progress } else { None },
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
        }
    }
}

#[component]
pub fn StepIndicator(
    name: String,
    is_current: bool,
    is_complete: bool,
    progress: Option<f32>,
) -> Element {
    let class = if is_complete {
        "step step-complete"
    } else if is_current {
        "step step-current"
    } else {
        "step step-pending"
    };

    rsx! {
        div { class: "{class}",
            "{name}"
            if is_complete {
                span { class: "step-icon", "✓" }
            }
            if is_current {
                span { class: "step-icon",
                    Spinner { size: "sm" }
                }
            }
        }
    }
}
