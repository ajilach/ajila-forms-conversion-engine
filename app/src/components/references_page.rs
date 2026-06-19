//! Full-page reference-form manager.
//!
//! Add a reference form (with an LLM-generated description), browse/search and
//! delete references, and import/export reference datasets. A profile dropdown
//! scopes which profile new references are added to (and imported/exported).
//! Opened from the Settings panel. Desktop-only; a short stub renders on web.

use dioxus::prelude::*;

use crate::settings::AppSettings;

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
conditional show/hide). Use precise, literal labels.\n\n\
Output ONLY the description text itself, as prose with no markdown. Do NOT include any preamble, \
sign-off, or meta-commentary about your analysis, the tools, or the sources. Never write sentences \
like \"I now have a complete picture...\", \"Based on the XFA and AEM package...\", or \"Here is the \
catalogue description.\". Begin immediately with the form's purpose (e.g. \"This form ...\").";

/// Full-page reference-form manager.
#[cfg(not(target_arch = "wasm32"))]
#[component]
pub fn ReferencesPage(
    /// Active conversion profile — used as the default profile selection.
    profile: Option<String>,
    /// Current settings (for the Anthropic API key and model).
    settings: AppSettings,
    /// Called when the user closes the page.
    on_close: EventHandler<()>,
) -> Element {
    let mut refresh = use_signal(|| 0u32);
    let mut pdfs = use_signal(Vec::<(String, Vec<u8>)>::new);
    let mut pkg = use_signal(|| None::<(String, Vec<u8>)>);
    let mut import_file = use_signal(|| None::<(String, Vec<u8>)>);
    let mut status = use_signal(|| None::<(bool, String)>);
    let mut busy = use_signal(|| false);
    let mut search = use_signal(String::new);
    // Reference-documentation upload (filename, content) + its own search.
    let mut doc_file = use_signal(|| None::<(String, String)>);
    let mut doc_search = use_signal(String::new);
    // Ids whose description/content is expanded (collapsed to one line by default).
    let mut expanded = use_signal(std::collections::HashSet::<String>::new);
    let mut doc_expanded = use_signal(std::collections::HashSet::<String>::new);

    let profiles = blueprint::list_profiles();

    // Profile that new references are added to / imported into / exported from.
    // Defaults to the active profile, falling back to the first configured one.
    let mut selected_profile =
        use_signal(|| profile.clone().or_else(|| profiles.first().cloned()));

    // No profiles configured → nothing to scope references to.
    if profiles.is_empty() {
        return rsx! {
            div { class: "references-page",
                div { class: "references-header",
                    div { h2 { "Reference Forms" } }
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| on_close.call(()),
                        "✕ Close"
                    }
                }
                div { class: "references-content",
                    p { class: "references-empty", "No profiles are configured." }
                }
            }
        };
    }

    // Recompute whenever `refresh` changes.
    let _ = refresh();

    // The reference store is per-profile; union every profile's references into
    // one list, then filter by the search query (label or profile).
    let mut all_refs = Vec::new();
    for p in &profiles {
        all_refs.extend(crate::references::list_references(p));
    }
    let total_refs = all_refs.len();
    let query = search.read().trim().to_lowercase();
    let filtered: Vec<_> = all_refs
        .iter()
        .filter(|r| {
            query.is_empty()
                || r.label.to_lowercase().contains(&query)
                || r.profile.to_lowercase().contains(&query)
        })
        .collect();

    // Documentation: union across profiles, filtered by its own search.
    let mut all_docs = Vec::new();
    for p in &profiles {
        all_docs.extend(crate::references::list_docs(p));
    }
    let total_docs = all_docs.len();
    let doc_query = doc_search.read().trim().to_lowercase();
    let filtered_docs: Vec<_> = all_docs
        .iter()
        .filter(|d| {
            doc_query.is_empty()
                || d.label.to_lowercase().contains(&doc_query)
                || d.profile.to_lowercase().contains(&doc_query)
                || d.content.to_lowercase().contains(&doc_query)
        })
        .collect();

    let api_key = settings.active_api_key().to_string();
    let model = settings.active_model().to_string();

    let current_profile = selected_profile.read().clone().unwrap_or_default();
    // Export is available if the selected profile has any forms or any docs.
    let can_export = all_refs.iter().any(|r| r.profile == current_profile)
        || all_docs.iter().any(|d| d.profile == current_profile);

    rsx! {
        div { class: "references-page",
            div { class: "references-header",
                div {
                    h2 { "Reference Forms" }
                    span { class: "references-subtitle", "{total_refs} reference(s)" }
                }
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| on_close.call(()),
                    "✕ Close"
                }
            }

            div { class: "references-content",

                if let Some((ok, msg)) = status.read().clone() {
                    div {
                        class: if ok { "references-status references-status-ok" } else { "references-status references-status-err" },
                        "{msg}"
                    }
                }

                // ── Add a reference form (original PDF + final AEM package) ──────
                section { class: "references-card",
                    h3 { "Add a reference form" }
                    p { class: "references-card-desc",
                        "Pick the original input form and the resulting AEM package. The form is analysed and described automatically, then stored for matching."
                    }
                    div { class: "references-row",
                        div { class: "references-row-info",
                            span { class: "references-row-label", "Profile" }
                            span { class: "references-row-desc",
                                "The reference is added to (and imported/exported for) this profile."
                            }
                        }
                        select {
                            class: "settings-select-model",
                            value: "{current_profile}",
                            onchange: move |e: Event<FormData>| selected_profile.set(Some(e.value())),
                            for p in profiles.iter() {
                                option {
                                    value: "{p}",
                                    selected: current_profile == *p,
                                    "{p}"
                                }
                            }
                        }
                    }
                    div { class: "references-row",
                        div { class: "references-row-info",
                            span { class: "references-row-label", "Original PDF(s)" }
                            span { class: "references-row-desc",
                                {
                                    let names: Vec<String> = pdfs.read().iter().map(|(n, _)| n.clone()).collect();
                                    if names.is_empty() {
                                        "The input form (one or more XFA PDFs).".to_string()
                                    } else {
                                        names.join(", ")
                                    }
                                }
                            }
                        }
                        input {
                            r#type: "file",
                            accept: ".pdf",
                            multiple: true,
                            onchange: move |evt: Event<FormData>| async move {
                                let mut collected = Vec::new();
                                for f in evt.files() {
                                    if let Ok(b) = f.read_bytes().await {
                                        collected.push((f.name(), b.to_vec()));
                                    }
                                }
                                if !collected.is_empty() {
                                    pdfs.set(collected);
                                }
                            },
                        }
                    }
                    div { class: "references-row",
                        div { class: "references-row-info",
                            span { class: "references-row-label", "Final AEM package" }
                            span { class: "references-row-desc",
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
                    div { class: "references-row references-row-actions",
                        button {
                            class: "btn btn-primary",
                            disabled: *busy.read(),
                            onclick: move |_| {
                                let api_key = api_key.clone();
                                let model = model.clone();
                                let pdf_data = pdfs.read().clone();
                                let pkg_data = pkg.read().clone();
                                let profile = selected_profile.read().clone();
                                spawn(async move {
                                    let Some(profile) = profile.filter(|p| !p.is_empty()) else {
                                        status.set(Some((false, "Select a profile first.".into())));
                                        return;
                                    };
                                    let Some((_, pkg_bytes)) = pkg_data else {
                                        status.set(Some((false, "Pick an AEM package first.".into())));
                                        return;
                                    };
                                    if pdf_data.is_empty() {
                                        status.set(Some((false, "Pick at least one original PDF first.".into())));
                                        return;
                                    }
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
                                        pdf_data.clone(),
                                        files.clone(),
                                        Some(profile.as_str()),
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
                                    // Label from the first PDF; note how many more were bundled.
                                    let first = pdf_data[0].0.trim_end_matches(".pdf").to_string();
                                    let label = if pdf_data.len() > 1 {
                                        format!("{first} (+{} more)", pdf_data.len() - 1)
                                    } else {
                                        first
                                    };
                                    let result = tokio::task::spawn_blocking(move || {
                                        let emb = crate::references::embed_description(&description)?;
                                        // Order-independent content hash over all PDFs (as sessions hash).
                                        let ref_id = crate::references::compute_ref_id(&pdf_data);
                                        let rows: Vec<(u32, Vec<u8>)> = pdf_data
                                            .into_iter()
                                            .map(|(_, b)| (crate::references::pdf_state_count(&b), b))
                                            .collect();
                                        crate::references::add_reference(
                                            &profile, &ref_id, &label, &description, &emb,
                                            &rows, &files,
                                        )
                                    })
                                    .await
                                    .unwrap_or_else(|e| Err(e.to_string()));

                                    busy.set(false);
                                    match result {
                                        Ok(()) => {
                                            status.set(Some((true, "Reference form added.".into())));
                                            pdfs.set(Vec::new());
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
                }

                // ── Existing references (all profiles, searchable) ──────────────
                section { class: "references-card",
                    h3 { "Existing references ({total_refs})" }
                    input {
                        class: "references-search",
                        r#type: "text",
                        placeholder: "Search references by name or profile…",
                        value: "{search.read()}",
                        oninput: move |e: Event<FormData>| search.set(e.value()),
                    }
                    if total_refs == 0 {
                        p { class: "references-empty", "No reference forms yet." }
                    } else if filtered.is_empty() {
                        p { class: "references-empty", "No references match your search." }
                    } else {
                        ul { class: "session-list",
                            for r in filtered.iter() {
                                li {
                                    key: "{r.ref_id}",
                                    class: "session-item",
                                    div { class: "session-meta",
                                        span { class: "session-label", "{r.label}" }
                                        span { class: "session-submeta",
                                            "{r.profile} · {r.pdf_count} pdf(s) · {r.files.len()} file(s)"
                                        }
                                        if !r.description.trim().is_empty() {
                                            {
                                                let ref_id = r.ref_id.clone();
                                                let is_expanded = expanded.read().contains(&r.ref_id);
                                                rsx! {
                                                    span {
                                                        class: if is_expanded { "references-item-desc references-item-desc-expanded" } else { "references-item-desc" },
                                                        title: if is_expanded { "Click to collapse" } else { "Click to expand" },
                                                        onclick: move |_| {
                                                            let mut set = expanded.write();
                                                            if set.contains(&ref_id) {
                                                                set.remove(&ref_id);
                                                            } else {
                                                                set.insert(ref_id.clone());
                                                            }
                                                        },
                                                        "{r.description}"
                                                    }
                                                }
                                            }
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
                }

                // ── Add reference documentation (plain txt/md) ──────────────────
                section { class: "references-card",
                    h3 { "Add reference documentation" }
                    p { class: "references-card-desc",
                        "Upload a plain text or Markdown file to keep alongside this profile's references."
                    }
                    div { class: "references-row",
                        div { class: "references-row-info",
                            span { class: "references-row-label", "Documentation file (.txt / .md)" }
                            span { class: "references-row-desc",
                                {doc_file.read().as_ref().map(|(n, _)| n.clone()).unwrap_or_else(|| "A plain .txt or .md file.".to_string())}
                            }
                        }
                        input {
                            r#type: "file",
                            accept: ".txt,.md,text/plain,text/markdown",
                            onchange: move |evt: Event<FormData>| async move {
                                if let Some(f) = evt.files().into_iter().next()
                                    && let Ok(b) = f.read_bytes().await
                                {
                                    doc_file.set(Some((f.name(), String::from_utf8_lossy(&b).to_string())));
                                }
                            },
                        }
                    }
                    div { class: "references-row references-row-actions",
                        button {
                            class: "btn btn-primary",
                            disabled: doc_file.read().is_none(),
                            onclick: move |_| {
                                let profile = selected_profile.read().clone();
                                let file = doc_file.read().clone();
                                let Some(profile) = profile.filter(|p| !p.is_empty()) else {
                                    status.set(Some((false, "Select a profile first.".into())));
                                    return;
                                };
                                let Some((name, content)) = file else {
                                    status.set(Some((false, "Pick a documentation file first.".into())));
                                    return;
                                };
                                let label = name
                                    .trim_end_matches(".md")
                                    .trim_end_matches(".txt")
                                    .to_string();
                                let doc_id = crate::references::compute_doc_id(&content);
                                match crate::references::add_doc(&profile, &doc_id, &label, &content) {
                                    Ok(()) => {
                                        status.set(Some((true, "Documentation added.".into())));
                                        doc_file.set(None);
                                        let n = refresh();
                                        refresh.set(n + 1);
                                    }
                                    Err(e) => status.set(Some((false, format!("Add failed: {e}")))),
                                }
                            },
                            "Add documentation"
                        }
                    }
                }

                // ── Existing documentation (all profiles, searchable) ───────────
                section { class: "references-card",
                    h3 { "Reference documentation ({total_docs})" }
                    input {
                        class: "references-search",
                        r#type: "text",
                        placeholder: "Search documentation by name, profile, or content…",
                        value: "{doc_search.read()}",
                        oninput: move |e: Event<FormData>| doc_search.set(e.value()),
                    }
                    if total_docs == 0 {
                        p { class: "references-empty", "No reference documentation yet." }
                    } else if filtered_docs.is_empty() {
                        p { class: "references-empty", "No documentation matches your search." }
                    } else {
                        ul { class: "session-list",
                            for d in filtered_docs.iter() {
                                li {
                                    key: "{d.doc_id}",
                                    class: "session-item",
                                    div { class: "session-meta",
                                        span { class: "session-label", "{d.label}" }
                                        span { class: "session-submeta",
                                            "{d.profile} · {d.content.chars().count()} chars"
                                        }
                                        if !d.content.trim().is_empty() {
                                            {
                                                let doc_id = d.doc_id.clone();
                                                let is_expanded = doc_expanded.read().contains(&d.doc_id);
                                                rsx! {
                                                    span {
                                                        class: if is_expanded { "references-item-desc references-item-desc-expanded" } else { "references-item-desc" },
                                                        title: if is_expanded { "Click to collapse" } else { "Click to expand" },
                                                        onclick: move |_| {
                                                            let mut set = doc_expanded.write();
                                                            if set.contains(&doc_id) {
                                                                set.remove(&doc_id);
                                                            } else {
                                                                set.insert(doc_id.clone());
                                                            }
                                                        },
                                                        "{d.content}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    button {
                                        class: "btn btn-secondary btn-sm",
                                        onclick: {
                                            let doc_id = d.doc_id.clone();
                                            move |_| {
                                                crate::references::delete_doc(&doc_id);
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
                }

                // ── Import / export dataset (scoped to the selected profile) ────
                section { class: "references-card",
                    h3 { "Import / Export" }
                    div { class: "references-row",
                        div { class: "references-row-info",
                            span { class: "references-row-label", "Import dataset" }
                            span { class: "references-row-desc",
                                {import_file.read().as_ref().map(|(n, _)| n.clone()).unwrap_or_else(|| "Load a reference dataset (.db) into the selected profile.".to_string())}
                            }
                        }
                        input {
                            r#type: "file",
                            accept: ".db,.sqlite",
                            onchange: move |evt: Event<FormData>| async move {
                                if let Some(f) = evt.files().into_iter().next()
                                    && let Ok(b) = f.read_bytes().await
                                {
                                    import_file.set(Some((f.name(), b.to_vec())));
                                }
                            },
                        }
                    }
                    div { class: "references-row references-row-actions",
                        button {
                            class: "btn btn-primary",
                            disabled: *busy.read() || import_file.read().is_none(),
                            onclick: move |_| {
                                let profile = selected_profile.read().clone();
                                let file = import_file.read().clone();
                                spawn(async move {
                                    let Some(profile) = profile.filter(|p| !p.is_empty()) else {
                                        status.set(Some((false, "Select a profile first.".into())));
                                        return;
                                    };
                                    let Some((_, bytes)) = file else {
                                        status.set(Some((false, "Pick a dataset file first.".into())));
                                        return;
                                    };
                                    busy.set(true);
                                    status.set(Some((true, "Importing dataset…".into())));
                                    let res = tokio::task::spawn_blocking(move || {
                                        let tmp = std::env::temp_dir().join("blueprint-ref-import.db");
                                        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
                                        let r = crate::references::import_reference_db(
                                            &tmp.to_string_lossy(),
                                            &profile,
                                        );
                                        let _ = std::fs::remove_file(&tmp);
                                        r
                                    })
                                    .await
                                    .unwrap_or_else(|e| Err(e.to_string()));
                                    busy.set(false);
                                    match res {
                                        Ok((refs, docs)) => {
                                            status.set(Some((true, format!("Imported {refs} reference(s), {docs} doc(s)."))));
                                            import_file.set(None);
                                            let c = refresh();
                                            refresh.set(c + 1);
                                        }
                                        Err(e) => status.set(Some((false, format!("Import failed: {e}")))),
                                    }
                                });
                            },
                            if *busy.read() { "Working…" } else { "Import dataset" }
                        }
                    }
                    div { class: "references-row",
                        div { class: "references-row-info",
                            span { class: "references-row-label", "Export references" }
                            span { class: "references-row-desc",
                                "Save the selected profile's references and documentation to a dataset in your Downloads folder."
                            }
                        }
                        button {
                            class: "btn btn-secondary",
                            disabled: !can_export,
                            onclick: move |_| {
                                let profile = selected_profile.read().clone();
                                spawn(async move {
                                    let Some(profile) = profile.filter(|p| !p.is_empty()) else {
                                        status.set(Some((false, "Select a profile first.".into())));
                                        return;
                                    };
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
                                        Ok((refs, docs)) => {
                                            status.set(Some((true, format!("Exported {refs} reference(s), {docs} doc(s) to {}", out.display()))));
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
    }
}

/// Web stub — reference forms require the desktop app's local database.
#[cfg(target_arch = "wasm32")]
#[component]
pub fn ReferencesPage(
    profile: Option<String>,
    settings: AppSettings,
    on_close: EventHandler<()>,
) -> Element {
    let _ = (&profile, &settings);
    rsx! {
        div { class: "references-page",
            div { class: "references-header",
                div { h2 { "Reference Forms" } }
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| on_close.call(()),
                    "✕ Close"
                }
            }
            div { class: "references-content",
                p { class: "references-empty",
                    "Reference forms are only available in the desktop app."
                }
            }
        }
    }
}
