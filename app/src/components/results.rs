use dioxus::prelude::*;

use crate::components::spinner::Spinner;
use crate::models::{DocumentEnvelope, ProcessingState};
use crate::platform::{download_file, show_html_preview};

/// Lifecycle of the on-demand "Upload to AEM" action, surfaced inside the button.
#[derive(Clone, PartialEq)]
enum UploadState {
    Idle,
    Uploading,
    Success,
    Error(String),
}

/// Which edit the user is about to make when a "discard downstream edits"
/// confirmation is showing. Each regenerates the layer below it.
#[derive(Clone, Copy, PartialEq)]
enum PendingEdit {
    /// Edit the structured view — regenerates the AEM tree and the content XML.
    Structure,
    /// Edit the AEM tree — regenerates the content XML.
    Aem,
}

fn filename(prefix: &str, form_code: &Option<String>, ext: &str) -> String {
    match form_code {
        Some(code) => format!("{prefix}-{code}.{ext}"),
        None => format!("{prefix}.{ext}"),
    }
}

#[derive(Clone, PartialEq, Props)]
pub struct ResultsSectionProps {
    /// The current processing state.
    pub state: ProcessingState,
    /// AEM upload connection from settings, or `None` if not configured.
    pub aem_connection: Option<blueprint::AemConnection>,
    /// Whether the AEM tree has unsaved edits that a structured-view edit would
    /// discard. Drives a confirmation prompt on "Edit Structure".
    pub aem_modified: bool,
    /// Whether the raw content XML has unsaved edits that editing the AEM tree
    /// (or the structured view) would discard. Drives a confirmation prompt on
    /// "Edit AEM" and "Edit Structure".
    pub aem_xml_modified: bool,
    /// Callback when the Edit Structure button is clicked.
    /// Passes the envelope to edit.
    pub on_edit: EventHandler<DocumentEnvelope>,
    /// Callback when the Preview AEM Structure button is clicked.
    pub on_aem_preview: EventHandler<DocumentEnvelope>,
    /// Callback when the Edit AEM button is clicked.
    pub on_aem_edit: EventHandler<DocumentEnvelope>,
    /// Callback when the Edit content XML button is clicked.
    pub on_aem_xml_edit: EventHandler<DocumentEnvelope>,
}

#[component]
pub fn ResultsSection(props: ResultsSectionProps) -> Element {
    let state = &props.state;
    // Lifecycle of the on-demand AEM upload, reported inside the button.
    let mut upload_state = use_signal(|| UploadState::Idle);
    // Which edit (if any) is awaiting "discard downstream edits" confirmation.
    let mut pending_edit = use_signal(|| None::<PendingEdit>);

    let has_edit_group = state.envelope.is_some() || state.html_preview.is_some();
    let has_download_group =
        state.merged_json.is_some() || state.aem_package.is_some() || state.xsd_schema.is_some();

    rsx! {
        div { class: "results-container",

            h2 { "✓ Processing Complete!" }

            // ── Edit & Preview ────────────────────────────────────────────
            if has_edit_group {
                div { class: "results-group",
                    span { class: "results-group-label", "Edit & Preview" }
                    div { class: "results-actions",
                        if let Some(envelope) = state.envelope.clone() {
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let on_edit = props.on_edit;
                                    let envelope_clone = envelope.clone();
                                    let aem_modified = props.aem_modified;
                                    let aem_xml_modified = props.aem_xml_modified;
                                    move |_| {
                                        // Editing the structure regenerates both the AEM
                                        // tree and the content XML, so warn if either has
                                        // unsaved edits.
                                        if aem_modified || aem_xml_modified {
                                            pending_edit.set(Some(PendingEdit::Structure));
                                        } else {
                                            on_edit.call(envelope_clone.clone());
                                        }
                                    }
                                },
                                "✎ Edit Structure"
                            }
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let on_aem_edit = props.on_aem_edit;
                                    let envelope_clone = envelope.clone();
                                    let aem_xml_modified = props.aem_xml_modified;
                                    move |_| {
                                        // Editing the AEM tree regenerates the content XML,
                                        // so warn if the raw XML has unsaved edits.
                                        if aem_xml_modified {
                                            pending_edit.set(Some(PendingEdit::Aem));
                                        } else {
                                            on_aem_edit.call(envelope_clone.clone());
                                        }
                                    }
                                },
                                "✎ Edit AEM"
                            }
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let on_aem_xml_edit = props.on_aem_xml_edit;
                                    let envelope_clone = envelope.clone();
                                    move |_| on_aem_xml_edit.call(envelope_clone.clone())
                                },
                                "✎ Edit content XML"
                            }
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let on_aem_preview = props.on_aem_preview;
                                    let envelope_clone = envelope.clone();
                                    move |_| on_aem_preview.call(envelope_clone.clone())
                                },
                                "⊞ Preview AEM"
                            }
                        }
                        if let Some(ref html_preview) = state.html_preview {
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let html_preview = html_preview.clone();
                                    let preview_filename = filename("preview", &state.form_code, "html");
                                    move |_| show_html_preview(html_preview.clone(), &preview_filename)
                                },
                                "◹ HTML Preview"
                            }
                        }
                    }
                }
            }

            // ── Download ──────────────────────────────────────────────────
            if has_download_group {
                div { class: "results-group",
                    span { class: "results-group-label", "Download" }
                    div { class: "results-actions",
                        if let Some(ref json_data) = state.merged_json {
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let json_data = json_data.clone();
                                    let json_filename = filename("structure", &state.form_code, "json");
                                    move |_| download_file(json_data.as_bytes(), &json_filename, "application/json")
                                },
                                "Structure JSON"
                            }
                        }
                        if let Some(ref aem_data) = state.aem_package {
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let aem_data = aem_data.clone();
                                    let zip_filename = filename("forms-package", &state.form_code, "zip");
                                    move |_| download_file(&aem_data, &zip_filename, "application/zip")
                                },
                                "AEM Package"
                            }
                        }
                        if let Some(ref xsd_data) = state.xsd_schema {
                            button {
                                class: "btn btn-secondary btn-lg",
                                onclick: {
                                    let xsd_data = xsd_data.clone();
                                    let xsd_filename = filename("schema", &state.form_code, "xsd");
                                    move |_| download_file(xsd_data.as_bytes(), &xsd_filename, "application/xml")
                                },
                                "XSD Schema"
                            }
                        }
                    }
                }
            }

            // ── AEM ───────────────────────────────────────────────────────
            if let Some(ref aem_data) = state.aem_package {
                {
                    let st = upload_state.read().clone();
                    let uploading = st == UploadState::Uploading;
                    let no_connection = props.aem_connection.is_none();
                    let title = match &st {
                        UploadState::Error(msg) => msg.clone(),
                        _ if no_connection => {
                            "Configure the AEM connection in Settings to enable this".to_string()
                        }
                        _ => "Upload and install the package on the configured AEM instance".to_string(),
                    };

                    rsx! {
                        div { class: "results-group",
                            span { class: "results-group-label", "AEM" }
                            div { class: "results-actions",
                                button {
                                    class: "btn btn-primary btn-lg",
                                    disabled: uploading || no_connection,
                                    title,
                                    onclick: {
                                        let aem_data = aem_data.clone();
                                        let connection = props.aem_connection.clone();
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
            }

            // Warn before an edit regenerates a lower layer and discards its edits.
            if let Some(kind) = pending_edit() {
                {
                    // What the pending edit would discard, and the message to show.
                    let (title, body): (&str, String) = match kind {
                        PendingEdit::Structure => {
                            let lost = match (props.aem_modified, props.aem_xml_modified) {
                                (true, true) => "AEM tree and content XML edits",
                                (true, false) => "AEM tree edits",
                                _ => "content XML edits",
                            };
                            (
                                "Discard downstream edits?",
                                format!(
                                    "Editing the structured view regenerates the AEM tree and the \
                                     content XML from the structure, which will reset your {lost}. \
                                     Continue?"
                                ),
                            )
                        }
                        PendingEdit::Aem => (
                            "Discard content XML edits?",
                            "Editing the AEM tree regenerates the content XML from the tree, which \
                             will reset your raw content XML edits. Continue?"
                                .to_string(),
                        ),
                    };
                    rsx! {
                        div { class: "modal-overlay", onclick: move |_| pending_edit.set(None),
                            div {
                                class: "modal-content confirm-dialog",
                                onclick: move |evt| evt.stop_propagation(),
                                div { class: "modal-title", "{title}" }
                                p { class: "confirm-dialog-text", "{body}" }
                                div { class: "confirm-dialog-actions",
                                    button {
                                        class: "btn btn-secondary",
                                        onclick: move |_| pending_edit.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-primary",
                                        onclick: {
                                            let on_edit = props.on_edit;
                                            let on_aem_edit = props.on_aem_edit;
                                            let envelope = state.envelope.clone();
                                            move |_| {
                                                pending_edit.set(None);
                                                if let Some(env) = envelope.clone() {
                                                    match kind {
                                                        PendingEdit::Structure => on_edit.call(env),
                                                        PendingEdit::Aem => on_aem_edit.call(env),
                                                    }
                                                }
                                            }
                                        },
                                        "Edit anyway"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
