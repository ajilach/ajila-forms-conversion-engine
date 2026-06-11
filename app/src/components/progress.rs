use dioxus::prelude::*;

use super::spinner::Spinner;
use crate::models::{ProcessingState, ProcessingStep};

#[component]
pub fn ProgressDisplay(
    state: ProcessingState,
    on_image_click: EventHandler<(String, String)>,
) -> Element {
    // AI processing shows the staged pipeline up to state rendering, then a
    // single "AI Generation" step takes over (same StepIndicator style, fewer
    // steps).
    if state.ai_mode {
        let step = &state.step;
        return rsx! {
            div { class: "progress-container",

                h2 { "Progress" }

                div { class: "progress-steps",

                    StepIndicator {
                        name: "1. Parsing",
                        is_current: *step == ProcessingStep::Parsing,
                        is_complete: matches!(
                            step,
                            ProcessingStep::ExhaustiveSearching
                            | ProcessingStep::Flattening
                            | ProcessingStep::AiGenerating
                            | ProcessingStep::Complete
                        ),
                        progress: if *step == ProcessingStep::Parsing { state.step_progress } else { None },
                    }

                    StepIndicator {
                        name: "2. Exhaustive Searching",
                        is_current: *step == ProcessingStep::ExhaustiveSearching,
                        is_complete: matches!(
                            step,
                            ProcessingStep::Flattening
                            | ProcessingStep::AiGenerating
                            | ProcessingStep::Complete
                        ),
                        progress: if *step == ProcessingStep::ExhaustiveSearching { state.step_progress } else { None },
                    }

                    StepIndicator {
                        name: "3. State Rendering",
                        is_current: *step == ProcessingStep::Flattening,
                        is_complete: matches!(
                            step,
                            ProcessingStep::AiGenerating | ProcessingStep::Complete
                        ),
                        progress: if *step == ProcessingStep::Flattening { state.step_progress } else { None },
                    }

                    // Show the rendered page images once available (same grid
                    // as normal processing).
                    if !state.plain_images.is_empty() {
                        super::image_grid::ImageGrid {
                            title: "Plain State Images",
                            images: state.plain_images.clone(),
                            on_image_click,
                        }
                    }

                    StepIndicator {
                        name: "4. AI Generation",
                        is_current: *step == ProcessingStep::AiGenerating,
                        is_complete: *step == ProcessingStep::Complete,
                        progress: None,
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
