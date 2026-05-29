//! Settings panel component.
//!
//! Renders a dropdown panel beneath the settings button in the header.

use dioxus::prelude::*;

use crate::settings::AppSettings;

#[component]
pub fn SettingsPanel(
    /// Whether the panel is visible.
    open: bool,
    /// Called when the user closes the panel (e.g. click outside).
    on_close: EventHandler<()>,
    /// Current settings.
    settings: AppSettings,
    /// Called when the user changes any setting.
    on_settings_changed: EventHandler<AppSettings>,
) -> Element {
    if !open {
        return rsx! {};
    }

    rsx! {
        // Invisible full-screen backdrop — clicking outside closes the dropdown.
        div { class: "settings-backdrop", onclick: move |_| on_close.call(()) }

        // Dropdown panel — positioned via CSS relative to the header button.
        div { class: "settings-dropdown",

            // ── Window behaviour (desktop only) ──────────────────────────────
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let settings_for_aot = settings.clone();
                    let settings_for_port = settings.clone();
                    let settings_for_apikey = settings.clone();
                    let settings_for_model = settings.clone();
                    rsx! {
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "Window" }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "Always on top" }
                                    span { class: "settings-row-desc", "Keep the window above all other applications." }
                                }
                                label { class: "toggle-switch",
                                    input {
                                        r#type: "checkbox",
                                        checked: settings_for_aot.always_on_top,
                                        onchange: {
                                            let on_changed = on_settings_changed;
                                            let s = settings_for_aot.clone();
                                            move |e: Event<FormData>| {
                                                let mut new_s = s.clone();
                                                new_s.always_on_top = e.checked();
                                                on_changed.call(new_s);
                                            }
                                        },
                                    }
                                    span { class: "toggle-slider" }
                                }
                            }
                        }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Live Preview Port" }
                                span { class: "settings-row-desc",
                                    "Local port for the live HTML preview server. Requires restart."
                                }
                            }
                            input {
                                class: "settings-input-port",
                                r#type: "number",
                                min: "1024",
                                max: "65535",
                                value: "{settings_for_port.live_preview_port}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_port.clone();
                                    move |e: Event<FormData>| {
                                        if let Ok(port) = e.value().parse::<u16>() {
                                            if port >= 1024 {
                                                let mut new_s = s.clone();
                                                new_s.live_preview_port = port;
                                                on_changed.call(new_s);
                                            }
                                        }
                                    }
                                },
                            }
                        }
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "AI (Smart Edit)" }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "OpenAI API Key" }
                                    span { class: "settings-row-desc",
                                        "Paste your OpenAI API key here. Used for Smart Edit. Stored locally on disk."
                                    }
                                }
                                input {
                                    class: "settings-input-apikey",
                                    r#type: "password",
                                    placeholder: "sk-...",
                                    value: "{settings_for_apikey.openai_api_key}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_apikey.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.openai_api_key = e.value().trim().to_string();
                                            on_changed.call(new_s);
                                        }
                                    },
                                }
                            }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "Model" }
                                    span { class: "settings-row-desc",
                                        "OpenAI model to use for Smart Edit."
                                    }
                                }
                                select {
                                    class: "settings-select-model",
                                    value: "{settings_for_model.openai_model}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_model.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.openai_model = e.value();
                                            on_changed.call(new_s);
                                        }
                                    },
                                    option { value: "gpt-4o", selected: settings_for_model.openai_model == "gpt-4o", "GPT-4o" }
                                    option { value: "gpt-4o-mini", selected: settings_for_model.openai_model == "gpt-4o-mini", "GPT-4o mini" }
                                    option { value: "gpt-4.1", selected: settings_for_model.openai_model == "gpt-4.1", "GPT-4.1" }
                                    option { value: "gpt-4.1-mini", selected: settings_for_model.openai_model == "gpt-4.1-mini", "GPT-4.1 mini" }
                                    option { value: "gpt-4.1-nano", selected: settings_for_model.openai_model == "gpt-4.1-nano", "GPT-4.1 nano" }
                                    option { value: "o3", selected: settings_for_model.openai_model == "o3", "o3" }
                                    option { value: "o4-mini", selected: settings_for_model.openai_model == "o4-mini", "o4-mini" }
                                }
                            }
                        }
                    }
                }

                #[cfg(target_arch = "wasm32")]
                rsx! {
                    p { class: "settings-no-options", "No configurable settings for the web version." }
                }
            }
        }
    }
}
