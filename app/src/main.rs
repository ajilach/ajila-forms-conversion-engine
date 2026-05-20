mod components;
mod markdown;
mod models;
mod pipeline;
mod platform;
mod processing;

use dioxus::prelude::*;

use components::{
    AemPreview, AemPreviewEnvelope, EnvelopeWrapper, FileUploadSection, ImageModal,
    ProgressDisplay, ResultsSection, SettingsPanel, StructuredEditor,
};
use models::{DocumentEnvelope, ProcessingState, ProcessingStep};
use processing::run_and_track;

fn main() {
    #[cfg(feature = "desktop")]
    {
        // Start with always-on-top enabled (previous behaviour preserved by default).
        let config = dioxus::desktop::Config::new().with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_always_on_top(false)
                .with_title("Ajila Forms Conversion Engine"),
        );
        dioxus::LaunchBuilder::new().with_cfg(config).launch(App);
    }

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut processing_state = use_signal(ProcessingState::new);
    let mut is_processing = use_signal(|| false);
    let mut enlarged_image = use_signal(|| None::<(String, String)>);
    let selected_profile = use_signal(|| None::<String>);
    let mut editor_envelope = use_signal(|| None::<DocumentEnvelope>);
    let mut settings_open = use_signal(|| false);
    let mut always_on_top = use_signal(|| false);
    let mut aem_preview_envelope = use_signal(|| None::<DocumentEnvelope>);

    let profiles = blueprint::list_profiles();

    // ── Pipeline ──────────────────────────────────────────────────────────────
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

    // Handle applying changes from the editor
    let handle_editor_apply = move |envelope: DocumentEnvelope| {
        // Update the processing state with the edited envelope
        let mut state = processing_state.write();
        state.envelope = Some(envelope.clone());

        // Regenerate JSON
        if let Ok(json) = serde_json::to_string_pretty(&envelope) {
            state.merged_json = Some(json);
        }

        // Regenerate HTML preview if profile supports it
        let profile = selected_profile.read().clone();
        if let Some(ref profile_name) = profile
            && blueprint::has_html_config(profile_name)
            && let Ok(styles) = blueprint::load_html_custom_styles(profile_name)
        {
            let html_config = blueprint::HtmlConfig {
                custom_styles: Some(styles),
                ..blueprint::HtmlConfig::default()
            };
            state.html_preview = Some(blueprint::to_html(&envelope.content, &html_config));
        }

        // Regenerate AEM package if profile supports it
        if let Some(ref profile_name) = profile
            && blueprint::has_aem_config(profile_name)
            && let Ok(aem_config) = blueprint::load_aem_config(profile_name, &envelope.context)
        {
            let aem_zip = blueprint::to_aem_package(&envelope.content, &aem_config);
            state.form_code = Some(aem_config.form_code.clone());
            state.aem_package = Some(aem_zip);
        }

        // Regenerate XSD if profile supports it
        if let Some(ref profile_name) = profile
            && blueprint::has_xsd_config(profile_name)
            && let Ok(mut xsd_config) = blueprint::load_xsd_config(profile_name)
        {
            if let Some(ref fc) = state.form_code {
                xsd_config.form_code = Some(fc.clone());
            }
            state.xsd_schema = Some(blueprint::to_xsd(&envelope.content, &xsd_config));
        }

        drop(state);

        // Close the editor
        editor_envelope.set(None);
    };

    // ── Render ────────────────────────────────────────────────────────────────
    rsx! {
        document::Stylesheet { href: asset!("/assets/styles.css") }

        // App Header
        header { class: "app-header",
            img {
                class: "app-header-logo",
                src: asset!("/assets/company-logo.webp"),
                alt: "Company Logo",
            }
            div { class: "app-header-right",
                h1 { class: "app-header-title", "Forms Conversion Engine" }
                span { class: "app-header-version", "v{env!(\"CARGO_PKG_VERSION\")}" }
            }
            button {
                class: "settings-btn",
                title: "Settings",
                onclick: move |_| settings_open.set(true),
                "⚙"
            }
        }

        // Settings modal
        SettingsPanel {
            open: *settings_open.read(),
            on_close: move |_| settings_open.set(false),
            always_on_top: *always_on_top.read(),
            on_toggle_always_on_top: move |value: bool| {
                always_on_top.set(value);
                // Window API only exists on native desktop targets.
                #[cfg(feature = "desktop")]
                dioxus::desktop::use_window().set_always_on_top(value);
            },
        }

        // Show either the editor, AEM preview, or the main app content
        if let Some(envelope) = editor_envelope.read().clone() {
            // Structured Editor (full page view)
            div { class: "editor-page",
                StructuredEditor {
                    envelope: EnvelopeWrapper(envelope),
                    plain_images: processing_state.read().plain_images.clone(),
                    on_apply: handle_editor_apply,
                    on_cancel: move |_| editor_envelope.set(None),
                }
            }
        } else if let Some(envelope) = aem_preview_envelope.read().clone() {
            // AEM Structure Preview (full page view)
            AemPreview {
                envelope: AemPreviewEnvelope(envelope),
                profile: selected_profile.read().clone(),
                on_close: move |_| aem_preview_envelope.set(None),
            }
        } else {
            // Main app content (scrollable area)
            div { class: "app-scrollable",
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
                        ResultsSection {
                            state: processing_state.read().clone(),
                            on_edit: move |envelope| {
                                editor_envelope.set(Some(envelope));
                            },
                            on_aem_preview: move |envelope| {
                                aem_preview_envelope.set(Some(envelope));
                            },
                        }
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
    }
}
