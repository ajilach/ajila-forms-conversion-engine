//! Redacto output profile, loaded from `profiles/{name}/redacto/config.toml`.
//!
//! Like the AEM profile, all string fields accept
//! [Tera](https://keats.github.io/tera/) syntax over two namespaces:
//!
//! - `xfa.*`       — raw XFA `<variables><text>` values extracted from the PDF
//! - `variables.*` — user-defined intermediate values (themselves templates)

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use super::Status;
use crate::template;

/// The document configuration schema this exporter emits.
const SCHEMA: &str = "redacto-document/v2";
/// Default AEM authoring root for Redacto documents.
const DEFAULT_FORM_PATH_ROOT: &str = "/content/forms/af/redacto-documents";
/// Default stylesheet file name.
const DEFAULT_STYLE: &str = "default.css";
/// Default owner of generated documents.
const DEFAULT_OWNER_ID: &str = "admin";
/// Default panel style for a two-column grid layout.
const DEFAULT_GRID_PANEL_STYLE: &str = "layout-split-block";
/// Default panel style for a multi-column text flow.
const DEFAULT_COLUMN_PANEL_STYLE: &str = "layout-split";
/// Default panel style wrapping the trailing footnote assets.
const DEFAULT_FOOTNOTE_PANEL_STYLE: &str = "footnote";
/// Default language when the content carries none.
const DEFAULT_LANGUAGE: &str = "en";

/// Maximum length of `documents.document_id` and `assets.asset_id`.
const MAX_ID_LEN: usize = 50;
/// Maximum length of `documents.form_path`.
const MAX_FORM_PATH_LEN: usize = 200;

/// A Redacto output profile as written in TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct RedactoProfile {
    /// Tera template for `documents.document_id`, e.g.
    /// `"{{ xfa.formrange_code | lower }}_{{ xfa.formrange_entity }}"`.
    pub document_id: String,

    /// Tera template for the document title shown in the rendered output.
    pub title: String,

    /// Tera template for `documents.form_path`. Defaults to
    /// `/content/forms/af/redacto-documents/{document_id}`.
    #[serde(default)]
    pub form_path: Option<String>,

    /// Stylesheet file name resolved from the Redacto bundle.
    /// Defaults to `default.css`.
    #[serde(default)]
    pub style: Option<String>,

    /// Tera template for the page header. Rendered once per language, against
    /// that language's own XFA variables and recovered `page.header`, and
    /// emitted as a text asset in the configuration's `header` section.
    #[serde(default)]
    pub header: Option<String>,

    /// Named fields making up the page footer, rendered once per language like
    /// [`header`](Self::header) against that language's own XFA variables. Each
    /// field becomes its own `<span class="{class}">value</span>` in the footer
    /// text asset, in the order listed here, separated from its neighbours by a
    /// literal space; a field whose rendered value is blank for a language is
    /// skipped for that language rather than printing an empty span. The footer
    /// asset also always carries the legacy page-number counter after the field
    /// spans (not configurable — see
    /// [`render_footer_html`](super::content::render_footer_html)), mirroring
    /// what `HtmlDocumentService.renderLegacyFurniture` added automatically for
    /// every v1 document.
    ///
    /// Defaults to no fields, so a profile without a UBS-style footer still
    /// resolves.
    #[serde(default)]
    pub footer_fields: Vec<FooterFieldTemplate>,

    /// Authoring user recorded as the document owner. Defaults to `admin`.
    #[serde(default)]
    pub owner_id: Option<String>,

    /// Lifecycle status of the generated variants. Defaults to `DRAFT`.
    #[serde(default)]
    pub status: Option<String>,

    /// `styledPanel` style applied to a `GridLayout`.
    /// Defaults to `layout-split-block`.
    #[serde(default)]
    pub grid_panel_style: Option<String>,

    /// `styledPanel` style wrapping the trailing footnote assets.
    /// Defaults to `footnote`.
    #[serde(default)]
    pub footnote_panel_style: Option<String>,

    /// `styledPanel` style applied to a multi-column text flow.
    /// Defaults to `layout-split`.
    #[serde(default)]
    pub column_panel_style: Option<String>,

    /// Primary language code. Defaults to `en`.
    #[serde(default)]
    pub master_language: Option<String>,

    /// User-defined intermediate variables available as `variables.*`.
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

/// One named footer field template, e.g. the UBS form-id or version column.
/// Field names are entirely profile-defined — nothing here is UBS-specific.
#[derive(Debug, Clone, Deserialize)]
pub struct FooterFieldTemplate {
    /// CSS class applied to the field's `<span>` in the footer asset, e.g.
    /// `footer-form-id`.
    pub class: String,
    /// Tera template for the field's value, rendered per language like
    /// [`RedactoProfile::header`].
    pub template: String,
}

/// Normalise a rendered header/footer value.
///
/// Line structure is load-bearing now that the page furniture ships as a text
/// asset — a recovered header stacks its lines and the renderer honours them —
/// so only the line endings and the surrounding blank space are touched. The
/// value stays plain text; escaping happens when the asset body is rendered
/// (see [`render_header_html`](super::content::render_header_html) and
/// [`render_footer_html`](super::content::render_footer_html)).
fn normalize_furniture(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

/// Build the Tera environment for one document context: the profile's
/// user-defined variables resolved against that context's XFA variables, plus
/// the document furniture the analysis recovered under `page.*` so a profile
/// template can reinstate it, e.g. `{{ page.header }}`.
fn tera_ctx_for(
    profile: &RedactoProfile,
    ctx: &crate::Context,
) -> Result<tera::Context, crate::Error> {
    let xfa_vars = ctx.variables.clone();
    let user_vars = template::resolve_variables(&profile.variables, &xfa_vars)?;
    let mut tera_ctx = template::build_context(&xfa_vars, &user_vars);
    tera_ctx.insert(
        "page",
        &serde_json::json!({ "header": ctx.header.clone().unwrap_or_default() }),
    );
    Ok(tera_ctx)
}

/// One resolved footer field for one language: its CSS class and rendered,
/// normalized (but not yet HTML-escaped) value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FooterField {
    /// CSS class applied to the field's `<span>`.
    pub class: String,
    /// The rendered value, plain text.
    pub value: String,
}

/// A fully resolved Redacto configuration: every Tera template rendered and
/// every default applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactoConfig {
    /// `documents.document_id`.
    pub document_id: String,
    /// Document title.
    pub title: String,
    /// `documents.form_path`.
    pub form_path: String,
    /// Stylesheet file name.
    pub style: String,
    /// Page header text per language code, plain and possibly multi-line.
    ///
    /// Ordered, so a dump built from the same inputs is byte-identical.
    pub headers: BTreeMap<String, String>,
    /// Page footer fields per language: each language's ordered list of
    /// resolved `(class, value)` fields. A field's value may be blank for a
    /// given language; the converter decides the per-language/whole-record
    /// fallback and skips blank fields when it builds the asset body.
    pub footers: BTreeMap<String, Vec<FooterField>>,
    /// Owner recorded in the mandatory `(USER, OWNER, DOCUMENT)` row.
    pub owner_id: String,
    /// Document configuration schema marker.
    pub schema: String,
    /// Lifecycle status of the generated variants.
    pub status: Status,
    /// `styledPanel` style used for a `GridLayout`.
    pub grid_panel_style: String,
    /// `styledPanel` style wrapping the trailing footnote assets.
    pub footnote_panel_style: String,
    /// `styledPanel` style applied to a multi-column text flow.
    pub column_panel_style: String,
    /// Languages that receive an `asset_version` / `document_version` row.
    pub languages: Vec<String>,
    /// Primary language code.
    pub master_language: String,
    /// Value written into every `created` column.
    pub created: String,
}

impl RedactoConfig {
    /// Build a configuration from a profile, the master-language document
    /// [`Context`](crate::Context) and one context per language variant.
    ///
    /// The document's identity (id, title, path, styles) is single-valued and
    /// comes from `master_ctx`. The page header and footer are per-language:
    /// each language variant carries its own recovered master-page header and
    /// its own `Footer_Line_*` XFA variables, so their templates are rendered
    /// once per context in `language_ctxs` and shipped as text assets.
    ///
    /// [`RedactoConfig::languages`] is seeded with the master language;
    /// [`resolve_redacto_languages`](crate::resolve_redacto_languages) fills in
    /// the languages actually present in the content.
    pub fn from_profile(
        profile: &RedactoProfile,
        master_ctx: &crate::Context,
        language_ctxs: &[crate::Context],
    ) -> Result<Self, crate::Error> {
        let tera_ctx = tera_ctx_for(profile, master_ctx)?;

        // Name the failing field: a Redacto profile is usually driven by XFA
        // variables, and the bare Tera error does not say which one is missing.
        let render = |field: &str, tmpl: &str| -> Result<String, crate::Error> {
            template::render_string(tmpl, &tera_ctx).map_err(|e| {
                crate::Error::Profile(format!(
                    "redacto profile field '{field}' could not be rendered \
                     (are the required XFA variables present?): {e}"
                ))
            })
        };

        let document_id = render("document_id", &profile.document_id)?;
        let title = render("title", &profile.title)?;

        let form_path = match &profile.form_path {
            Some(tmpl) => render("form_path", tmpl)?,
            None => format!("{DEFAULT_FORM_PATH_ROOT}/{document_id}"),
        };

        let status = match &profile.status {
            Some(s) => Status::parse(s).ok_or_else(|| {
                crate::Error::Profile(format!("unknown Redacto status '{s}' in redacto profile"))
            })?,
            None => Status::Draft,
        };

        let master_language = profile
            .master_language
            .clone()
            .unwrap_or_else(|| DEFAULT_LANGUAGE.into());

        // The master context is a language variant like any other; include it
        // so a single-PDF run still gets its furniture. First context per
        // language wins.
        let mut headers = BTreeMap::new();
        let mut footers = BTreeMap::new();
        for ctx in std::iter::once(master_ctx).chain(language_ctxs) {
            let language = ctx.language().to_string();
            if headers.contains_key(&language) {
                continue;
            }
            let ctx_tera = tera_ctx_for(profile, ctx)?;
            let render_furniture =
                |field: &str, tmpl: &Option<String>| -> Result<String, crate::Error> {
                    let Some(tmpl) = tmpl else {
                        return Ok(String::new());
                    };
                    let rendered = template::render_string(tmpl, &ctx_tera).map_err(|e| {
                        crate::Error::Profile(format!(
                            "redacto profile field '{field}' could not be rendered for \
                             language '{language}' (are the required XFA variables \
                             present?): {e}"
                        ))
                    })?;
                    Ok(normalize_furniture(&rendered))
                };
            headers.insert(
                language.clone(),
                render_furniture("header", &profile.header)?,
            );

            let mut fields = Vec::with_capacity(profile.footer_fields.len());
            for field in &profile.footer_fields {
                let rendered = template::render_string(&field.template, &ctx_tera).map_err(|e| {
                    crate::Error::Profile(format!(
                        "redacto profile footer field '{}' could not be rendered for \
                         language '{language}' (are the required XFA variables \
                         present?): {e}",
                        field.class
                    ))
                })?;
                fields.push(FooterField {
                    class: field.class.clone(),
                    value: normalize_furniture(&rendered),
                });
            }
            footers.insert(language.clone(), fields);
        }

        Ok(Self {
            document_id,
            title,
            form_path,
            style: profile
                .style
                .clone()
                .unwrap_or_else(|| DEFAULT_STYLE.into()),
            headers,
            footers,
            owner_id: profile
                .owner_id
                .clone()
                .unwrap_or_else(|| DEFAULT_OWNER_ID.into()),
            schema: SCHEMA.to_string(),
            status,
            grid_panel_style: profile
                .grid_panel_style
                .clone()
                .unwrap_or_else(|| DEFAULT_GRID_PANEL_STYLE.into()),
            footnote_panel_style: profile
                .footnote_panel_style
                .clone()
                .unwrap_or_else(|| DEFAULT_FOOTNOTE_PANEL_STYLE.into()),
            column_panel_style: profile
                .column_panel_style
                .clone()
                .unwrap_or_else(|| DEFAULT_COLUMN_PANEL_STYLE.into()),
            languages: vec![master_language.clone()],
            master_language,
            created: crate::util::sql_now(),
        })
    }

    /// Report values that exceed the width of their database column.
    ///
    /// Values are never truncated — an over-long identifier is a profile bug
    /// and should be surfaced rather than silently mangled.
    pub(super) fn column_width_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.document_id.chars().count() > MAX_ID_LEN {
            warnings.push(format!(
                "document_id '{}' exceeds documents.document_id varchar({MAX_ID_LEN})",
                self.document_id
            ));
        }
        if self.form_path.chars().count() > MAX_FORM_PATH_LEN {
            warnings.push(format!(
                "form_path '{}' exceeds documents.form_path varchar({MAX_FORM_PATH_LEN})",
                self.form_path
            ));
        }
        warnings
    }
}
