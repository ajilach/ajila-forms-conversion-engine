//! AEM profile configuration loaded from a TOML file.
//!
//! Defines the schema for AEM output profiles and handles deserialization.
//! Profile values use [Tera](https://keats.github.io/tera/) template syntax
//! and can reference two namespaces:
//!
//! - `xfa.*`  — raw XFA `<variables><text>` values extracted from the PDF
//! - `variables.*` — user-defined intermediate values (themselves Tera templates)
//!
//! Runtime values injected automatically:
//! - `master_language` — auto-detected primary language code
//! - `languages` — comma-separated list of all detected language codes
//! - `form_code` — resolved form code (from rendered `title`)
//! - `author` — authoring user name

use serde::Deserialize;
use std::collections::HashMap;

/// An AEM output profile loaded from a TOML file.
///
/// All template-typed fields accept Tera syntax. Non-template fields are
/// plain strings passed through as-is.
#[derive(Debug, Clone, Deserialize)]
pub struct AemProfile {
    /// Tera template for the human-readable form title / form code.
    /// Default: `"{{ xfa.formrange_code }}"`.
    pub title: Option<String>,

    /// Tera template for the JCR path segment between
    /// `content/forms/af/` and the form directory.
    pub form_path: Option<String>,

    /// Tera template for the JCR folder name
    /// (e.g. `"AF_{{ xfa.formrange_code }}"`).
    pub form_dir: Option<String>,

    /// Reusable intermediate variables. Each value is a Tera template that
    /// can reference `xfa.*` and previously resolved `variables.*`.
    #[serde(default)]
    pub variables: HashMap<String, String>,

    /// Path configurations (values are Tera templates).
    #[serde(default)]
    pub paths: ProfilePaths,

    /// Component resource type and CSS configurations.
    #[serde(default)]
    pub components: ProfileComponents,

    /// Language synonym mappings (e.g. `de = ["de-ch"]`).
    #[serde(default)]
    pub language_synonyms: HashMap<String, Vec<String>>,

    /// Raw XML snippets (Tera templates) for injection points.
    ///
    /// Supported keys:
    /// - `print_branding` — injected inside `<print>` (DOR branding)
    /// - `first_panel`    — injected as the first child(ren) of rootPanel items
    /// - `last_panel`     — injected as the last child(ren) of rootPanel items
    #[serde(default)]
    pub xml_snippets: HashMap<String, String>,

    /// Configurable toolbar script strings.
    #[serde(default)]
    pub scripts: ProfileScripts,
}

/// Path-related profile settings (all Tera templates).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ProfilePaths {
    pub template_path: Option<String>,
    pub page_resource_type: Option<String>,
    pub theme_ref: Option<String>,
    pub redirect_url: Option<String>,
    pub meta_template_ref: Option<String>,
    /// Tera template for DOR template reference path.
    pub dor_template_ref: Option<String>,
}

/// Component resource type and CSS settings.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ProfileComponents {
    pub resource_type_base: Option<String>,
    pub custom_resource_type_base: Option<String>,
    pub action_type: Option<String>,
    pub client_lib_ref: Option<String>,
    pub wizard_layout: Option<String>,
    /// DOR branding form type indicator (e.g. `" "` or `"K"`).
    pub form_type: Option<String>,
    /// Per-component configuration (CSS class, etc.).
    ///
    /// Example keys: `textbox`, `numericbox`, `datepicker`, `checkbox`,
    ///               `radiobutton`, `dropdownlist`, `primarybutton`,
    ///               `textbox_multiline`.
    #[serde(flatten, default)]
    pub component_config: HashMap<String, ComponentConfig>,
}

/// Per-component configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ComponentConfig {
    /// CSS class(es) applied to this component widget.
    pub css: Option<String>,
}

/// Toolbar script configuration.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ProfileScripts {
    /// Script content for the Next button's click handler.
    pub next_click: Option<String>,
    /// Whether to include the Preview button in the toolbar.
    /// Default: `true`.
    pub include_preview_button: Option<bool>,
}
