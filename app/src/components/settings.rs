//! Settings page component.
//!
//! Renders a full-page settings view (toggled from the header gear button),
//! organized into tabs (reusing the reference-page tab styling).

use std::collections::HashMap;

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
    let settings_for_agent_instr = settings.clone();
    let settings_for_se_instr = settings.clone();
    let settings_for_aem_instr = settings.clone();
    let settings_for_local_mode = settings.clone();
    let settings_for_local_model = settings.clone();

    // Per-model download state, keyed by model name, so each model downloads
    // independently and concurrently with its own progress and error.
    let mut download_progress: Signal<HashMap<String, f32>> = use_signal(HashMap::new);
    let mut download_error: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    // Bumped after a download/delete to force the downloaded-models list to re-read.
    let mut models_refresh: Signal<u32> = use_signal(|| 0u32);

    // Whether the Blueprint MCP server is registered in Claude Desktop, and the
    // last install error (shown below the row). Checked once on mount; flipped
    // to `true` after a successful install.
    let mut mcp_installed = use_signal(crate::mcp_install::is_installed);
    let mut mcp_install_error: Signal<Option<String>> = use_signal(|| None);

    // Keep the selected local model valid. If it points at a model that isn't
    // downloaded (e.g. a stale value persisted from before, or a since-deleted
    // model), fall back to the first downloaded model — otherwise the <select>
    // displays the first option while the saved value still points at the missing
    // model, and re-picking the shown option fires no change event, so inference
    // would keep trying to load the wrong/absent model. Mirror `local_model` into
    // a signal so the effect re-runs with the current value (not a mount snapshot)
    // whenever the selection or the downloaded set changes.
    let mut local_model_dep = use_signal(|| settings.local_model.clone());
    if *local_model_dep.peek() != settings.local_model {
        local_model_dep.set(settings.local_model.clone());
    }
    {
        let settings_for_reconcile = settings.clone();
        use_effect(move || {
            let _ = models_refresh.read();
            let current = local_model_dep();
            let downloaded = crate::local_inference::downloaded_models();
            if !downloaded.iter().any(|m| *m == current) {
                // Current selection isn't downloaded (stale, removed, or empty):
                // fall back to the first downloaded model, or empty if none. Also
                // auto-selects a freshly downloaded model when none was selected.
                let replacement = downloaded.first().cloned().unwrap_or_default();
                if replacement != current {
                    let mut new_s = settings_for_reconcile.clone();
                    new_s.local_model = replacement;
                    on_settings_changed.call(new_s);
                }
            }
        });
    }

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
                button {
                    class: if *tab.read() == 4 { "references-tab active" } else { "references-tab" },
                    onclick: move |_| tab.set(4),
                    "Local Model"
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
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "Claude Desktop" }
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Blueprint MCP server" }
                                span { class: "settings-row-desc",
                                    "Register Blueprint's conversion tools with Claude Desktop so you can drive conversions from Claude. Restart Claude Desktop after installing."
                                }
                            }
                            if *mcp_installed.read() {
                                span { class: "local-model-downloaded", "Installed ✓" }
                            } else {
                                button {
                                    class: "btn btn-primary btn-sm",
                                    onclick: move |_| {
                                        match crate::mcp_install::install() {
                                            Ok(()) => {
                                                mcp_install_error.set(None);
                                                mcp_installed.set(true);
                                            }
                                            Err(e) => mcp_install_error.set(Some(e)),
                                        }
                                    },
                                    "Install"
                                }
                            }
                        }
                        if let Some(err) = mcp_install_error.read().as_ref() {
                            div { class: "local-model-error", "{err}" }
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
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "Custom instructions" }
                        div { class: "settings-row settings-row-stack",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Agent (AI processing)" }
                                span { class: "settings-row-desc",
                                    "Extra instructions appended to the autonomous conversion agent's system prompt. Applied to AI processing and feedback re-runs."
                                }
                            }
                            textarea {
                                class: "settings-textarea",
                                rows: "4",
                                placeholder: "e.g. Always keep signature blocks on the last page.",
                                value: "{settings_for_agent_instr.agent_instructions}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_agent_instr.clone();
                                    move |e: Event<FormData>| {
                                        let mut new_s = s.clone();
                                        new_s.agent_instructions = e.value();
                                        on_changed.call(new_s);
                                    }
                                },
                            }
                        }
                        div { class: "settings-row settings-row-stack",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Smart Edit (structure)" }
                                span { class: "settings-row-desc",
                                    "Extra instructions appended to the structured-tree Smart Edit prompt."
                                }
                            }
                            textarea {
                                class: "settings-textarea",
                                rows: "4",
                                placeholder: "e.g. Prefer Radio over Select for choices with up to 5 options.",
                                value: "{settings_for_se_instr.smart_edit_instructions}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_se_instr.clone();
                                    move |e: Event<FormData>| {
                                        let mut new_s = s.clone();
                                        new_s.smart_edit_instructions = e.value();
                                        on_changed.call(new_s);
                                    }
                                },
                            }
                        }
                        div { class: "settings-row settings-row-stack",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Smart Edit (AEM)" }
                                span { class: "settings-row-desc",
                                    "Extra instructions appended to the AEM-tree Smart Edit prompt."
                                }
                            }
                            textarea {
                                class: "settings-textarea",
                                rows: "4",
                                placeholder: "e.g. Keep page panels intact and never merge distinct signers.",
                                value: "{settings_for_aem_instr.aem_smart_edit_instructions}",
                                onchange: {
                                    let on_changed = on_settings_changed;
                                    let s = settings_for_aem_instr.clone();
                                    move |e: Event<FormData>| {
                                        let mut new_s = s.clone();
                                        new_s.aem_smart_edit_instructions = e.value();
                                        on_changed.call(new_s);
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
                else if *tab.read() == 3 {
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

                // ── Local Model ──────────────────────────────────────────────
                else {
                    div { class: "settings-section",
                        h3 { class: "settings-section-title", "Local Model Inference" }

                        // Enable local mode toggle
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Local Mode" }
                                span { class: "settings-row-desc",
                                    "Use a locally downloaded model instead of the Claude API."
                                }
                            }
                            label { class: "toggle-switch",
                                input {
                                    r#type: "checkbox",
                                    checked: settings.local_mode,
                                    onchange: move |e: Event<FormData>| {
                                        let mut new_s = settings_for_local_mode.clone();
                                        new_s.local_mode = e.checked();
                                        on_settings_changed.call(new_s);
                                    },
                                }
                                span { class: "toggle-slider" }
                            }
                        }

                        // Model selector dropdown
                        div { class: "settings-row",
                            div { class: "settings-row-info",
                                span { class: "settings-row-label", "Active model" }
                                span { class: "settings-row-desc",
                                    "Select from your downloaded models."
                                }
                            }
                            {
                                let _ = models_refresh.read();
                                let downloaded = crate::local_inference::downloaded_models();
                                rsx! {
                                    select {
                                        class: "settings-select-model",
                                        disabled: downloaded.is_empty(),
                                        value: settings.local_model.clone(),
                                        onchange: move |e| {
                                            let mut new_s = settings_for_local_model.clone();
                                            new_s.local_model = e.value();
                                            on_settings_changed.call(new_s);
                                        },
                                        if downloaded.is_empty() {
                                            option { value: "", "— no models downloaded —" }
                                        }
                                        for m in &downloaded {
                                            option { value: "{m}", selected: *m == settings.local_model, "{m}" }
                                        }
                                    }
                                }
                            }
                        }

                        // Available models list
                        h3 { class: "settings-section-title", style: "margin-top: 24px;", "Available Models" }
                        for spec in crate::local_inference::AVAILABLE_MODELS {
                            {
                                let _ = models_refresh.read();
                                let name = spec.name.to_string();
                                let dl_name = name.clone();
                                let del_name = name.clone();
                                // This model's own download state — independent of the others.
                                let progress = download_progress.read().get(&name).copied();
                                let downloading = progress.is_some();
                                let is_dl = crate::local_inference::is_downloaded(&name) && !downloading;
                                let err = download_error.read().get(&name).cloned();
                                rsx! {
                                    div { class: "settings-row",
                                        div { class: "settings-row-info",
                                            span { class: "settings-row-label", "{spec.name}" }
                                            span { class: "settings-row-desc", "{spec.hf_repo}" }
                                        }
                                        if downloading {
                                            {
                                                let pct = (progress.unwrap_or(0.0) * 100.0) as u32;
                                                rsx! {
                                                    div { class: "local-model-progress-wrap",
                                                        div { class: "download-progress-bar",
                                                            div {
                                                                class: "download-progress-fill",
                                                                style: "width: {pct}%",
                                                            }
                                                        }
                                                        span { class: "local-model-progress-label", "{pct}%" }
                                                    }
                                                }
                                            }
                                        } else if is_dl {
                                            div { class: "local-model-actions",
                                                span { class: "local-model-downloaded", "Downloaded ✓" }
                                                button {
                                                    class: "btn btn-secondary btn-sm",
                                                    onclick: move |_| {
                                                        let name = del_name.clone();
                                                        download_error.with_mut(|m| { m.remove(&name); });
                                                        dioxus::prelude::spawn(async move {
                                                            if let Err(e) = crate::local_inference::delete_model(&name).await {
                                                                download_error.with_mut(|m| { m.insert(name.clone(), e); });
                                                            }
                                                            models_refresh.with_mut(|n| *n += 1);
                                                        });
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        } else {
                                            button {
                                                class: "btn btn-primary btn-sm",
                                                onclick: move |_| {
                                                    let name = dl_name.clone();
                                                    download_error.with_mut(|m| { m.remove(&name); });
                                                    download_progress.with_mut(|m| { m.insert(name.clone(), 0.0); });
                                                    dioxus::prelude::spawn(async move {
                                                        let progress_name = name.clone();
                                                        let result = crate::local_inference::download_model(
                                                            &name,
                                                            move |p| {
                                                                download_progress.with_mut(|m| { m.insert(progress_name.clone(), p); });
                                                            },
                                                        ).await;
                                                        download_progress.with_mut(|m| { m.remove(&name); });
                                                        if let Err(e) = result {
                                                            download_error.with_mut(|m| { m.insert(name.clone(), e); });
                                                        }
                                                        models_refresh.with_mut(|n| *n += 1);
                                                    });
                                                },
                                                "Download"
                                            }
                                        }
                                    }
                                    if let Some(err) = err {
                                        div { class: "local-model-error", "{err}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
