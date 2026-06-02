//! Unified processing invocation — shared between desktop and web.

use dioxus::prelude::*;

use crate::models::{DocumentEnvelope, ProcessingState};

/// Run the blueprint pipeline and stream progress updates into the signal.
///
/// On desktop, once processing completes a new edit-history session is created
/// for the uploaded document set and its id is written into `current_session`.
pub async fn run_and_track(
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
    let profile_for_session = profile.clone();
    crate::pipeline::run_blueprint_pipeline(&files, profile, |state| {
        processing_state.set(state.clone());
    })
    .await;

    // Record the initial snapshot as a new editing session (desktop only).
    let envelope_json = processing_state
        .read()
        .envelope
        .as_ref()
        .and_then(|env| serde_json::to_string(env).ok());

    if let Some(json) = envelope_json {
        let label = files
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let doc_hash = crate::db::document_hash(&files);
        crate::db::upsert_document(&doc_hash, &label);
        if let Some(session_id) =
            crate::db::create_session(&doc_hash, profile_for_session.as_deref(), &label)
        {
            crate::db::insert_edit(&session_id, "Initial conversion", &json);
            current_session.set(Some(session_id));
        }
    }
}

/// Regenerate all derived outputs (JSON, HTML, AEM, XSD) for an envelope and
/// store them into the processing state, according to the active profile.
pub fn regenerate_outputs(
    state: &mut ProcessingState,
    envelope: &DocumentEnvelope,
    profile: Option<&str>,
) {
    state.envelope = Some(envelope.clone());

    // JSON
    if let Ok(json) = serde_json::to_string_pretty(envelope) {
        state.merged_json = Some(json);
    }

    // HTML preview
    if let Some(profile_name) = profile
        && blueprint::has_html_config(profile_name)
        && let Ok(styles) = blueprint::load_html_custom_styles(profile_name)
    {
        let html_config = blueprint::HtmlConfig {
            custom_styles: Some(styles),
            ..blueprint::HtmlConfig::default()
        };
        let html = blueprint::to_html(&envelope.content, &html_config);
        state.html_preview = Some(html);
    }

    // AEM package
    if let Some(profile_name) = profile
        && blueprint::has_aem_config(profile_name)
        && let Ok(aem_config) = blueprint::load_aem_config(profile_name, &envelope.context)
    {
        let aem_zip = blueprint::to_aem_package(&envelope.content, &aem_config);
        state.form_code = Some(aem_config.form_code.clone());
        state.aem_package = Some(aem_zip);
    }

    // XSD
    if let Some(profile_name) = profile
        && blueprint::has_xsd_config(profile_name)
        && let Ok(mut xsd_config) = blueprint::load_xsd_config(profile_name)
    {
        if let Some(ref fc) = state.form_code {
            xsd_config.form_code = Some(fc.clone());
        }
        state.xsd_schema = Some(blueprint::to_xsd(&envelope.content, &xsd_config));
    }
}
