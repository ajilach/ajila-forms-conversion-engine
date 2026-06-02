//! Settings panel component.
//!
//! Renders a dropdown panel beneath the settings button in the header.

use dioxus::prelude::*;

use crate::settings::AppSettings;

/// Hardcoded fallback model list, used when models cannot be fetched from the API.
const FALLBACK_MODELS: &[&str] = &[
    "gpt-4.1",
    "gpt-4.1-mini",
    "gpt-4.1-nano",
    "gpt-4o",
    "gpt-4o-mini",
    "o3",
    "o4-mini",
];

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
                    let api_key_for_fetch = settings.openai_api_key.clone();

                    // Fetch available models whenever the API key changes.
                    let models = use_resource(move || {
                        let key = api_key_for_fetch.clone();
                        async move { crate::platform::openai_list_models(&key).await }
                    });

                    let model_list: Vec<String> = match &*models.read() {
                        Some(Ok(list)) if !list.is_empty() => list.clone(),
                        _ => FALLBACK_MODELS.iter().map(|s| s.to_string()).collect(),
                    };

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
                                        if let Ok(port) = e.value().parse::<u16>()
                                            && port >= 1024 {
                                            let mut new_s = s.clone();
                                            new_s.live_preview_port = port;
                                            on_changed.call(new_s);
                                        }
                                    }
                                },
                            }
                        }
                        div { class: "settings-section",
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
                                    span { class: "settings-row-desc", "OpenAI model to use for Smart Edit." }
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
                                    for model_id in model_list.iter() {
                                        option {
                                            value: "{model_id}",
                                            selected: settings_for_model.openai_model == *model_id,
                                            "{model_id}"
                                        }
                                    }
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
