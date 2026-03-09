use dioxus::prelude::*;

use crate::models::ProcessingState;
use crate::platform::{download_file, show_html_preview};

fn filename(prefix: &str, form_code: &Option<String>, ext: &str) -> String {
    match form_code {
        Some(code) => format!("{prefix}-{code}.{ext}"),
        None => format!("{prefix}.{ext}"),
    }
}

#[component]
pub fn ResultsSection(state: ProcessingState) -> Element {
    rsx! {
        div { class: "results-container",

            h2 { "✓ Processing Complete!" }

            div { class: "results-actions",

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
            }
        }
    }
}
