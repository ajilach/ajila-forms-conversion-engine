//! Settings page component.
//!
//! Renders a full-page settings view (toggled from the header gear button),
//! organized into tabs. Every control funnels through one `update` callback:
//! it copies the current settings, applies the one field the row owns, and
//! hands the whole struct back to the app, which persists it.

use dioxus::prelude::*;

use super::page::FullPage;
use crate::settings::AppSettings;

/// Hardcoded fallback Anthropic model list, used when models cannot be fetched from the API.
const ANTHROPIC_FALLBACK_MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
];

/// The settings tabs, in the order they are shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsTab {
    #[default]
    General,
    Ai,
    Aem,
    References,
}

impl SettingsTab {
    const ALL: &'static [Self] = &[Self::General, Self::Ai, Self::Aem, Self::References];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Ai => "AI (Claude)",
            Self::Aem => "AEM Connection",
            Self::References => "References",
        }
    }
}

/// A change to one settings field, applied to a copy of the current settings.
type Edit = Box<dyn FnOnce(&mut AppSettings)>;

#[component]
pub fn SettingsPage(
    /// Called when the user closes the settings page.
    on_close: EventHandler<()>,
    /// Current settings.
    settings: ReadSignal<AppSettings>,
    /// Called when the user changes any setting.
    on_settings_changed: EventHandler<AppSettings>,
    /// Called when the user opens the reference-forms manager page.
    on_open_references: EventHandler<()>,
) -> Element {
    let mut tab = use_signal(SettingsTab::default);

    // Whether the Blueprint MCP server is registered in Claude Desktop, and the
    // last install error (shown below the row). Checked once on mount; flipped
    // to `true` after a successful install.
    let mut mcp_installed = use_signal(crate::mcp_install::is_installed);
    let mut mcp_install_error: Signal<Option<String>> = use_signal(|| None);

    // The single write path: every row below calls this with the one field it
    // owns, so there is no per-row copy of the settings struct.
    let update = use_callback(move |edit: Edit| {
        let mut next = settings();
        edit(&mut next);
        on_settings_changed.call(next);
    });

    // Narrow the model fetch to the key alone — a memo only fires when the key
    // itself changes, so editing an unrelated setting does not refetch.
    let api_key = use_memo(move || settings.read().anthropic_api_key.clone());
    let models = use_resource(move || {
        let key = api_key();
        async move { crate::platform::anthropic_list_models(&key).await }
    });
    let model_list = use_memo(move || match &*models.read() {
        Some(Ok(list)) if !list.is_empty() => list.clone(),
        _ => ANTHROPIC_FALLBACK_MODELS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    });

    let s = settings.read();

    rsx! {
        FullPage { title: "Settings", on_close,

            div { class: "tabs",
                for t in SettingsTab::ALL {
                    button {
                        class: if tab() == *t { "tab active" } else { "tab" },
                        onclick: move |_| tab.set(*t),
                        "{t.label()}"
                    }
                }
            }

            div { class: "page-content",
                match tab() {
                    SettingsTab::General => rsx! {
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "Window" }
                            ToggleRow {
                                label: "Always on top",
                                desc: "Keep the window above all other applications.",
                                checked: s.always_on_top,
                                on_toggle: move |v: bool| update.call(Box::new(move |s| s.always_on_top = v)),
                            }
                        }
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "Claude Desktop" }
                            div { class: "settings-row",
                                RowInfo {
                                    label: "Blueprint MCP server",
                                    desc: "Register Blueprint's conversion tools with Claude Desktop so you can drive conversions from Claude. Restart Claude Desktop after installing.",
                                }
                                if mcp_installed() {
                                    span { class: "mcp-installed", "Installed ✓" }
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
                                div { class: "mcp-error", "{err}" }
                            }
                        }
                    },

                    SettingsTab::Ai => rsx! {
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "AI (Claude)" }
                            TextRow {
                                label: "Anthropic API Key",
                                desc: "Paste your Anthropic (Claude) API key here. Used for AI features. Stored locally on disk.",
                                value: s.anthropic_api_key.clone(),
                                placeholder: "sk-ant-...",
                                secret: true,
                                on_change: move |v: String| {
                                    update.call(Box::new(move |s| s.anthropic_api_key = v.trim().to_string()))
                                },
                            }
                            SelectRow {
                                label: "Model",
                                desc: "Claude model used for AI features (the conversion agent and reference descriptions).",
                                value: s.anthropic_model.clone(),
                                options: model_list(),
                                on_change: move |v: String| update.call(Box::new(move |s| s.anthropic_model = v)),
                            }
                            NumberRow {
                                label: "Max review rounds",
                                desc: "How many Reviewer → Author fix rounds before finalizing with whatever is built. Higher = more self-correction, more tokens.",
                                value: s.max_review_rounds,
                                min: 1,
                                step: 1,
                                on_change: move |v: usize| {
                                    update.call(Box::new(move |s| s.max_review_rounds = v.max(1)))
                                },
                            }
                        }
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "Context management" }
                            NumberRow {
                                label: "Keep recent messages",
                                desc: "Most recent messages kept verbatim each turn (rounded up to even). Higher = better grounding, more tokens.",
                                value: s.evict_keep_recent_messages,
                                min: 2,
                                step: 2,
                                on_change: move |v: usize| {
                                    update.call(Box::new(move |s| s.evict_keep_recent_messages = v))
                                },
                            }
                            NumberRow {
                                label: "Eviction trigger (KB)",
                                desc: "Start shrinking stale content once the conversation exceeds this size.",
                                value: s.evict_trigger_bytes / 1000,
                                min: 0,
                                step: 10,
                                on_change: move |kb: usize| {
                                    update.call(Box::new(move |s| s.evict_trigger_bytes = kb * 1000))
                                },
                            }
                            NumberRow {
                                label: "Elide text over (chars)",
                                desc: "Stale tool-result text longer than this is replaced with a re-fetchable stub.",
                                value: s.evict_text_over_chars,
                                min: 0,
                                step: 500,
                                on_change: move |v: usize| {
                                    update.call(Box::new(move |s| s.evict_text_over_chars = v))
                                },
                            }
                            NumberRow {
                                label: "Elide tool input over (chars)",
                                desc: "Stale tool-call inputs (e.g. whole-tree writes) longer than this are stubbed.",
                                value: s.evict_input_over_chars,
                                min: 0,
                                step: 500,
                                on_change: move |v: usize| {
                                    update.call(Box::new(move |s| s.evict_input_over_chars = v))
                                },
                            }
                        }
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "Custom instructions" }
                            div { class: "settings-row settings-row-stack",
                                RowInfo {
                                    label: "Agent (AI processing)",
                                    desc: "Extra instructions appended to the autonomous conversion agent's system prompt. Applied to AI processing and feedback re-runs.",
                                }
                                textarea {
                                    class: "settings-textarea",
                                    rows: "4",
                                    placeholder: "e.g. Always keep signature blocks on the last page.",
                                    value: "{s.agent_instructions}",
                                    onchange: move |e: Event<FormData>| {
                                        let v = e.value();
                                        update.call(Box::new(move |s| s.agent_instructions = v));
                                    },
                                }
                            }
                        }
                    },

                    SettingsTab::Aem => rsx! {
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "AEM Connection" }
                            TextRow {
                                label: "AEM Host",
                                desc: "Base URL of the AEM author instance used for package upload.",
                                value: s.aem_host.clone(),
                                placeholder: "http://localhost:4502",
                                secret: false,
                                on_change: move |v: String| {
                                    update.call(Box::new(move |s| s.aem_host = v.trim().to_string()))
                                },
                            }
                            TextRow {
                                label: "AEM Username",
                                desc: "Username for AEM HTTP basic auth.",
                                value: s.aem_username.clone(),
                                placeholder: "admin",
                                secret: false,
                                on_change: move |v: String| {
                                    update.call(Box::new(move |s| s.aem_username = v.trim().to_string()))
                                },
                            }
                            TextRow {
                                label: "AEM Password",
                                desc: "Password for AEM HTTP basic auth. Stored locally on disk.",
                                value: s.aem_password.clone(),
                                placeholder: "••••••••",
                                secret: true,
                                on_change: move |v: String| update.call(Box::new(move |s| s.aem_password = v)),
                            }
                        }
                    },

                    SettingsTab::References => rsx! {
                        div { class: "settings-section",
                            h3 { class: "settings-section-title", "Reference forms" }
                            div { class: "settings-row",
                                RowInfo {
                                    label: "Manage reference forms",
                                    desc: "Add, import, export, and delete the reference forms used for matching.",
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
                    },
                }
            }
        }
    }
}

/// The label + description column every settings row starts with.
#[component]
fn RowInfo(label: &'static str, desc: &'static str) -> Element {
    rsx! {
        div { class: "settings-row-info",
            span { class: "settings-row-label", "{label}" }
            span { class: "settings-row-desc", "{desc}" }
        }
    }
}

/// A settings row carrying an on/off switch.
#[component]
fn ToggleRow(
    label: &'static str,
    desc: &'static str,
    checked: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "settings-row",
            RowInfo { label, desc }
            label { class: "toggle-switch",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |e: Event<FormData>| on_toggle.call(e.checked()),
                }
                span { class: "toggle-slider" }
            }
        }
    }
}

/// A settings row carrying a single-line text field. `secret` masks the input.
#[component]
fn TextRow(
    label: &'static str,
    desc: &'static str,
    value: String,
    placeholder: &'static str,
    secret: bool,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "settings-row",
            RowInfo { label, desc }
            input {
                class: "settings-input-text",
                r#type: if secret { "password" } else { "text" },
                placeholder,
                value,
                onchange: move |e: Event<FormData>| on_change.call(e.value()),
            }
        }
    }
}

/// A settings row carrying a whole-number field. Unparseable input is ignored,
/// leaving the stored value untouched.
#[component]
fn NumberRow(
    label: &'static str,
    desc: &'static str,
    value: usize,
    min: usize,
    step: usize,
    on_change: EventHandler<usize>,
) -> Element {
    rsx! {
        div { class: "settings-row",
            RowInfo { label, desc }
            input {
                class: "settings-input-number",
                r#type: "number",
                min: "{min}",
                step: "{step}",
                value: "{value}",
                onchange: move |e: Event<FormData>| {
                    if let Ok(v) = e.value().parse::<usize>() {
                        on_change.call(v);
                    }
                },
            }
        }
    }
}

/// A settings row carrying a dropdown over `options`.
#[component]
fn SelectRow(
    label: &'static str,
    desc: &'static str,
    value: String,
    options: Vec<String>,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "settings-row",
            RowInfo { label, desc }
            select {
                class: "settings-select",
                value: value.clone(),
                onchange: move |e: Event<FormData>| on_change.call(e.value()),
                for option_value in options.iter() {
                    option {
                        value: "{option_value}",
                        selected: value == *option_value,
                        "{option_value}"
                    }
                }
            }
        }
    }
}
