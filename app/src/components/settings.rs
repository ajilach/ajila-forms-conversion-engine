//! Settings page component.
//!
//! Renders a full-page settings view (toggled from the header gear button),
//! organized into tabs (reusing the reference-page tab styling).

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
pub fn SettingsPage(
    /// Called when the user closes the settings page.
    on_close: EventHandler<()>,
    /// Current settings.
    settings: AppSettings,
    /// Called when the user changes any setting.
    on_settings_changed: EventHandler<AppSettings>,
    /// Called when the user opens the reference-forms manager page.
    on_open_references: EventHandler<()>,
) -> Element {
    // Active tab: 0 General · 1 AI · 2 AEM · 3 References.
    let mut tab = use_signal(|| 0usize);

    // Per-field clones for the change handlers.
    let settings_for_aot = settings.clone();
    let settings_for_legacy_ui = settings.clone();
    let settings_for_port = settings.clone();
    let settings_for_apikey = settings.clone();
    let settings_for_model = settings.clone();
    let settings_for_aem_host = settings.clone();
    let settings_for_aem_user = settings.clone();
    let settings_for_aem_pass = settings.clone();
    let settings_for_keep = settings.clone();
    let settings_for_trigger = settings.clone();
    let settings_for_etext = settings.clone();
    let settings_for_einput = settings.clone();

    let active_api_key = settings.active_api_key().to_string();
    let active_model = settings.active_model().to_string();

    // Reactive fetch dependency. `active_api_key` is a plain prop-derived value,
    // so `use_resource` won't see it change on its own — mirror it into a signal
    // that the resource reads, and keep it in sync each render so editing the key
    // triggers a refetch. (Hooks must run unconditionally, before any tab logic.)
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

    let api_key_desc =
        "Paste your Anthropic (Claude) API key here. Used for AI features. Stored locally on disk.";
    let model_desc = "Claude model used for AI features (Smart Edit and AI processing).";

    rsx! {
        div { class: "settings-page",
            div { class: "settings-header",
                h2 { "Settings" }
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| on_close.call(()),
                    "✕ Close"
                }
            }

            div { class: "references-tabs",
                button {
                    class: if *tab.read() == 0 { "references-tab active" } else { "references-tab" },
                    onclick: move |_| tab.set(0),
                    "General"
                }
                button {
                    class: if *tab.read() == 1 { "references-tab active" } else { "references-tab" },
                    onclick: move |_| tab.set(1),
                    "AI (Claude)"
                }
                button {
                    class: if *tab.read() == 2 { "references-tab active" } else { "references-tab" },
                    onclick: move |_| tab.set(2),
                    "AEM Connection"
                }
                button {
                    class: if *tab.read() == 3 { "references-tab active" } else { "references-tab" },
                    onclick: move |_| tab.set(3),
                    "References"
                }
            }

            div { class: "settings-content",

                // ── General ──────────────────────────────────────────────────
                if *tab.read() == 0 {
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
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "Interface" }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Use legacy agent UI" }
                                span { class: "settings-row-desc",
                                    "Restore the previous stacked upload / progress / results layout (and normal, non-agent processing)."
                                }
                            }
                            label { class: "toggle-switch",
                                input {
                                    r#type: "checkbox",
                                    checked: settings_for_legacy_ui.legacy_agent_ui,
                                    onchange: {
                                        let on_changed = on_settings_changed;
                                        let s = settings_for_legacy_ui.clone();
                                        move |e: Event<FormData>| {
                                            let mut new_s = s.clone();
                                            new_s.legacy_agent_ui = e.checked();
                                            on_changed.call(new_s);
                                        }
                                    },
                                }
                                span { class: "toggle-slider" }
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
                    }
                }

                // ── AI (Claude) ──────────────────────────────────────────────
                else if *tab.read() == 1 {
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "AI (Claude)" }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Anthropic API Key" }
                                span { class: "settings-row-desc", "{api_key_desc}" }
                            }
                            input {
                                class: "settings-input-apikey",
                                r#type: "password",
                                placeholder: "sk-ant-...",
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
                        h3 { class: "settings-section-title", "Context management" }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Keep recent messages" }
                                span { class: "settings-row-desc",
                                    "Most recent messages kept verbatim each turn (rounded up to even). Higher = better grounding, more tokens."
                                }
                            }
                            input {
                                class: "settings-input-port",
                                r#type: "number",
                                min: "2",
                                step: "2",
                                value: "{settings_for_keep.evict_keep_recent_messages}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_keep.clone();
                                    move |e: Event<FormData>| {
                                        if let Ok(v) = e.value().parse::<usize>() {
                                            let mut new_s = s.clone();
                                            new_s.evict_keep_recent_messages = v;
                                            on_changed.call(new_s);
                                        }
                                    }
                                },
                            }
                        }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Eviction trigger (KB)" }
                                span { class: "settings-row-desc",
                                    "Start shrinking stale content once the conversation exceeds this size."
                                }
                            }
                            input {
                                class: "settings-input-port",
                                r#type: "number",
                                min: "0",
                                step: "10",
                                value: "{settings_for_trigger.evict_trigger_bytes / 1000}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_trigger.clone();
                                    move |e: Event<FormData>| {
                                        if let Ok(kb) = e.value().parse::<usize>() {
                                            let mut new_s = s.clone();
                                            new_s.evict_trigger_bytes = kb * 1000;
                                            on_changed.call(new_s);
                                        }
                                    }
                                },
                            }
                        }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Elide text over (chars)" }
                                span { class: "settings-row-desc",
                                    "Stale tool-result text longer than this is replaced with a re-fetchable stub."
                                }
                            }
                            input {
                                class: "settings-input-port",
                                r#type: "number",
                                min: "0",
                                step: "500",
                                value: "{settings_for_etext.evict_text_over_chars}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_etext.clone();
                                    move |e: Event<FormData>| {
                                        if let Ok(v) = e.value().parse::<usize>() {
                                            let mut new_s = s.clone();
                                            new_s.evict_text_over_chars = v;
                                            on_changed.call(new_s);
                                        }
                                    }
                                },
                            }
                        }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Elide tool input over (chars)" }
                                span { class: "settings-row-desc",
                                    "Stale tool-call inputs (e.g. whole-tree writes) longer than this are stubbed."
                                }
                            }
                            input {
                                class: "settings-input-port",
                                r#type: "number",
                                min: "0",
                                step: "500",
                                value: "{settings_for_einput.evict_input_over_chars}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_einput.clone();
                                    move |e: Event<FormData>| {
                                        if let Ok(v) = e.value().parse::<usize>() {
                                            let mut new_s = s.clone();
                                            new_s.evict_input_over_chars = v;
                                            on_changed.call(new_s);
                                        }
                                    }
                                },
                            }
                        }
                    }
                }

                // ── AEM Connection ───────────────────────────────────────────
                else if *tab.read() == 2 {
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

                // ── References ───────────────────────────────────────────────
                else {
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "Reference forms" }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Manage reference forms" }
                                span { class: "settings-row-desc",
                                    "Add, import, export, and delete the reference forms used for matching."
                                }
                            }
                            button {
                                class: "btn btn-primary btn-sm",
                                onclick: move |_| {
                                    on_close.call(());
                                    on_open_references.call(());
                                },
                                "Open…"
                            }
                        }
                    }
                }
            }
        }
    }
}
