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
    /// Active conversion profile — reference forms are managed per profile.
    profile: Option<String>,
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
                    let settings_for_refs = settings.clone();

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
                        ReferenceFormsSection {
                            profile: profile.clone(),
                            settings: settings_for_refs.clone(),
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

/// Prompt sent to the LLM to describe an input form when adding a reference.
/// The model is given tools to analyse the inputs first (see
/// [`crate::ai_tools::build_describe_tools`]).
#[cfg(not(target_arch = "wasm32"))]
const DESCRIBE_PROMPT: &str = "\
You are cataloguing a reference form so it can later be matched against similar forms. \
First ANALYSE THE INPUTS using the tools: inspect the source form via `list_states`, \
`get_plain_state_image`, `get_flattened_structure_for_state`, and `get_xfa` (the XFA is the \
authoritative field/label/option source), and inspect the resulting AEM package via \
`list_package_files` and `read_package_file`. Call as many as you need before answering.\n\n\
Then write a detailed description covering: the overall purpose; each section and its heading; \
the fields in order with their literal labels and types (text, date, number, select, radio, \
checkbox); logical groupings (address blocks, signature blocks, account-holder / client-details \
sections, type selectors like 'Tipo'/'Type'); and any dynamic behaviour (repeatable sections, \
conditional show/hide). Use precise, literal labels. Respond with prose only — no preamble, no \
markdown.";

/// Per-profile reference-form management: add (with LLM-generated description),
/// list/delete, import a dataset, and export the profile's references.
#[cfg(not(target_arch = "wasm32"))]
#[component]
fn ReferenceFormsSection(profile: Option<String>, settings: AppSettings) -> Element {
    let mut refresh = use_signal(|| 0u32);
    let mut pdf = use_signal(|| None::<(String, Vec<u8>)>);
    let mut pkg = use_signal(|| None::<(String, Vec<u8>)>);
    let mut status = use_signal(|| None::<(bool, String)>);
    let mut busy = use_signal(|| false);

    // No profile → nothing to manage.
    let Some(profile_name) = profile.clone() else {
        return rsx! {
            div { class: "settings-section",
                h3 { class: "settings-section-title", "Reference forms" }
                p { class: "settings-row-desc",
                    "Select a profile to manage its reference forms."
                }
            }
        };
    };

    // Recompute the list whenever `refresh` changes.
    let _ = refresh();
    let refs = crate::references::list_references(&profile_name);

    let api_key = settings.active_api_key().to_string();
    let model = settings.active_model().to_string();

    // Per-closure owned clones (Option<String> / String are not Copy).
    let profile_add = profile_name.clone();
    let profile_import = profile_name.clone();
    let profile_export = profile_name.clone();

    rsx! {
        div { class: "settings-section",
            h3 { class: "settings-section-title", "Reference forms" }

            if let Some((ok, msg)) = status.read().clone() {
                p {
                    class: "settings-row-desc",
                    style: if ok { "" } else { "color: var(--danger, #c0392b);" },
                    "{msg}"
                }
            }

            // ── Add a reference form (original PDF + final AEM package) ──────
            div { class: "settings-row",
                div { class: "settings-row-info",
                    span { class: "settings-row-label", "Original PDF" }
                    span { class: "settings-row-desc",
                        {pdf.read().as_ref().map(|(n, _)| n.clone()).unwrap_or_else(|| "The input form (XFA PDF).".to_string())}
                    }
                }
                input {
                    r#type: "file",
                    accept: ".pdf",
                    onchange: move |evt: Event<FormData>| async move {
                        if let Some(f) = evt.files().into_iter().next()
                            && let Ok(b) = f.read_bytes().await
                        {
                            pdf.set(Some((f.name(), b.to_vec())));
                        }
                    },
                }
            }
            div { class: "settings-row",
                div { class: "settings-row-info",
                    span { class: "settings-row-label", "Final AEM package" }
                    span { class: "settings-row-desc",
                        {pkg.read().as_ref().map(|(n, _)| n.clone()).unwrap_or_else(|| "The resulting FileVault ZIP.".to_string())}
                    }
                }
                input {
                    r#type: "file",
                    accept: ".zip",
                    onchange: move |evt: Event<FormData>| async move {
                        if let Some(f) = evt.files().into_iter().next()
                            && let Ok(b) = f.read_bytes().await
                        {
                            pkg.set(Some((f.name(), b.to_vec())));
                        }
                    },
                }
            }
            div { class: "settings-row",
                button {
                    class: "btn btn-primary btn-sm",
                    disabled: *busy.read(),
                    onclick: move |_| {
                        let profile = profile_add.clone();
                        let api_key = api_key.clone();
                        let model = model.clone();
                        let pdf_data = pdf.read().clone();
                        let pkg_data = pkg.read().clone();
                        spawn(async move {
                            let (Some((pdf_name, pdf_bytes)), Some((_, pkg_bytes))) = (pdf_data, pkg_data) else {
                                status.set(Some((false, "Pick both an original PDF and an AEM package first.".into())));
                                return;
                            };
                            busy.set(true);
                            status.set(Some((true, "Unpacking the AEM package…".into())));

                            // Unzip the package so the model can read it via tools.
                            let pkg_for_unzip = pkg_bytes.clone();
                            let files = match tokio::task::spawn_blocking(move || {
                                crate::references::unzip_package(&pkg_for_unzip)
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                            {
                                Ok(f) => f,
                                Err(e) => {
                                    busy.set(false);
                                    status.set(Some((false, format!("Unzip failed: {e}"))));
                                    return;
                                }
                            };

                            // Build the analysis tools (form states/XFA + package files).
                            status.set(Some((true, "Analyzing the inputs…".into())));
                            let tools = crate::ai_tools::build_describe_tools(
                                &pdf_name,
                                pdf_bytes.clone(),
                                files.clone(),
                            )
                            .await;

                            let mut history: Vec<serde_json::Value> = Vec::new();
                            let description = match crate::platform::anthropic_agentic_turn(
                                &mut history,
                                DESCRIBE_PROMPT,
                                &api_key,
                                &model,
                                4000,
                                &tools.tools(),
                                |name, input| tools.execute(name, input),
                            )
                            .await
                            {
                                Ok(d) => d,
                                Err(e) => {
                                    busy.set(false);
                                    status.set(Some((false, format!("Describe failed: {e}"))));
                                    return;
                                }
                            };

                            status.set(Some((true, "Saving the reference…".into())));
                            let label = pdf_name.trim_end_matches(".pdf").to_string();
                            let result = tokio::task::spawn_blocking(move || {
                                let emb = crate::references::embed_description(&description)?;
                                let ref_id = crate::references::compute_ref_id(&pdf_bytes);
                                let pages = crate::references::pdf_state_count(&pdf_bytes);
                                crate::references::add_reference(
                                    &profile, &ref_id, &label, &description, &emb,
                                    &[(pages, pdf_bytes)], &files,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));

                            busy.set(false);
                            match result {
                                Ok(()) => {
                                    status.set(Some((true, "Reference form added.".into())));
                                    pdf.set(None);
                                    pkg.set(None);
                                    let n = refresh();
                                    refresh.set(n + 1);
                                }
                                Err(e) => status.set(Some((false, format!("Add failed: {e}")))),
                            }
                        });
                    },
                    if *busy.read() { "Working…" } else { "Add reference form" }
                }
            }

            // ── Existing references ──────────────────────────────────────────
            if refs.is_empty() {
                p { class: "settings-row-desc", "No reference forms for this profile yet." }
            } else {
                ul { class: "session-list",
                    for r in refs.iter() {
                        li {
                            key: "{r.ref_id}",
                            class: "session-item",
                            div { class: "session-meta",
                                span { class: "session-label", "{r.label}" }
                                span { class: "session-submeta",
                                    "{r.pdf_count} pdf(s) · {r.files.len()} file(s)"
                                }
                            }
                            button {
                                class: "btn btn-secondary btn-sm",
                                onclick: {
                                    let ref_id = r.ref_id.clone();
                                    move |_| {
                                        crate::references::delete_reference(&ref_id);
                                        let n = refresh();
                                        refresh.set(n + 1);
                                    }
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }

            // ── Import / export dataset ──────────────────────────────────────
            div { class: "settings-row",
                div { class: "settings-row-info",
                    span { class: "settings-row-label", "Import dataset" }
                    span { class: "settings-row-desc",
                        "Load a reference dataset (.db) into this profile."
                    }
                }
                input {
                    r#type: "file",
                    accept: ".db,.sqlite",
                    onchange: move |evt: Event<FormData>| {
                        let profile = profile_import.clone();
                        async move {
                            if let Some(f) = evt.files().into_iter().next()
                                && let Ok(b) = f.read_bytes().await
                            {
                                let tmp = std::env::temp_dir().join("blueprint-ref-import.db");
                                if std::fs::write(&tmp, &b).is_ok() {
                                    match crate::references::import_reference_db(
                                        &tmp.to_string_lossy(),
                                        &profile,
                                    ) {
                                        Ok(n) => {
                                            status.set(Some((true, format!("Imported {n} reference(s)."))));
                                            let c = refresh();
                                            refresh.set(c + 1);
                                        }
                                        Err(e) => status.set(Some((false, format!("Import failed: {e}")))),
                                    }
                                    let _ = std::fs::remove_file(&tmp);
                                }
                            }
                        }
                    },
                }
            }
            div { class: "settings-row",
                button {
                    class: "btn btn-secondary btn-sm",
                    disabled: refs.is_empty(),
                    onclick: move |_| {
                        let profile = profile_export.clone();
                        spawn(async move {
                            let Some(home) = dirs::home_dir() else {
                                status.set(Some((false, "Could not find home directory.".into())));
                                return;
                            };
                            let out = home.join("Downloads").join(format!("references-{profile}.db"));
                            let out_str = out.to_string_lossy().to_string();
                            let profile2 = profile.clone();
                            let res = tokio::task::spawn_blocking(move || {
                                crate::references::export_references(&out_str, Some(&profile2))
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                            match res {
                                Ok(n) => {
                                    status.set(Some((true, format!("Exported {n} reference(s) to {}", out.display()))));
                                    crate::platform::reveal_in_file_explorer(&out);
                                }
                                Err(e) => status.set(Some((false, format!("Export failed: {e}")))),
                            }
                        });
                    },
                    "Export references"
                }
            }
        }
    }
}
