mod components;
mod models;
mod pipeline;
mod platform;
mod processing;
#[cfg(feature = "fullstack")]
mod server;

use dioxus::prelude::*;

use components::{FileUploadSection, ImageModal, ProgressDisplay, ResultsSection};
use models::{ProcessingState, ProcessingStep};
use processing::run_and_track;

fn main() {
    #[cfg(feature = "desktop")]
    {
        dioxus::LaunchBuilder::desktop()
            .with_cfg(dioxus::desktop::Config::new().with_window(
                dioxus::desktop::WindowBuilder::new().with_title("Ajila Forms Conversion Engine"),
            ))
            .launch(App);
    }

    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(App);
    }
}

#[component]
fn App() -> Element {
    let mut processing_state = use_signal(ProcessingState::new);
    let mut is_processing = use_signal(|| false);
    let mut enlarged_image = use_signal(|| None::<(String, String)>);
    let selected_profile = use_signal(|| None::<String>);

    // Fetch available profiles on mount
    let profiles_resource = use_resource(|| async {
        #[cfg(feature = "fullstack")]
        {
            return crate::server::get_profiles().await.unwrap_or_default();
        }
        #[cfg(not(feature = "fullstack"))]
        {
            return blueprint::list_profiles();
        }
        #[allow(unreachable_code)]
        Vec::<String>::new()
    });
    let profiles: Vec<String> = profiles_resource
        .read()
        .as_ref()
        .cloned()
        .unwrap_or_default();

    let mut on_process = move |file_data: Vec<(String, Vec<u8>)>| {
        is_processing.set(true);
        processing_state.set(ProcessingState {
            step: ProcessingStep::Parsing,
            ..ProcessingState::new()
        });

        let profile = selected_profile.read().clone();
        spawn(async move {
            run_and_track(file_data, profile, processing_state).await;
            is_processing.set(false);
        });
    };

    rsx! {
        document::Stylesheet { href: asset!("./assets/styles.css") }

        // App Header
        header { class: "app-header",
            img {
                class: "app-header-logo",
                src: asset!("./assets/company-logo.webp"),
                alt: "Company Logo",
            }
            h1 { class: "app-header-title", "Forms Conversion Engine" }
            span { class: "app-header-version", "v{env!(\"CARGO_PKG_VERSION\")}" }
        }

        div { class: "app-container",

            // File Upload Section
            FileUploadSection {
                is_processing: *is_processing.read(),
                profiles: profiles.clone(),
                selected_profile,
                on_process: move |files: Vec<(String, Vec<u8>)>| {
                    on_process(files);
                },
            }

            // Progress Display
            if *is_processing.read() || processing_state.read().step != ProcessingStep::Idle {
                ProgressDisplay {
                    state: processing_state.read().clone(),
                    on_image_click: move |(name, data)| enlarged_image.set(Some((name, data))),
                }
            }

            // Results Section
            if processing_state.read().step == ProcessingStep::Complete {
                ResultsSection { state: processing_state.read().clone() }
            }

            // Image Modal Overlay
            if let Some((name, data)) = enlarged_image.read().as_ref() {
                ImageModal {
                    name: name.clone(),
                    data: data.clone(),
                    on_close: move |_| enlarged_image.set(None),
                }
            }
        }
    }
}
