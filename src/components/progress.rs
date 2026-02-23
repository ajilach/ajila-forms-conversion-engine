use dioxus::prelude::*;

use crate::models::{ProcessingState, ProcessingStep};

#[component]
pub fn ProgressDisplay(
    state: ProcessingState,
    on_image_click: EventHandler<(String, String)>,
) -> Element {
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
                }

                StepIndicator {
                    name: "3. Flattening",
                    is_current: state.step == ProcessingStep::Flattening,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::Structuring | ProcessingStep::Merging | ProcessingStep::Complete
                    ),
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
pub fn StepIndicator(name: String, is_current: bool, is_complete: bool) -> Element {
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
                span { class: "step-icon", "●" }
            }
        }
    }
}
