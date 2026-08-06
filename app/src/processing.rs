//! Derived outputs for a converted document: the exports the agent run offers
//! on its done screen, each generated only when the active profile configures
//! it and the document actually has content to emit.

use blueprint::DocumentEnvelope;

/// Generate the standalone HTML preview for `envelope`, if the profile has an
/// `html/` section and the document has content.
///
/// The profile's typefaces travel inside the document: `load_html_custom_styles`
/// embeds them as base64 `@font-face` data-URIs, so the preview needs no font
/// manager. Shared by the standard pipeline and the agent run.
pub fn html_preview_for(envelope: &DocumentEnvelope, profile: Option<&str>) -> Option<String> {
    let profile_name = profile?;
    if envelope.content.is_empty() || !blueprint::has_html_config(profile_name) {
        return None;
    }
    let styles = blueprint::load_html_custom_styles(profile_name).ok()?;
    let config = blueprint::HtmlConfig {
        custom_styles: Some(styles),
        ..blueprint::HtmlConfig::default()
    };
    Some(blueprint::to_html(&envelope.content, &config))
}

/// Generate the XSD schema for `envelope`, if the profile has an `xsd/` section
/// and the document has content.
///
/// `form_code` is the code the AEM package was built with, so the schema names
/// the same form; pass `None` to leave the profile default in place.
pub fn xsd_schema_for(
    envelope: &DocumentEnvelope,
    profile: Option<&str>,
    form_code: Option<&str>,
) -> Option<String> {
    let profile_name = profile?;
    if envelope.content.is_empty() || !blueprint::has_xsd_config(profile_name) {
        return None;
    }
    let mut config = blueprint::load_xsd_config(profile_name).ok()?;
    if let Some(code) = form_code {
        config.form_code = Some(code.to_string());
    }
    Some(blueprint::to_xsd(&envelope.content, &config))
}

/// Generate the Redacto PostgreSQL dump for `envelope`, if the profile has a
/// `redacto/config.toml`, its templates resolve against the document, and the
/// document actually has content to emit.
///
/// Returns `None` when the profile has no Redacto section, the document does
/// not carry the XFA variables the profile's identity templates need, or the
/// dump would contain no text assets at all. That last case is the important
/// one: a contentless dump is still valid SQL describing an empty document, so
/// without this guard it reaches the download button looking like a result.
pub fn redacto_sql_for(envelope: &DocumentEnvelope, profile: Option<&str>) -> Option<String> {
    let profile_name = profile?;
    if !blueprint::has_redacto_config(profile_name) {
        return None;
    }
    let config = blueprint::load_redacto_config(profile_name, &envelope.context).ok()?;
    let config = blueprint::resolve_redacto_languages(&envelope.content, &config);
    let dump = blueprint::generate_redacto_dump(&envelope.content, &config);
    if dump.assets.is_empty() {
        return None;
    }
    Some(dump.to_sql())
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

    /// Regression: an empty structured tree used to yield a syntactically valid
    /// dump with a `documents` row and no assets at all, which reached the
    /// download button as an importable file describing an empty document. A
    /// dump with no content is a failure, not an output.
    #[test]
    fn redacto_sql_for_empty_content_returns_none() {
        let mut envelope =
            envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);
        envelope.content = Vec::new();

        assert!(
            redacto_sql_for(&envelope, Some("ubs")).is_none(),
            "a dump with no content assets must not be offered for download"
        );
    }

    /// The agent done screen offers the preview straight from
    /// `ProcessingState::html_preview`, so this is the whole data path.
    #[test]
    fn html_preview_is_generated_for_the_ubs_profile() {
        let envelope = envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);

        let html = html_preview_for(&envelope, Some("ubs")).expect("ubs profile yields HTML");

        assert!(html.contains("Ein Textabschnitt."), "{html}");
        // The profile's typefaces travel inside the document rather than via the
        // font manager, which is why the agent path needs no font loading.
        assert!(html.contains("@font-face"), "custom styles must be inlined");
    }

    #[test]
    fn html_preview_is_absent_without_a_profile_or_content() {
        let mut envelope =
            envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);

        assert!(html_preview_for(&envelope, None).is_none());
        assert!(html_preview_for(&envelope, Some("missing-profile")).is_none());

        envelope.content = Vec::new();
        assert!(
            html_preview_for(&envelope, Some("ubs")).is_none(),
            "an empty document must not be offered as a preview"
        );
    }

    /// The form code comes from the AEM package the agent built, so the schema
    /// names the same form.
    #[test]
    fn xsd_schema_uses_the_supplied_form_code() {
        let envelope = envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);

        let xsd = xsd_schema_for(&envelope, Some("ubs"), Some("AAAD_001"))
            .expect("ubs profile yields a schema");

        assert!(xsd.contains("<?xml"), "{xsd}");
        assert!(xsd.contains("AAAD_001"), "{xsd}");
    }

    #[test]
    fn xsd_schema_is_absent_without_a_profile_or_content() {
        let mut envelope =
            envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);

        assert!(xsd_schema_for(&envelope, None, None).is_none());
        assert!(xsd_schema_for(&envelope, Some("missing-profile"), None).is_none());

        envelope.content = Vec::new();
        assert!(
            xsd_schema_for(&envelope, Some("ubs"), None).is_none(),
            "an empty document must not be offered as a schema"
        );
    }

    /// The same guard for a document whose content carries no *renderable*
    /// blocks: input fields alone produce zero assets, which is the shape a
    /// non-text-only document takes.
    #[test]
    fn redacto_sql_for_field_only_content_returns_none() {
        use blueprint::structured::{FieldNode, FieldType};

        let mut envelope =
            envelope_with(&[("formrange_code", "AAAD"), ("formrange_entity", "001")]);
        envelope.content = vec![StructuredNode::Field(FieldNode {
            name: "first_name".into(),
            som_path: None,
            label: Some(TranslatedText::plain("First name")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
            required: false,
        })];

        assert!(redacto_sql_for(&envelope, Some("ubs")).is_none());
    }
}
