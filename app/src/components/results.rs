use dioxus::prelude::*;

use crate::models::{DocumentEnvelope, ProcessingState};
use crate::platform::{download_file, show_html_preview};

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
    /// Callback when the Edit Structure button is clicked.
    /// Passes the envelope to edit.
    pub on_edit: EventHandler<DocumentEnvelope>,
    /// Callback when the Preview AEM Structure button is clicked.
    pub on_aem_preview: EventHandler<DocumentEnvelope>,
    /// Callback when the Edit AEM button is clicked.
    pub on_aem_edit: EventHandler<DocumentEnvelope>,
}

#[component]
pub fn ResultsSection(props: ResultsSectionProps) -> Element {
    let state = &props.state;

    rsx! {
        div { class: "results-container",

            h2 { "✓ Processing Complete!" }

            div { class: "results-actions",

                // Edit Structure button (shown first, before download buttons)
                if let Some(envelope) = state.envelope.clone() {
                    button {
                        class: "btn btn-secondary btn-lg",
                        onclick: {
                            let on_edit = props.on_edit;
                            let envelope_clone = envelope.clone();
                            move |_| {
                                on_edit.call(envelope_clone.clone());
                            }
                        },
                        "✎ Edit Structure"
                    }

                    // Preview AEM Structure button
                    button {
                        class: "btn btn-secondary btn-lg",
                        onclick: {
                            let on_aem_preview = props.on_aem_preview;
                            let envelope_clone = envelope.clone();
                            move |_| {
                                on_aem_preview.call(envelope_clone.clone());
                            }
                        },
                        "⊞ Preview AEM Structure"
                    }

                    // Edit AEM Structure button
                    button {
                        class: "btn btn-secondary btn-lg",
                        onclick: {
                            let on_aem_edit = props.on_aem_edit;
                            let envelope_clone = envelope.clone();
                            move |_| {
                                on_aem_edit.call(envelope_clone.clone());
                            }
                        },
                        "✎ Edit AEM Structure"
                    }
                }

                // HTML Preview button
                if let Some(ref html_preview) = state.html_preview {
                    button {
                        class: "btn btn-primary btn-lg",
                        onclick: {
                            let html_preview = html_preview.clone();
                            let preview_filename = filename("preview", &state.form_code, "html");
                            move |_| {
                                show_html_preview(html_preview.clone(), &preview_filename);
                            }
                        },
                        "Preview as HTML Form"
                    }
                }

                // Download JSON button
                if let Some(ref json_data) = state.merged_json {
                    button {
                        class: "btn btn-primary btn-lg",
                        onclick: {
                            let json_data = json_data.clone();
                            let json_filename = filename("structure", &state.form_code, "json");
                            move |_| {
                                download_file(json_data.as_bytes(), &json_filename, "application/json");
                            }
                        },
                        "Download Structure JSON"
                    }
                }

                // AEM Package Download button
                if let Some(ref aem_data) = state.aem_package {
                    button {
                        class: "btn btn-primary btn-lg",
                        onclick: {
                            let aem_data = aem_data.clone();
                            let zip_filename = filename("forms-package", &state.form_code, "zip");
                            move |_| {
                                download_file(&aem_data, &zip_filename, "application/zip");
                            }
                        },
                        "Download AEM Package"
                    }
                }

                // XSD Schema Download button
                if let Some(ref xsd_data) = state.xsd_schema {
                    button {
                        class: "btn btn-primary btn-lg",
                        onclick: {
                            let xsd_data = xsd_data.clone();
                            let xsd_filename = filename("schema", &state.form_code, "xsd");
                            move |_| {
                                download_file(xsd_data.as_bytes(), &xsd_filename, "application/xml");
                            }
                        },
                        "Download XSD Schema"
                    }
                }
            }
        }
    }
}
