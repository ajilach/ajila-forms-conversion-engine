//! Unified processing invocation — shared between desktop and web.

use dioxus::prelude::*;

use crate::models::{DocumentEnvelope, ProcessingState, ProcessingStep};

/// Returns `true` if the upload consists of a single JSON file that should be
/// loaded directly as a structured [`DocumentEnvelope`] instead of running the
/// PDF pipeline.
pub fn is_json_upload(files: &[(String, Vec<u8>)]) -> bool {
    files.len() == 1
        && files[0]
            .0
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

/// Load a [`DocumentEnvelope`] directly from an uploaded JSON file, bypassing
/// the PDF pipeline.
///
/// The JSON is parsed into a `DocumentEnvelope`, all derived outputs are
/// regenerated for the active profile, and an initial editing session is
/// recorded (desktop only). Returns the parsed envelope on success.
pub fn load_envelope_from_json(
    files: &[(String, Vec<u8>)],
    profile: Option<String>,
    mut processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let Some((name, bytes)) = files.first() else {
        return;
    };

    let envelope: DocumentEnvelope = match serde_json::from_slice(bytes) {
        Ok(env) => env,
        Err(e) => {
            processing_state.set(ProcessingState {
                step: ProcessingStep::Idle,
                error: Some(format!("Failed to parse JSON as structured document: {e}")),
                ..ProcessingState::new()
            });
            return;
        }
    };

    let label = name.clone();
    finalize_envelope(
        &envelope,
        files,
        profile.as_deref(),
        processing_state,
        current_session,
        SessionLabels {
            session: &label,
            edit: "Imported from JSON",
        },
        false,
    );
}

/// Labels for the initial editing session recorded by [`finalize_envelope`].
pub struct SessionLabels<'a> {
    /// Names the session.
    pub session: &'a str,
    /// Labels the initial snapshot.
    pub edit: &'a str,
}

/// Finalize a structured [`DocumentEnvelope`] that was loaded outside the PDF
/// pipeline (JSON import or AI generation): load profile fonts, regenerate all
/// derived outputs, mark processing complete, and record an initial editing
/// session (desktop only; the `db` calls are no-ops on web).
///
/// `files` are the source bytes used to compute the document hash; `labels`
/// names the session and its initial snapshot.
pub fn finalize_envelope(
    envelope: &DocumentEnvelope,
    files: &[(String, Vec<u8>)],
    profile: Option<&str>,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
    labels: SessionLabels<'_>,
    ai_mode: bool,
) {
    // Load profile fonts so derived outputs render with the right typefaces.
    if let Some(profile_name) = profile {
        let _ = blueprint::load_profile_fonts(profile_name);
    }

    let mut state = ProcessingState {
        step: ProcessingStep::Complete,
        ai_mode,
        ..ProcessingState::new()
    };
    regenerate_outputs(&mut state, envelope, profile);
    processing_state.set(state);

    // Record the initial snapshot as a new editing session (desktop only).
    if let Ok(json) = serde_json::to_string(envelope) {
        let doc_hash = crate::db::document_hash(files);
        crate::db::upsert_document(&doc_hash, labels.session);
        if let Some(session_id) = crate::db::create_session(&doc_hash, profile, labels.session) {
            crate::db::insert_edit(&session_id, labels.edit, &json);
            current_session.set(Some(session_id));
        }
    }
}

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
    // The AEM package below is rebuilt from the structured content, so any
    // previously authored AEM tree no longer describes it. Callers that still
    // have an authoritative tree (a restored session) re-attach it afterwards.
    state.aem_translated = None;

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

    // Redacto SQL dump
    state.redacto_sql = redacto_sql_for(envelope, profile);
}

/// Generate the Redacto PostgreSQL dump for `envelope`, if the profile has a
/// `redacto/config.toml` and its templates resolve against the document.
///
/// Returns `None` when the profile has no Redacto section or the document does
/// not carry the XFA variables the profile's identity templates need. Shared by
/// the standard pipeline and the agent run, which otherwise fills
/// [`ProcessingState`] itself.
pub fn redacto_sql_for(envelope: &DocumentEnvelope, profile: Option<&str>) -> Option<String> {
    let profile_name = profile?;
    if !blueprint::has_redacto_config(profile_name) {
        return None;
    }
    let config = blueprint::load_redacto_config(profile_name, &envelope.context).ok()?;
    Some(blueprint::to_redacto_sql(&envelope.content, &config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueprint::structured::{ParagraphNode, StructuredNode, TranslatedText};

    fn envelope_with(variables: &[(&str, &str)]) -> DocumentEnvelope {
        let vars = variables
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        DocumentEnvelope {
            context: blueprint::Context::new("de".into(), vars),
            content: vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain("Ein Textabschnitt."),
                som_path: None,
                source_name: None,
            })],
            state_count: 1,
        }
    }

    /// The download button in both UIs is driven purely by
    /// `ProcessingState::redacto_sql`, so this is the whole data path.
    #[test]
    fn redacto_sql_is_generated_for_the_ubs_profile() {
        let envelope = envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);

        let sql = redacto_sql_for(&envelope, Some("ubs")).expect("ubs profile yields a dump");

        assert!(sql.contains("INSERT INTO app_redacto.documents "), "{sql}");
        assert!(sql.contains("'aaad_001'"), "{sql}");
        assert!(sql.contains("<p>Ein Textabschnitt.</p>"), "{sql}");
    }

    #[test]
    fn redacto_sql_is_absent_without_a_profile_or_config() {
        let envelope = envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);

        assert!(redacto_sql_for(&envelope, None).is_none());
        assert!(redacto_sql_for(&envelope, Some("missing-profile")).is_none());
    }

    /// A document without the XFA variables the profile's identity templates
    /// need must skip the dump, not abort the conversion.
    #[test]
    fn redacto_sql_is_skipped_when_identity_variables_are_missing() {
        assert!(redacto_sql_for(&envelope_with(&[]), Some("ubs")).is_none());
    }
}
