use std::collections::HashMap;

use dioxus::prelude::*;

/// Build an `<img>` data URL, detecting PNG vs JPEG from the base64 prefix
/// (`iVBOR` → PNG, `/9j/` → JPEG). Plain renders are JPEG-compressed; labelled
/// renders stay PNG.
fn image_data_uri(b64: &str) -> String {
    let mime = if b64.starts_with("/9j/") {
        "image/jpeg"
    } else {
        "image/png"
    };
    format!("data:{mime};base64,{b64}")
}

#[component]
pub fn ImageGrid(
    title: String,
    /// label → per-page base64 images (page order).
    images: HashMap<String, Vec<String>>,
    /// Fired with the clicked state's label and all of its page images.
    on_image_click: EventHandler<(String, Vec<String>)>,
) -> Element {
    // Stable display order (HashMap iteration is otherwise non-deterministic).
    let mut entries: Vec<(&String, &Vec<String>)> = images.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    rsx! {
        div { class: "image-grid",
            h3 { "{title}" }
            div { class: "image-grid-scroll",
                for (state_name , pages) in entries {
                    if let Some(first) = pages.first() {
                        div { class: "image-card",
                            div { class: "image-card-label",
                                if pages.len() > 1 {
                                    "{state_name} ({pages.len()} pages)"
                                } else {
                                    "{state_name}"
                                }
                            }
                            img {
                                src: image_data_uri(first),
                                class: "thumbnail-image",
                                alt: "{state_name}",
                                onclick: {
                                    let name = state_name.clone();
                                    let data = pages.clone();
                                    move |_| on_image_click.call((name.clone(), data.clone()))
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ImageModal(name: String, pages: Vec<String>, on_close: EventHandler<()>) -> Element {
    let page_count = pages.len();
    let mut current_page = use_signal(|| 0usize);

    // Guard against an out-of-range index if the same modal is reused for a
    // state with fewer pages.
    let idx = (*current_page.read()).min(page_count.saturating_sub(1));
    let current = pages.get(idx).cloned().unwrap_or_default();

    rsx! {
        div { class: "modal-overlay", onclick: move |_| on_close.call(()),

            div {
                class: "modal-content",
                onclick: move |evt| evt.stop_propagation(),

                button {
                    class: "modal-close-btn",
                    onclick: move |_| on_close.call(()),
                    "×"
                }

                div { class: "modal-title", "{name}" }

                img {
                    src: image_data_uri(&current),
                    class: "modal-image",
                    alt: "{name}",
                }

                if page_count > 1 {
                    div { class: "modal-pagination",
                        button {
                            class: "modal-page-btn",
                            disabled: idx == 0,
                            onclick: move |evt| {
                                evt.stop_propagation();
                                let cur = *current_page.read();
                                if cur > 0 {
                                    current_page.set(cur - 1);
                                }
                            },
                            "‹ Prev"
                        }
                        span { class: "modal-page-indicator", "Page {idx + 1} / {page_count}" }
                        button {
                            class: "modal-page-btn",
                            disabled: idx + 1 >= page_count,
                            onclick: move |evt| {
                                evt.stop_propagation();
                                let cur = *current_page.read();
                                if cur + 1 < page_count {
                                    current_page.set(cur + 1);
                                }
                            },
                            "Next ›"
                        }
                    }
                }
            }
        }
    }
}
