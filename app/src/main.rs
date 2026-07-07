mod agent_runner;
mod ai_tools;
mod components;
mod markdown;
mod mcp_install;
mod models;
mod pipeline;
mod platform;
mod preview_server;
mod processing;
mod settings;

// The headless engine layer (edit-history store, reference store, AEM client)
// lives in the `agent` crate. Re-export it under the historical `crate::*`
// paths so the rest of the app is unchanged.
pub use agent::{aem_client, db, references};

use dioxus::prelude::*;

use components::{
    AemConfigWrapper, AemConnWrapper, AemEditor, AemPreview, AemPreviewEnvelope, AemRootWrapper,
    AemXmlEditor, AgentFlow, EnvelopeWrapper, FileUploadSection, ImageModal, ProgressDisplay,
    ReferencesPage, ResultsSection, SettingsPage, StructuredEditor, TranslationsWrapper,
};
use models::{DocumentEnvelope, ProcessingState, ProcessingStep};
use processing::run_and_track;
use settings::AppSettings;

fn main() {
    let saved = AppSettings::load();
    saved.apply_runtime_config();
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

/// Decode the bundled PNG into a `tao` window icon.
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
    let mut enlarged_image = use_signal(|| None::<(String, Vec<String>)>);
    let mut selected_profile = use_signal(|| None::<String>);
    let mut editor_envelope = use_signal(|| None::<DocumentEnvelope>);
    let mut settings_open = use_signal(|| false);
    // Whether the full-page reference-forms manager is open.
    let mut references_open = use_signal(|| false);
    let mut app_settings = use_signal(AppSettings::load);
    let mut aem_preview_envelope = use_signal(|| None::<DocumentEnvelope>);
    let mut aem_editor_envelope = use_signal(|| None::<DocumentEnvelope>);
    let mut aem_xml_editor_envelope = use_signal(|| None::<DocumentEnvelope>);
    // Edit-history session id for the currently loaded document (desktop only).
    let mut current_session = use_signal(|| None::<String>);
    // Source PDF bytes of the currently loaded document, retained so Smart Edit
    // can expose the same form-inspection tools as AI processing. Empty when the
    // document has no PDF source (e.g. JSON/AEM input, or a reopened session).
    let mut source_pdfs = use_signal(Vec::<(String, Vec<u8>)>::new);
    // Whether the AEM structure has been edited in the AEM editor since the last
    // (re)generation from the structured document. AEM edits live only in the
    // generated package; editing the structured view regenerates that package
    // and discards them, so we warn before that happens.
    let mut aem_modified = use_signal(|| false);
    let mut aem_xml_modified = use_signal(|| false);

    let profiles = blueprint::list_profiles();

    // ── Pipeline ──────────────────────────────────────────────────────────────
    let mut on_process = move |file_data: Vec<(String, Vec<u8>)>| {
        is_processing.set(true);
        current_session.set(None);
        aem_modified.set(false);
        aem_xml_modified.set(false);

        let profile = selected_profile.read().clone();

        // A single JSON file is loaded directly as a structured document,
        // bypassing the PDF pipeline.
        if processing::is_json_upload(&file_data) {
            source_pdfs.set(Vec::new());
            processing::load_envelope_from_json(
                &file_data,
                profile,
                processing_state,
                current_session,
            );
            is_processing.set(false);
            return;
        }

        // Retain the PDF sources so Smart Edit gets the same tools as AI processing.
        source_pdfs.set(
            file_data
                .iter()
                .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
                .cloned()
                .collect(),
        );

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
            .iter()
            .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
            .cloned()
            .collect();
        // An AEM content-package ZIP may be attached as an editable template for
        // the agent's working tree. Proceed with PDFs, a template, or both.
        let has_template = file_data
            .iter()
            .any(|(_, bytes)| blueprint::detect_aem_zip(bytes));
        if pdfs.is_empty() && !has_template {
            return;
        }

        is_processing.set(true);
        current_session.set(None);
        aem_modified.set(false);
        aem_xml_modified.set(false);
        // Retain the PDF sources so Smart Edit gets the same tools as AI processing.
        source_pdfs.set(pdfs.clone());

        let profile = selected_profile.read().clone();
        let settings = app_settings.read().clone();

        processing_state.set(ProcessingState {
            step: ProcessingStep::Parsing,
            ai_mode: true,
            ..ProcessingState::new()
        });

        spawn(async move {
            let session_label = file_data
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
                .join(", ");

            // Load the selected profile's fonts so on-demand renders have the
            // right typefaces (the font store is global, shared with rendering).
            if let Some(profile_name) = profile.as_deref() {
                let _ = blueprint::load_profile_fonts(profile_name);
            }

            // Hand the whole conversion to the autonomous agent: it drives the
            // engine via tools (extract → structure → convert → AEM → package →
            // upload/verify), versioning each step, and finalizes the result.
            // The full file set is passed so an attached content-package ZIP can
            // be pre-loaded as the agent's editable working tree.
            crate::agent_runner::run_agent(
                file_data,
                profile,
                settings,
                session_label,
                processing_state,
                current_session,
            )
            .await;
            is_processing.set(false);
        });
    };

    // ── Agent feedback re-run ─────────────────────────────────────────────────
    // From the agent "done" screen the user can submit feedback; this resumes
    // the agent in the same session to refine the result and returns the flow
    // to the in-progress (running) phase.
    let mut on_ai_feedback = move |feedback: String| {
        let Some(session) = current_session.read().clone() else {
            return;
        };
        let pdfs = source_pdfs.read().clone();
        if pdfs.is_empty() {
            return;
        }

        let profile = selected_profile.read().clone();
        let settings = app_settings.read().clone();

        is_processing.set(true);
        aem_modified.set(false);
        aem_xml_modified.set(false);
        // Return to the running phase with a fresh activity log.
        processing_state.set(ProcessingState {
            step: ProcessingStep::Parsing,
            ai_mode: true,
            ..ProcessingState::new()
        });

        spawn(async move {
            crate::agent_runner::run_agent_feedback(
                feedback,
                pdfs,
                profile,
                settings,
                session,
                processing_state,
                current_session,
            )
            .await;
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

        // Reopened sessions don't carry the source PDFs, so Smart Edit falls
        // back to the rendered page images only.
        source_pdfs.set(Vec::new());
        aem_modified.set(false);
        aem_xml_modified.set(false);
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

        // The AEM package was just regenerated from the structure, so any prior
        // AEM-tree or content-XML edits are gone — clear the modified flags.
        aem_modified.set(false);
        aem_xml_modified.set(false);

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

        // Show either the settings, editor, AEM preview, references manager, or
        // main content (full-page views, under the persistent header).
        if *settings_open.read() {
            SettingsPage {
                on_close: move |_| settings_open.set(false),
                settings: app_settings.read().clone(),
                on_settings_changed: move |new_settings: AppSettings| {
                    new_settings.save();
                    new_settings.apply_runtime_config();
                    dioxus::desktop::use_window().set_always_on_top(new_settings.always_on_top);
                    app_settings.set(new_settings);
                },
                on_open_references: move |_| {
                    settings_open.set(false);
                    references_open.set(true);
                },
            }
        } else if *references_open.read() {
            // Reference-forms manager (full page view)
            ReferencesPage {
                profile: selected_profile.read().clone(),
                settings: app_settings.read().clone(),
                on_close: move |_| references_open.set(false),
            }
        } else if let Some(envelope) = editor_envelope.read().clone() {
            // Structured Editor (full page view)
            div { class: "editor-page",
                StructuredEditor {
                    envelope: EnvelopeWrapper(envelope),
                    plain_images: processing_state.read().plain_images.clone(),
                    source_pdfs: source_pdfs.read().clone(),
                    api_key: app_settings.read().active_api_key().to_string(),
                    model: app_settings.read().active_model().to_string(),
                    smart_edit_instructions: app_settings.read().smart_edit_instructions.clone(),
                    session_id: current_session.read().clone(),
                    profile: selected_profile.read().clone(),
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
        } else if let Some(envelope) = aem_editor_envelope.read().clone() {
            // AEM Node Editor (full page view)
            {
                let profile = selected_profile.read().clone();
                let config = profile
                    .as_deref()
                    .filter(|p| blueprint::has_aem_config(p))
                    .and_then(|p| blueprint::load_aem_config(p, &envelope.context).ok());
                match config {
                    Some(cfg) => {
                        let root = blueprint::convert_to_aem(&envelope.content, &cfg);
                        let conn = app_settings.read().aem_connection();
                        let master_lang = cfg.master_language.clone();
                        let content_translations = blueprint::aem_translations_from_content(
                            &envelope.content,
                            &cfg.master_language,
                        );
                        // Show a tab per locale present anywhere in the document —
                        // mirroring the structured editor, which derives its tabs
                        // from the document itself rather than the AEM config alone.
                        // Union: config languages + master + context languages +
                        // every locale appearing in the content translations.
                        let languages: Vec<String> = {
                            let mut set: std::collections::BTreeSet<String> =
                                std::collections::BTreeSet::new();
                            set.insert(cfg.master_language.clone());
                            set.extend(cfg.languages.iter().cloned());
                            for l in envelope.context.language().split(',') {
                                let l = l.trim();
                                if !l.is_empty() {
                                    set.insert(l.to_string());
                                }
                            }
                            for langs in content_translations.values() {
                                set.extend(langs.keys().cloned());
                            }
                            set.into_iter().collect()
                        };
                        let form_code = cfg.form_code.clone();
                        rsx! {
                            div { class: "editor-page",
                                AemEditor {
                                    root: AemRootWrapper(root),
                                    plain_images: processing_state.read().plain_images.clone(),
                                    source_pdfs: source_pdfs.read().clone(),
                                    aem_config: AemConfigWrapper(cfg),
                                    connection: AemConnWrapper(conn),
                                    master_lang,
                                    languages,
                                    content_translations,
                                    api_key: app_settings.read().active_api_key().to_string(),
                                    model: app_settings.read().active_model().to_string(),
                                    smart_edit_instructions: app_settings.read().aem_smart_edit_instructions.clone(),
                                    // Dedicated AEM history session, derived from the document's
                                    // structured session so it resets per document and never
                                    // collides with the (structured) session browser.
                                    session_id: current_session.read().clone().map(|s| format!("{s}#aem")),
                                    profile: profile.clone(),
                                    on_apply: move |zip: Vec<u8>| {
                                        let mut state = processing_state.write();
                                        state.aem_package = Some(zip);
                                        state.form_code = Some(form_code.clone());
                                        drop(state);
                                        // AEM edits now diverge from the structure; warn before a
                                        // structured edit regenerates (and discards) them. The
                                        // package was rebuilt from the tree, so any raw content-XML
                                        // edits are gone — clear that flag.
                                        aem_modified.set(true);
                                        aem_xml_modified.set(false);
                                        aem_editor_envelope.set(None);
                                    },
                                    on_cancel: move |_| aem_editor_envelope.set(None),
                                }
                            }
                        }
                    }
                    None => rsx! {
                        div { class: "editor-page",
                            p { "This profile has no AEM configuration." }
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| aem_editor_envelope.set(None),
                                "Close"
                            }
                        }
                    },
                }
            }
        } else if let Some(envelope) = aem_xml_editor_envelope.read().clone() {
            // AEM content-XML plain-text editor (full page view)
            {
                let profile = selected_profile.read().clone();
                let config = profile
                    .as_deref()
                    .filter(|p| blueprint::has_aem_config(p))
                    .and_then(|p| blueprint::load_aem_config(p, &envelope.context).ok());
                match config {
                    Some(cfg) => {
                        let root = blueprint::convert_to_aem(&envelope.content, &cfg);
                        let translations = blueprint::aem_translations_from_content(
                            &envelope.content,
                            &cfg.master_language,
                        );
                        let initial_xml = blueprint::generate_aem_xml(&root, &cfg);
                        let form_code = cfg.form_code.clone();
                        rsx! {
                            div { class: "editor-page",
                                AemXmlEditor {
                                    root: AemRootWrapper(root),
                                    aem_config: AemConfigWrapper(cfg),
                                    translations: TranslationsWrapper(translations),
                                    initial_xml,
                                    // Shares the agent's XML history session, so manual
                                    // and agent edits land on one timeline.
                                    session_id: current_session.read().clone().map(|s| format!("{s}#aem-xml")),
                                    on_apply: move |zip: Vec<u8>| {
                                        let mut state = processing_state.write();
                                        state.aem_package = Some(zip);
                                        state.form_code = Some(form_code.clone());
                                        drop(state);
                                        // The package now uses raw content-XML edits that diverge
                                        // from the AEM tree; warn before an AEM-tree or structured
                                        // edit regenerates (and discards) them.
                                        aem_xml_modified.set(true);
                                        aem_xml_editor_envelope.set(None);
                                    },
                                    on_cancel: move |_| aem_xml_editor_envelope.set(None),
                                }
                            }
                        }
                    }
                    None => rsx! {
                        div { class: "editor-page",
                            p { "This profile has no AEM configuration." }
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| aem_xml_editor_envelope.set(None),
                                "Close"
                            }
                        }
                    },
                }
            }
        } else if !app_settings.read().legacy_agent_ui {
            // Simplified agent flow: upload → live timeline → done (default).
            AgentFlow {
                processing_state,
                is_processing,
                profiles: profiles.clone(),
                selected_profile,
                ai_available: !app_settings.read().active_api_key().is_empty(),
                aem_connection: app_settings.read().aem_connection(),
                on_ai_process: move |files: Vec<(String, Vec<u8>)>| {
                    on_ai_process(files);
                },
                on_feedback: move |text: String| {
                    on_ai_feedback(text);
                },
                on_reset: move |_| {
                    is_processing.set(false);
                    current_session.set(None);
                    source_pdfs.set(Vec::new());
                    aem_modified.set(false);
                    aem_xml_modified.set(false);
                    processing_state.set(ProcessingState::new());
                },
            }
        } else {
            // Legacy stacked layout (upload + progress + results), plus normal
            // (non-agent) processing and "Continue editing".
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
                            aem_connection: app_settings.read().aem_connection(),
                            aem_modified: *aem_modified.read(),
                            aem_xml_modified: *aem_xml_modified.read(),
                            on_edit: move |envelope| {
                                editor_envelope.set(Some(envelope));
                            },
                            on_aem_preview: move |envelope| {
                                aem_preview_envelope.set(Some(envelope));
                            },
                            on_aem_edit: move |envelope| {
                                aem_editor_envelope.set(Some(envelope));
                            },
                            on_aem_xml_edit: move |envelope| {
                                aem_xml_editor_envelope.set(Some(envelope));
                            },
                        }
                    }

                    // Image Modal Overlay
                    if let Some((name, pages)) = enlarged_image.read().as_ref() {
                        ImageModal {
                            name: name.clone(),
                            pages: pages.clone(),
                            on_close: move |_| enlarged_image.set(None),
                        }
                    }
                }
            }
        }
    }
}
