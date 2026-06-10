//! Settings panel component.
//!
//! Renders a dropdown panel beneath the settings button in the header.

use dioxus::prelude::*;

use crate::settings::{AppSettings, LlmProvider};

/// Hardcoded fallback OpenAI model list, used when models cannot be fetched from the API.
const OPENAI_FALLBACK_MODELS: &[&str] = &[
    "gpt-4.1",
    "gpt-4.1-mini",
    "gpt-4.1-nano",
    "gpt-4o",
    "gpt-4o-mini",
    "o3",
    "o4-mini",
];

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
                {
                    let settings_for_aot = settings.clone();
                    let settings_for_port = settings.clone();
                    let settings_for_provider = settings.clone();
                    let settings_for_apikey = settings.clone();
                    let settings_for_model = settings.clone();

                    let provider = settings.provider;
                    let active_api_key = settings.active_api_key().to_string();
                    let active_model = settings.active_model().to_string();

                    // Reactive fetch dependency. `provider`/`active_api_key` are
                    // plain prop-derived values, so `use_resource` won't see them
                    // change on its own — mirror them into a signal that the
                    // resource reads, and keep it in sync each render so switching
                    // provider (or editing the key) triggers a refetch.
                    let mut fetch_dep = use_signal(|| (provider, active_api_key.clone()));
                    let current_dep = (provider, active_api_key.clone());
                    if *fetch_dep.peek() != current_dep {
                        fetch_dep.set(current_dep);
                    }

                    // Fetch available models for the current provider, tagging the
                    // result with the provider it was fetched for so a stale
                    // in-flight result for the previous provider is never shown.
                    let models = use_resource(move || {
                        let (provider, key) = fetch_dep();
                        async move { (provider, crate::platform::list_models(provider, &key).await) }
                    });

                    let fallback_models = match provider {
                        LlmProvider::OpenAi => OPENAI_FALLBACK_MODELS,
                        LlmProvider::Anthropic => ANTHROPIC_FALLBACK_MODELS,
                    };
                    let model_list: Vec<String> = match &*models.read() {
                        Some((p, Ok(list))) if *p == provider && !list.is_empty() => list.clone(),
                        _ => fallback_models.iter().map(|s| s.to_string()).collect(),
                    };

                    let (api_key_label, api_key_desc, api_key_placeholder, model_desc) =
                        match provider {
                            LlmProvider::OpenAi => (
                                "OpenAI API Key",
                                "Paste your OpenAI API key here. Used for Smart Edit. Stored locally on disk.",
                                "sk-...",
                                "OpenAI model to use for Smart Edit.",
                            ),
                            LlmProvider::Anthropic => (
                                "Anthropic API Key",
                                "Paste your Anthropic (Claude) API key here. Used for Smart Edit. Stored locally on disk.",
                                "sk-ant-...",
                                "Claude model to use for Smart Edit.",
                            ),
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
                            h3 { class: "settings-section-title", "AI Provider" }
                            div { class: "settings-row",
                                div { class: "settings-row-info",
                                    span { class: "settings-row-label", "Provider" }
                                    span { class: "settings-row-desc",
                                        "Which AI provider Smart Edit uses."
                                    }
                                }
                                select {
                                    class: "settings-select-model",
                                    value: match settings_for_provider.provider {
                                        LlmProvider::OpenAi => "openai",
                                        LlmProvider::Anthropic => "anthropic",
                                    },
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_provider.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.provider = match e.value().as_str() {
                                                "anthropic" => LlmProvider::Anthropic,
                                                _ => LlmProvider::OpenAi,
                                            };
                                            on_changed.call(new_s);
                                        }
                                    },
                                    option {
                                        value: "openai",
                                        selected: settings_for_provider.provider == LlmProvider::OpenAi,
                                        "ChatGPT (OpenAI)"
                                    }
                                    option {
                                        value: "anthropic",
                                        selected: settings_for_provider.provider == LlmProvider::Anthropic,
                                        "Claude (Anthropic)"
                                    }
                                }
                            }
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
                                            let v = e.value().trim().to_string();
                                            match new_s.provider {
                                                LlmProvider::OpenAi => new_s.openai_api_key = v,
                                                LlmProvider::Anthropic => new_s.anthropic_api_key = v,
                                            }
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
                                            match new_s.provider {
                                                LlmProvider::OpenAi => new_s.openai_model = e.value(),
                                                LlmProvider::Anthropic => new_s.anthropic_model = e.value(),
                                            }
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
