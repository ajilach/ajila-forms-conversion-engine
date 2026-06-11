mod components;
mod db;
mod markdown;
mod models;
mod pipeline;
mod platform;
#[cfg(not(target_arch = "wasm32"))]
mod preview_server;
mod processing;
mod settings;

use dioxus::prelude::*;

use components::{
    AemPreview, AemPreviewEnvelope, EnvelopeWrapper, FileUploadSection, ImageModal,
    ProgressDisplay, ResultsSection, SettingsPanel, StructuredEditor,
};
use models::{DocumentEnvelope, ProcessingState, ProcessingStep};
use processing::run_and_track;
use settings::AppSettings;

fn main() {
    #[cfg(feature = "desktop")]
    {
        let saved = AppSettings::load();
        let mut config = dioxus::desktop::Config::new().with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_always_on_top(saved.always_on_top)
                .with_inner_size(dioxus::desktop::LogicalSize::new(1400.0, 960.0))
                .with_title("Ajila Forms Conversion Engine"),
        );

        // Window/taskbar icon (no-op on macOS, used on Windows/Linux).
        if let Some(icon) = load_window_icon() {
            config = config.with_icon(icon);
        }

        dioxus::LaunchBuilder::new().with_cfg(config).launch(App);
    }

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
}

/// Decode the bundled PNG into a `tao` window icon.
#[cfg(feature = "desktop")]
fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let rgba = image::load_from_memory(include_bytes!("../icons/icon.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

#[component]
fn App() -> Element {
    let mut processing_state = use_signal(ProcessingState::new);
    let mut is_processing = use_signal(|| false);
    let mut enlarged_image = use_signal(|| None::<(String, String)>);
    let mut selected_profile = use_signal(|| None::<String>);
    let mut editor_envelope = use_signal(|| None::<DocumentEnvelope>);
    let mut settings_open = use_signal(|| false);
    let mut app_settings = use_signal(AppSettings::load);
    let mut aem_preview_envelope = use_signal(|| None::<DocumentEnvelope>);
    // Edit-history session id for the currently loaded document (desktop only).
    let mut current_session = use_signal(|| None::<String>);

    let profiles = blueprint::list_profiles();

    // ── Pipeline ──────────────────────────────────────────────────────────────
    let mut on_process = move |file_data: Vec<(String, Vec<u8>)>| {
        is_processing.set(true);
        current_session.set(None);

        let profile = selected_profile.read().clone();

        // A single JSON file is loaded directly as a structured document,
        // bypassing the PDF pipeline.
        if processing::is_json_upload(&file_data) {
            processing::load_envelope_from_json(
                &file_data,
                profile,
                processing_state,
                current_session,
            );
            is_processing.set(false);
            return;
        }

        processing_state.set(ProcessingState {
            step: ProcessingStep::Parsing,
            ..ProcessingState::new()
        });

        spawn(async move {
            run_and_track(file_data, profile, processing_state, current_session).await;
            is_processing.set(false);
        });
    };

    // ── AI processing ───────────────────────────────────────────────────────
    // Hand the uploaded PDFs to the configured LLM and parse the structured
    // document straight from its response, skipping the core pipeline.
    let mut on_ai_process = move |file_data: Vec<(String, Vec<u8>)>| {
        let pdfs: Vec<(String, Vec<u8>)> = file_data
            .into_iter()
            .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
            .collect();
        if pdfs.is_empty() {
            return;
        }

        is_processing.set(true);
        current_session.set(None);

        let profile = selected_profile.read().clone();
        let settings = app_settings.read().clone();
        let provider = settings.provider;
        let api_key = settings.active_api_key().to_string();
        let model = settings.active_model().to_string();

        processing_state.set(ProcessingState {
            step: ProcessingStep::Parsing,
            ai_mode: true,
            ..ProcessingState::new()
        });

        spawn(async move {
            let session_label = pdfs
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
                .join(", ");

            // Load the selected profile's fonts before rendering — the font
            // store is global and shared with the blocking render thread, so
            // without this the renderer has no typefaces ("no fallback font").
            if let Some(profile_name) = profile.as_deref() {
                let _ = blueprint::load_profile_fonts(profile_name);
            }

            // Run the pipeline up to state rendering (no labelling), streaming
            // the staged steps into the progress UI, and attach the plain page
            // renders as visual references for the model.
            let images =
                pipeline::render_plain_states(&pdfs, |s| processing_state.set(s.clone())).await;

            // Hand off to AI generation: keep the rendered images/steps on
            // screen and advance to the "AI Generation" step.
            processing_state.write().step = ProcessingStep::AiGenerating;

            match components::editor::smart_edit::run_ai_generate(
                &pdfs, &images, provider, &api_key, &model,
            )
            .await
            {
                Ok(nodes) => {
                    // Derive the real Context (language + XFA variables such as
                    // `formrange_code`) from the source PDF, so profile outputs
                    // (AEM/XSD/HTML) generate exactly like the normal pipeline.
                    // Fall back to a language-only context if the PDF can't be
                    // parsed (e.g. a flat/scanned PDF with no XFA).
                    let context = pdfs
                        .iter()
                        .find_map(|(_, bytes)| {
                            blueprint::Blueprint::from_pdf_bytes(bytes)
                                .ok()
                                .map(|bp| bp.context())
                        })
                        .unwrap_or_else(|| {
                            blueprint::Context::with_language(processing::primary_language(&nodes))
                        });
                    let envelope = DocumentEnvelope {
                        context,
                        content: nodes,
                        state_count: 1,
                    };
                    processing::finalize_envelope(
                        &envelope,
                        &pdfs,
                        profile.as_deref(),
                        processing_state,
                        current_session,
                        &session_label,
                        "Generated by AI",
                        true,
                    );
                }
                Err(e) => {
                    // Keep the AiGenerating step so ProgressDisplay stays visible
                    // and shows the error instead of the staged-pipeline view.
                    processing_state.set(ProcessingState {
                        step: ProcessingStep::AiGenerating,
                        ai_mode: true,
                        error: Some(format!("AI processing failed: {e}")),
                        ..ProcessingState::new()
                    });
                }
            }
            is_processing.set(false);
        });
    };

    // Continue a previous editing session: load its latest snapshot and
    // regenerate all derived outputs, then mark the document as complete.
    let mut on_continue_session = move |session_id: String| {
        let Some(seq) = db::latest_seq(&session_id) else {
            return;
        };
        let Some(json) = db::snapshot_at(&session_id, seq) else {
            return;
        };
        let Ok(envelope) = serde_json::from_str::<DocumentEnvelope>(&json) else {
            return;
        };

        // Restore the profile the session was created with so outputs are
        // regenerated with the matching capabilities.
        if let Some(profile) = db::session_profile(&session_id) {
            selected_profile.set(Some(profile));
        }

        let profile = selected_profile.read().clone();
        let mut state = processing_state.write();
        state.step = ProcessingStep::Complete;
        state.error = None;
        processing::regenerate_outputs(&mut state, &envelope, profile.as_deref());
        drop(state);

        current_session.set(Some(session_id));
        editor_envelope.set(Some(envelope));
    };

    // Handle applying changes from the editor
    let handle_editor_apply = move |envelope: DocumentEnvelope| {
        // Update the processing state with the edited envelope and regenerate
        // all derived outputs for the active profile.
        let profile = selected_profile.read().clone();
        let mut state = processing_state.write();
        processing::regenerate_outputs(&mut state, &envelope, profile.as_deref());
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
                alt: "Ajila Company Logo",
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
            settings: app_settings.read().clone(),
            on_settings_changed: move |new_settings: AppSettings| {
                new_settings.save();
                #[cfg(feature = "desktop")]
                dioxus::desktop::use_window().set_always_on_top(new_settings.always_on_top);
                app_settings.set(new_settings);
            },
        }

        // Show either the editor, AEM preview, or the main app content
        if let Some(envelope) = editor_envelope.read().clone() {
            // Structured Editor (full page view)
            div { class: "editor-page",
                StructuredEditor {
                    envelope: EnvelopeWrapper(envelope),
                    plain_images: processing_state.read().plain_images.clone(),
                    provider: app_settings.read().provider,
                    api_key: app_settings.read().active_api_key().to_string(),
                    model: app_settings.read().active_model().to_string(),
                    session_id: current_session.read().clone(),
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
                        on_ai_process: move |files: Vec<(String, Vec<u8>)>| {
                            on_ai_process(files);
                        },
                        ai_available: !app_settings.read().active_api_key().is_empty(),
                        on_continue: move |session_id: String| {
                            on_continue_session(session_id);
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
