use std::collections::HashMap;

use base64::Engine;
use dioxus::prelude::*;

#[component]
pub fn ImageGrid(
    title: String,
    images: HashMap<String, Vec<u8>>,
    on_image_click: EventHandler<(String, String)>,
) -> Element {
    rsx! {
        div { class: "image-grid",
            h3 { "{title}" }
            div { class: "image-grid-scroll",
                for (state_name , image_bytes) in images.iter() {
                    div { class: "image-card",
                        div { class: "image-card-label", "{state_name}" }
                        img {
                            src: "data:image/png;base64,{base64::prelude::BASE64_STANDARD.encode(image_bytes)}",
                            class: "thumbnail-image",
                            alt: "{state_name}",
                            onclick: {
                                let name = state_name.clone();
                                let data = base64::prelude::BASE64_STANDARD.encode(image_bytes);
                                move |_| on_image_click.call((name.clone(), data.clone()))
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ImageModal(
    name: String,
    data: String,
    on_close: EventHandler<()>,
) -> Element {
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
                    src: "data:image/png;base64,{data}",
                    class: "modal-image",
                    alt: "{name}",
                }
            }
        }
    }
}
