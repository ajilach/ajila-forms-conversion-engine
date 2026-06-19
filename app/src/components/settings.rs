//! Settings panel component.
//!
//! Renders a dropdown panel beneath the settings button in the header.

use dioxus::prelude::*;

use crate::settings::AppSettings;

/// Hardcoded fallback Anthropic model list, used when models cannot be fetched from the API.
const ANTHROPIC_FALLBACK_MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
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
                let section = {
                    let settings_for_aot = settings.clone();
                    let settings_for_port = settings.clone();
                    let settings_for_apikey = settings.clone();
                    let settings_for_model = settings.clone();
                    let settings_for_aem_host = settings.clone();
                    let settings_for_aem_user = settings.clone();
                    let settings_for_aem_pass = settings.clone();

                    let active_api_key = settings.active_api_key().to_string();
                    let active_model = settings.active_model().to_string();

                    // Reactive fetch dependency. `active_api_key` is a plain
                    // prop-derived value, so `use_resource` won't see it change on
                    // its own — mirror it into a signal that the resource reads,
                    // and keep it in sync each render so editing the key triggers
                    // a refetch.
                    let mut fetch_dep = use_signal(|| active_api_key.clone());
                    if *fetch_dep.peek() != active_api_key {
                        fetch_dep.set(active_api_key.clone());
                    }

                    // Fetch the available Anthropic models.
                    let models = use_resource(move || {
                        let key = fetch_dep();
                        async move { crate::platform::list_models(&key).await }
                    });

                    let model_list: Vec<String> = match &*models.read() {
                        Some(Ok(list)) if !list.is_empty() => list.clone(),
                        _ => ANTHROPIC_FALLBACK_MODELS.iter().map(|s| s.to_string()).collect(),
                    };

                    let api_key_label = "Anthropic API Key";
                    let api_key_desc =
                        "Paste your Anthropic (Claude) API key here. Used for AI features. Stored locally on disk.";
                    let api_key_placeholder = "sk-ant-...";
                    let model_desc =
                        "Claude model used for AI features (Smart Edit and AI processing).";

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
                            h3 { class: "settings-section-title", "AI (Claude)" }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "{api_key_label}" }
                                    span { class: "settings-row-desc", "{api_key_desc}" }
                                }
                                input {
                                    class: "settings-input-apikey",
                                    r#type: "password",
                                    placeholder: "{api_key_placeholder}",
                                    value: "{active_api_key}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_apikey.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.anthropic_api_key = e.value().trim().to_string();
                                            on_changed.call(new_s);
                                        }
                                    },
                                }
                            }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "Model" }
                                    span { class: "settings-row-desc", "{model_desc}" }
                                }
                                select {
                                    class: "settings-select-model",
                                    value: "{active_model}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_model.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.anthropic_model = e.value();
                                            on_changed.call(new_s);
                                        }
                                    },
                                    for model_id in model_list.iter() {
                                        option {
                                            value: "{model_id}",
                                            selected: active_model == *model_id,
                                            "{model_id}"
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "AEM Connection" }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "AEM Host" }
                                    span { class: "settings-row-desc",
                                        "Base URL of the AEM author instance used for package upload."
                                    }
                                }
                                input {
                                    class: "settings-input-apikey",
                                    r#type: "text",
                                    placeholder: "http://localhost:4502",
                                    value: "{settings_for_aem_host.aem_host}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_aem_host.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.aem_host = e.value().trim().to_string();
                                            on_changed.call(new_s);
                                        }
                                    },
                                }
                            }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "AEM Username" }
                                    span { class: "settings-row-desc", "Username for AEM HTTP basic auth." }
                                }
                                input {
                                    class: "settings-input-apikey",
                                    r#type: "text",
                                    placeholder: "admin",
                                    value: "{settings_for_aem_user.aem_username}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_aem_user.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.aem_username = e.value().trim().to_string();
                                            on_changed.call(new_s);
                                        }
                                    },
                                }
                            }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "AEM Password" }
                                    span { class: "settings-row-desc",
                                        "Password for AEM HTTP basic auth. Stored locally on disk."
                                    }
                                }
                                input {
                                    class: "settings-input-apikey",
                                    r#type: "password",
                                    placeholder: "••••••••",
                                    value: "{settings_for_aem_pass.aem_password}",
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_aem_pass.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.aem_password = e.value();
                                            on_changed.call(new_s);
                                        }
                                    },
                                }
                            }
                        }
                    }
                };

                #[cfg(target_arch = "wasm32")]
                let section = rsx! {
                    p { class: "settings-no-options", "No configurable settings for the web version." }
                };

                section
            }
        }
    }
}
