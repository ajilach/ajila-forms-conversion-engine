//! Redacto output profile, loaded from `profiles/{name}/redacto/config.toml`.
//!
//! Like the AEM profile, all string fields accept
//! [Tera](https://keats.github.io/tera/) syntax over two namespaces:
//!
//! - `xfa.*`       — raw XFA `<variables><text>` values extracted from the PDF
//! - `variables.*` — user-defined intermediate values (themselves templates)

use serde::Deserialize;
use std::collections::HashMap;

use super::Status;
use crate::template;

/// Default document configuration schema marker.
const DEFAULT_SCHEMA: &str = "redacto-document/v1";
/// Default AEM authoring root for Redacto documents.
const DEFAULT_FORM_PATH_ROOT: &str = "/content/forms/af/redacto-documents";
/// Default stylesheet file name.
const DEFAULT_STYLE: &str = "ubs-default.css";
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
    /// Defaults to `ubs-default.css`.
    #[serde(default)]
    pub style: Option<String>,

    /// Tera template for the page header (`${meta:header}`).
    #[serde(default)]
    pub header: Option<String>,

    /// Tera template for the page footer (`${meta:footer}`).
    #[serde(default)]
    pub footer: Option<String>,

    /// Authoring user recorded as the document owner. Defaults to `admin`.
    #[serde(default)]
    pub owner_id: Option<String>,

    /// Document configuration schema marker. Defaults to
    /// `redacto-document/v1`.
    #[serde(default)]
    pub schema: Option<String>,

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
    /// Page header text.
    pub header: String,
    /// Page footer text.
    pub footer: String,
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
    /// Build a configuration from a profile and a document
    /// [`Context`](crate::Context).
    ///
    /// [`RedactoConfig::languages`] is seeded with the master language;
    /// [`resolve_redacto_languages`](crate::resolve_redacto_languages) fills in
    /// the languages actually present in the content.
    pub fn from_profile(
        profile: &RedactoProfile,
        ctx: &crate::Context,
    ) -> Result<Self, crate::Error> {
        let xfa_vars = ctx.variables.clone();
        let user_vars = template::resolve_variables(&profile.variables, &xfa_vars)?;
        let mut tera_ctx = template::build_context(&xfa_vars, &user_vars);
        // Expose document furniture recovered by the analysis under `page.*`,
        // so a profile template can reinstate it, e.g. `{{ page.header }}`.
        tera_ctx.insert(
            "page",
            &serde_json::json!({ "header": ctx.header.clone().unwrap_or_default() }),
        );

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
        let render_opt = |field: &str, tmpl: &Option<String>| -> Result<String, crate::Error> {
            match tmpl {
                Some(t) => render(field, t),
                None => Ok(String::new()),
            }
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

        Ok(Self {
            document_id,
            title,
            form_path,
            style: profile
                .style
                .clone()
                .unwrap_or_else(|| DEFAULT_STYLE.into()),
            header: render_opt("header", &profile.header)?,
            footer: render_opt("footer", &profile.footer)?,
            owner_id: profile
                .owner_id
                .clone()
                .unwrap_or_else(|| DEFAULT_OWNER_ID.into()),
            schema: profile
                .schema
                .clone()
                .unwrap_or_else(|| DEFAULT_SCHEMA.into()),
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
