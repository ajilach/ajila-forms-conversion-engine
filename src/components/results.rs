use dioxus::prelude::*;

use crate::models::ProcessingState;
use crate::platform::{download_file, show_html_preview};

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
                            move |_| {
                                show_html_preview(html_preview.clone());
                            }
                        },
                        "Preview as HTML Form"
                    }
                }

                // Download JSON button
                if let Some(ref json_data) = state.merged_json {
                    button {
                        class: "btn btn-success btn-lg",
                        onclick: {
                            let json_data = json_data.clone();
                            move |_| {
                                download_file(
                                    json_data.as_bytes(),
                                    "merged_structure.json",
                                    "application/json",
                                );
                            }
                        },
                        "Download Structure JSON"
                    }
                }

                // AEM Package Download button
                if let Some(ref aem_data) = state.aem_package {
                    button {
                        class: "btn btn-success btn-lg",
                        onclick: {
                            let aem_data = aem_data.clone();
                            move |_| {
                                download_file(&aem_data, "aem_forms_package.zip", "application/zip");
                            }
                        },
                        "Download AEM Package"
                    }
                }
            }
        }
    }
}
