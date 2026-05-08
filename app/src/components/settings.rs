//! Settings panel component.
//!
//! Renders a dropdown panel beneath the settings button in the header.
//! Currently exposes the "always on top" window behaviour toggle (desktop only).

use dioxus::prelude::*;

#[component]
pub fn SettingsPanel(
    /// Whether the panel is visible.
    open: bool,
    /// Called when the user closes the panel (e.g. click outside).
    on_close: EventHandler<()>,
    /// Current value of the always-on-top setting.
    always_on_top: bool,
    /// Called when the user toggles always-on-top.
    on_toggle_always_on_top: EventHandler<bool>,
) -> Element {
    if !open {
        return rsx! {};
    }

    rsx! {
        // Invisible full-screen backdrop — clicking outside closes the dropdown.
        div {
            class: "settings-backdrop",
            onclick: move |_| on_close.call(()),
        }

        // Dropdown panel — positioned via CSS relative to the header button.
        div { class: "settings-dropdown",

            // ── Window behaviour (desktop only) ──────────────────────────────
            {
                #[cfg(not(target_arch = "wasm32"))]
                rsx! {
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "Window" }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Always on top" }
                                span { class: "settings-row-desc",
                                    "Keep the window above all other applications."
                                }
                            }
                            label { class: "toggle-switch",
                                input {
                                    r#type: "checkbox",
                                    checked: always_on_top,
                                    onchange: move |e| on_toggle_always_on_top.call(e.checked()),
                                }
                                span { class: "toggle-slider" }
                            }
                        }
                    }
                }

                #[cfg(target_arch = "wasm32")]
                rsx! {
                    p { class: "settings-no-options",
                        "No configurable settings for the web version."
                    }
                }
            }
        }
    }
}