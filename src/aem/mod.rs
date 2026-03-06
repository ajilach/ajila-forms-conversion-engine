//! AEM Forms XML Output Module
//!
//! Converts structured form nodes into Adobe Experience Manager (AEM) Adaptive
//! Forms JCR content XML. The module defines an intermediate `AemNode` tree
//! that captures the AEM-specific semantics, then serializes it to well-formed
//! XML using `quick-xml`.
//!
//! # Architecture
//!
//! ```text
//! StructuredNode ──► convert_to_aem() ──► AemNode tree ──► generate_aem_xml() ──► XML String
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use blueprint::aem::{AemConfig, convert_to_aem, generate_aem_xml};
//!
//! let config = AemConfig::new(&context)?;
//! let root = convert_to_aem(&structured_nodes, &config);
//! let xml = generate_aem_xml(&root, &config);
//! ```

mod converter;
mod package_writer;
pub mod profile;
pub mod template;
mod xml_writer;

pub use converter::convert_to_aem;
pub use package_writer::{collect_languages, detect_master_language, generate_aem_package};
pub use profile::AemProfile;
pub use xml_writer::generate_aem_xml;

use std::collections::HashMap;
use uuid::Uuid;

use crate::structured::{FieldId, InputValue};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for AEM Forms XML generation.
///
/// All fields have sensible defaults matching typical AEM Forms deployments.
/// Customize as needed for your AEM instance.
#[derive(Debug, Clone)]
pub struct AemConfig {
    // -- Form identity -------------------------------------------------------
    /// Human-readable form title (appears in `jcr:title`).
    pub form_title: String,

    /// Internal form code (used for metadata and paths).
    pub form_code: String,

    /// Available languages (ISO 639-1 codes, e.g. `["de", "en", "fr"]`).
    pub languages: Vec<String>,

    /// Master / primary language code.
    pub master_language: String,

    /// Language synonyms: maps a base language code to additional codes that
    /// should receive the same translations (e.g. `"de" → ["de-ch"]`,
    /// `"sp" → ["es"]`).
    pub language_synonyms: HashMap<String, Vec<String>>,

    // -- Resource types ------------------------------------------------------
    /// Base path for standard AEM Forms components.
    /// Default: `"fd/af/components"`.
    pub resource_type_base: String,

    /// Optional custom component base path (e.g. `"myproject/components"`).
    /// When set, field-level components use this base instead of
    /// `resource_type_base`.
    pub custom_resource_type_base: Option<String>,

    // -- Layout --------------------------------------------------------------
    /// Sling resource type for the default panel layout.
    /// Default: `"fd/af/layouts/gridFluidLayout2"`.
    pub default_layout: String,

    /// Total number of grid columns.
    /// Default: `12`.
    pub grid_columns: u32,

    /// Whether to enable layout optimisation on panels.
    /// Default: `true`.
    pub enable_layout_optimization: bool,

    // -- Document of Record (DOR) defaults -----------------------------------
    /// Default DOR field styling.
    /// Default: `"Default"`.
    pub dor_field_styling: String,

    /// Exclude description from DOR by default.
    /// Default: `true`.
    pub dor_exclude_description: bool,

    /// DOR generation type.
    /// Default: `"generate"`.
    pub dor_type: String,

    // -- Page chrome ---------------------------------------------------------
    /// Include the standard toolbar (prev/next/submit) in root output.
    /// Default: `true`.
    pub include_toolbar: bool,

    /// Wrap the output in the full JCR page structure (`jcr:root` /
    /// `jcr:content`).  When `false`, only the `rootPanel` subtree is emitted.
    /// Default: `true`.
    pub include_page_wrapper: bool,

    // -- CSS -----------------------------------------------------------------
    /// CSS class prefix prepended to widget classes (e.g. `"widget_"`).
    /// Default: `"widget_"`.
    pub css_prefix: String,

    // -- Authoring metadata --------------------------------------------------
    /// Value for `jcr:createdBy` / `jcr:lastModifiedBy`.
    /// Default: `"blueprint"`.
    pub author: String,

    // -- UUID generation -----------------------------------------------------
    /// When `true`, UUIDs are derived deterministically from the node name
    /// (UUID v5 with a fixed namespace), making the output reproducible
    /// across runs. When `false`, random UUID v4 values are used.
    /// Default: `false`.
    pub deterministic_uuids: bool,

    // -- Repeatable defaults -------------------------------------------------
    /// Default minimum occurrences for repeatable panels.
    /// Default: `1`.
    pub repeatable_min_occur: u32,

    /// Default maximum occurrences for repeatable panels.
    /// Default: `20`.
    pub repeatable_max_occur: u32,

    // -- Template references -------------------------------------------------
    /// Sling resource type for the page component (`jcr:content`).
    ///
    /// This is NOT the guide container resource type — it is the page-level
    /// rendering component that AEM uses to render the `cq:Page`.
    ///
    /// Default: `"fd/af/components/page2"`.
    pub page_resource_type: String,

    /// Path to the AEM template (used in `cq:template`).
    /// Default: `"/conf/ajila-forms-ubs/settings/wcm/templates/basic"`.
    pub template_path: String,

    /// Path to the theme client library.
    /// Default: `""` (empty — set to your theme path).
    pub theme_ref: String,

    /// DOR template reference path.
    /// Default: `""` (empty — set to your DOR template if needed).
    pub dor_template_ref: String,

    /// Redirect URL after form submission.
    /// Default: `"/content/forms/af/afforms_global_common/confirm-successful-submission"`.
    pub redirect_url: String,

    // -- Guide container extras -----------------------------------------------
    /// Action type for form submission.
    /// Default: `"ajila-forms-customers/ajila-forms-ubs/components/actions/submit"`.
    pub action_type: String,

    /// Client library reference.
    /// Default: `"ajila-forms-ubs"`.
    pub client_lib_ref: String,

    // -- Root panel wizard layout --------------------------------------------
    /// Sling resource type for the root panel wizard layout.
    /// Default: `"ajila-forms-customers/ajila-forms-ubs/layouts/panel/wizard"`.
    pub wizard_layout: String,

    // -- DOR branding --------------------------------------------------------
    /// Form type indicator for DOR branding (e.g. `" "` or `"K"`).
    /// Default: `" "`.
    pub form_type: String,

    /// DOR meta-template reference path.
    /// Default: `"/content/dam/formsanddocuments/reference-dor-templates/ajila-forms-ubs/02_forms/UBS_Blank_DoR.xdp"`.
    pub meta_template_ref: String,

    // -- Package paths -------------------------------------------------------
    /// JCR path segment between `content/forms/af/` (or
    /// `content/dam/formsanddocuments/`) and the form code.
    ///
    /// For example, `"ajila-forms-ubs/output/Germany_Tranch_1"` results in
    /// the form page being placed at
    /// `/content/forms/af/ajila-forms-ubs/output/Germany_Tranch_1/<form_code>`.
    ///
    /// Default: `"ajila-forms-ubs/output/Germany_Tranch_1"`.
    pub form_path: String,

    // -- Profile-based configuration -----------------------------------------
    /// Pre-rendered XML snippets for injection points.
    ///
    /// Supported keys: `print_branding`, `first_panel`, `last_panel`.
    /// Populated by `from_profile()` + `render_snippets()`.
    pub xml_snippets: HashMap<String, String>,

    /// Raw Tera templates for XML snippets (before language detection).
    /// Kept for deferred rendering in `render_snippets()`.
    snippet_templates: HashMap<String, String>,

    /// Base context entries (xfa.* + variables.*) for deferred snippet
    /// rendering.
    snippet_xfa_vars: HashMap<String, String>,
    snippet_user_vars: HashMap<String, String>,

    /// Per-component configuration from the profile.
    /// Keys match component names (e.g. `"textbox"`, `"textbox_multiline"`).
    pub component_config: HashMap<String, profile::ComponentConfig>,

    /// Optional script for the Next button's click handler.
    pub next_click_script: Option<String>,

    /// Whether to include the Preview button in the toolbar.
    pub include_preview_button: bool,

    /// Override for `form_dir()`. When set, `form_dir()` returns this value
    /// instead of computing `"AF_" + form_code`.
    pub form_dir_override: Option<String>,
}

impl AemConfig {
    /// Create a new `AemConfig` from a [`Context`](crate::Context).
    ///
    /// The context **must** contain the following XFA variables
    /// (extracted from `<variables><text>` in the XFA template):
    ///
    /// | Variable             | Maps to             |
    /// |----------------------|---------------------|
    /// | `formrange_code`     | `form_code` / `form_title` |
    /// | `formrange_entity`   | `form_path` |
    ///
    /// Returns an error if either required variable is missing.
    pub fn new(ctx: &crate::Context) -> Result<Self, crate::Error> {
        let form_code = ctx
            .get_variable("formrange_code")
            .ok_or_else(|| {
                crate::Error::AemConfig("missing required XFA variable 'formrange_code'".into())
            })?
            .to_string();

        let entity_code = ctx
            .get_variable("formrange_entity")
            .ok_or_else(|| {
                crate::Error::AemConfig("missing required XFA variable 'formrange_entity'".into())
            })?
            .to_string();

        // Derive form_path from entity code and form code prefix
        let entity_dir = entity_folder_name(&entity_code);
        let prefix_dir = format!(
            "af_{}",
            form_code.chars().take(2).collect::<String>().to_lowercase()
        );
        let form_path = format!("{}/{}", entity_dir, prefix_dir);

        let mut config = Self {
            form_title: form_code.clone(),
            form_code,
            languages: vec!["en".into()],
            master_language: "en".into(),
            language_synonyms: default_language_synonyms(),

            resource_type_base: "fd/af/components".into(),
            custom_resource_type_base: Some(
                "ajila-forms-customers/ajila-forms-ubs/components".into(),
            ),

            default_layout: "fd/af/layouts/gridFluidLayout2".into(),
            grid_columns: 12,
            enable_layout_optimization: true,

            dor_field_styling: "Default".into(),
            dor_exclude_description: true,
            dor_type: "generate".into(),

            include_toolbar: true,
            include_page_wrapper: true,

            css_prefix: "widget_".into(),
            author: "blueprint".into(),
            deterministic_uuids: false,

            repeatable_min_occur: 1,
            repeatable_max_occur: 20,

            page_resource_type:
                "/apps/ajila-forms-customers/ajila-forms-ubs/components/pages/aftemplatedpage"
                    .into(),
            template_path: "/conf/ajila-forms-ubs/settings/wcm/templates/basic".into(),
            theme_ref: "/content/dam/formsanddocuments-themes/ajila-forms-ubs/standard-theme"
                .into(),
            dor_template_ref: String::new(),
            redirect_url:
                "/content/forms/af/afforms_global_common/confirm-successful-submission".into(),

            action_type: "ajila-forms-customers/ajila-forms-ubs/components/actions/submit".into(),
            client_lib_ref: "ajila-forms-ubs".into(),
            wizard_layout: "ajila-forms-customers/ajila-forms-ubs/layouts/panel/wizard".into(),
            form_type: " ".into(),
            meta_template_ref: "/content/dam/formsanddocuments/reference-dor-templates/ajila-forms-ubs/02_forms/UBS_Blank_DoR.xdp".into(),

            form_path,

            xml_snippets: HashMap::new(),
            snippet_templates: HashMap::new(),
            snippet_xfa_vars: HashMap::new(),
            snippet_user_vars: HashMap::new(),
            component_config: HashMap::new(),
            next_click_script: None,
            include_preview_button: true,
            form_dir_override: None,
        };

        config.dor_template_ref = config.compute_dor_template_ref();

        Ok(config)
    }

    /// Create an `AemConfig` from an [`AemProfile`] and a [`Context`](crate::Context).
    ///
    /// The profile provides all customer-specific settings. XFA variables from
    /// the context are available in template expressions as `xfa.*`.
    ///
    /// XML snippets are stored as raw Tera templates; call
    /// [`render_snippets()`](Self::render_snippets) after language detection
    /// to render them with `master_language` / `languages` context.
    pub fn from_profile(
        profile: &AemProfile,
        ctx: &crate::Context,
    ) -> Result<Self, crate::Error> {
        let xfa_vars = ctx.variables.clone();
        let user_vars = template::resolve_variables(&profile.variables, &xfa_vars)?;
        let tera_ctx = template::build_context(&xfa_vars, &user_vars);

        // --- form identity ---
        let form_code = ctx
            .get_variable("formrange_code")
            .ok_or_else(|| {
                crate::Error::AemConfig("missing required XFA variable 'formrange_code'".into())
            })?
            .to_string();

        let form_title = match &profile.title {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => form_code.clone(),
        };

        let form_path = match &profile.form_path {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => String::new(),
        };

        let form_dir_override = match &profile.form_dir {
            Some(tmpl) => Some(template::render_string(tmpl, &tera_ctx)?),
            None => None,
        };

        // --- paths (render templates) ---
        let template_path = match &profile.paths.template_path {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => String::new(),
        };
        let page_resource_type = match &profile.paths.page_resource_type {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => "fd/af/components/page2".into(),
        };
        let theme_ref = match &profile.paths.theme_ref {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => String::new(),
        };
        let redirect_url = match &profile.paths.redirect_url {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => String::new(),
        };
        let meta_template_ref = match &profile.paths.meta_template_ref {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => String::new(),
        };

        // --- components ---
        let resource_type_base = profile
            .components
            .resource_type_base
            .clone()
            .unwrap_or_else(|| "fd/af/components".into());
        let custom_resource_type_base = profile.components.custom_resource_type_base.clone();
        let action_type = profile
            .components
            .action_type
            .clone()
            .unwrap_or_else(|| String::new());
        let client_lib_ref = profile
            .components
            .client_lib_ref
            .clone()
            .unwrap_or_else(|| String::new());
        let wizard_layout = profile
            .components
            .wizard_layout
            .clone()
            .unwrap_or_else(|| "fd/af/layouts/panel/wizard".into());
        let form_type = profile
            .components
            .form_type
            .clone()
            .unwrap_or_else(|| " ".into());

        // --- build config ---
        let mut config = Self {
            form_title,
            form_code,
            languages: vec!["en".into()],
            master_language: "en".into(),
            language_synonyms: profile.language_synonyms.clone(),

            resource_type_base,
            custom_resource_type_base,

            default_layout: "fd/af/layouts/gridFluidLayout2".into(),
            grid_columns: 12,
            enable_layout_optimization: true,

            dor_field_styling: "Default".into(),
            dor_exclude_description: true,
            dor_type: "generate".into(),

            include_toolbar: true,
            include_page_wrapper: true,

            css_prefix: "widget_".into(),
            author: "blueprint".into(),
            deterministic_uuids: false,

            repeatable_min_occur: 1,
            repeatable_max_occur: 20,

            page_resource_type,
            template_path,
            theme_ref,
            dor_template_ref: String::new(),
            redirect_url,

            action_type,
            client_lib_ref,
            wizard_layout,
            form_type,
            meta_template_ref,

            form_path,

            xml_snippets: HashMap::new(),
            snippet_templates: profile.xml_snippets.clone(),
            snippet_xfa_vars: xfa_vars,
            snippet_user_vars: user_vars,
            component_config: profile.components.component_config.clone(),
            next_click_script: profile.scripts.next_click.clone(),
            include_preview_button: profile.scripts.include_preview_button.unwrap_or(true),
            form_dir_override,
        };

        // Render dor_template_ref (may reference variables.*)
        config.dor_template_ref = match &profile.paths.dor_template_ref {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => config.compute_dor_template_ref(),
        };

        Ok(config)
    }

    /// Render deferred XML snippets with the current `master_language` and
    /// `languages` values.
    ///
    /// Call this after language detection has populated those fields
    /// (typically via [`resolve_aem_languages()`](crate::resolve_aem_languages)).
    pub fn render_snippets(&mut self) {
        if self.snippet_templates.is_empty() {
            return;
        }

        let mut ctx = template::build_context(&self.snippet_xfa_vars, &self.snippet_user_vars);
        ctx.insert("master_language", &self.master_language);
        ctx.insert("languages", &self.languages.join(","));
        ctx.insert("form_code", &self.form_code);
        ctx.insert("author", &self.author);

        self.xml_snippets.clear();
        for (name, tmpl) in &self.snippet_templates {
            match template::render_string(tmpl, &ctx) {
                Ok(rendered) => {
                    self.xml_snippets.insert(name.clone(), rendered);
                }
                Err(e) => {
                    log::warn!("Failed to render XML snippet '{}': {}", name, e);
                }
            }
        }
    }

    /// Resolve the sling resource type for a control component.
    ///
    /// If `custom_resource_type_base` is set, produces
    /// `"{custom_base}/controls/{component}"`.
    /// Otherwise falls back to
    /// `"{resource_type_base}/controls/{component}"`.
    pub fn control_resource_type(&self, component: &str) -> String {
        let base = self
            .custom_resource_type_base
            .as_deref()
            .unwrap_or(&self.resource_type_base);
        format!("{}/controls/{}", base, component)
    }

    /// Resolve the sling resource type for a panel.
    pub fn panel_resource_type(&self) -> String {
        format!("{}/panel", self.resource_type_base)
    }

    /// Resolve the sling resource type for the guide container.
    pub fn guide_container_resource_type(&self) -> String {
        format!("{}/guideContainer", self.resource_type_base)
    }

    /// CSS class for a control component.
    ///
    /// Checks `component_config` first (populated from the profile's
    /// per-component sections). Falls back to the hardcoded UBS
    /// widget classes when a custom resource type base is configured,
    /// or `"{css_prefix}{component}"` otherwise.
    pub fn css_class(&self, component: &str) -> String {
        // Profile override takes precedence
        if let Some(cfg) = self.component_config.get(component) {
            if let Some(ref css) = cfg.css {
                return css.clone();
            }
        }
        // Legacy fallback
        if self.custom_resource_type_base.is_some() {
            match component {
                "textbox" => "widget_ajila-forms-ubs-textbox".into(),
                "numericbox" => "widget_ajila-forms-ubs-numericbox".into(),
                "datepicker" => "widget_ajila_forms_datepicker".into(),
                "checkbox" => "widget_ajila_forms_checkbox".into(),
                "radiobutton" => "widget_ajila_forms_radiobutton".into(),
                "dropdownlist" => "widget_ajila_forms_dropdownlist".into(),
                "primarybutton" => "widget_ajila-forms-ubs-primarybutton".into(),
                _ => format!("{}{}", self.css_prefix, component),
            }
        } else {
            format!("{}{}", self.css_prefix, component)
        }
    }

    /// The JCR folder name for this form.
    ///
    /// Uses `form_dir_override` if set (from profile's `form_dir` template),
    /// otherwise computes `"AF_" + form_code`.
    pub fn form_dir(&self) -> String {
        if let Some(ref dir) = self.form_dir_override {
            dir.clone()
        } else {
            format!("AF_{}", self.form_code)
        }
    }

    /// Compute the DOR template ref from `form_path` and `form_dir()`.
    ///
    /// Produces a path like:
    /// `/content/dam/formsanddocuments/{form_path}/{form_dir}/jcr:content/renditions/dorTemplate`
    pub fn compute_dor_template_ref(&self) -> String {
        format!(
            "/content/dam/formsanddocuments/{}/{}/jcr:content/renditions/dorTemplate",
            self.form_path,
            self.form_dir()
        )
    }

    /// Expand `languages` to include synonyms.
    ///
    /// For each language in `self.languages`, if it has synonyms defined in
    /// `language_synonyms`, those are added to the result. The result is
    /// sorted alphabetically.
    ///
    /// Example: `["de", "en"]` with synonym `"de" → ["de-ch"]` produces
    /// `["de", "de-ch", "en"]`.
    pub fn expand_languages(&self) -> Vec<String> {
        let mut expanded = self.languages.clone();
        for lang in &self.languages {
            if let Some(synonyms) = self.language_synonyms.get(lang) {
                for syn in synonyms {
                    if !expanded.contains(syn) {
                        expanded.push(syn.clone());
                    }
                }
            }
        }
        expanded.sort();
        expanded
    }
}

/// Returns the default language synonym mappings.
///
/// - `"de"` → `["de-ch"]` (Swiss German)
/// - `"sp"` → `["es"]` (Spanish)
fn default_language_synonyms() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("de".into(), vec!["de-ch".into()]);
    map.insert("sp".into(), vec!["es".into()]);
    map
}

#[cfg(test)]
impl AemConfig {
    /// Create an `AemConfig` for testing without requiring a real `Context`.
    ///
    /// The caller provides the form code and entity code directly.
    /// All other fields use the same hard-coded values as [`AemConfig::new`].
    pub fn test_default(form_code: &str, entity_code: &str) -> Self {
        let entity_dir = entity_folder_name(entity_code);
        let prefix_dir = format!(
            "af_{}",
            form_code.chars().take(2).collect::<String>().to_lowercase()
        );
        let form_path = format!("{}/{}", entity_dir, prefix_dir);

        let mut config = Self {
            form_title: form_code.into(),
            form_code: form_code.into(),
            languages: vec!["en".into()],
            master_language: "en".into(),
            language_synonyms: default_language_synonyms(),

            resource_type_base: "fd/af/components".into(),
            custom_resource_type_base: Some(
                "ajila-forms-customers/ajila-forms-ubs/components".into(),
            ),

            default_layout: "fd/af/layouts/gridFluidLayout2".into(),
            grid_columns: 12,
            enable_layout_optimization: true,

            dor_field_styling: "Default".into(),
            dor_exclude_description: true,
            dor_type: "generate".into(),

            include_toolbar: true,
            include_page_wrapper: true,

            css_prefix: "widget_".into(),
            author: "blueprint".into(),
            deterministic_uuids: false,

            repeatable_min_occur: 1,
            repeatable_max_occur: 20,

            page_resource_type:
                "/apps/ajila-forms-customers/ajila-forms-ubs/components/pages/aftemplatedpage"
                    .into(),
            template_path: "/conf/ajila-forms-ubs/settings/wcm/templates/basic".into(),
            theme_ref: "/content/dam/formsanddocuments-themes/ajila-forms-ubs/standard-theme"
                .into(),
            dor_template_ref: String::new(),
            redirect_url:
                "/content/forms/af/afforms_global_common/confirm-successful-submission".into(),

            action_type: "ajila-forms-customers/ajila-forms-ubs/components/actions/submit".into(),
            client_lib_ref: "ajila-forms-ubs".into(),
            wizard_layout: "ajila-forms-customers/ajila-forms-ubs/layouts/panel/wizard".into(),
            form_type: " ".into(),
            meta_template_ref: "/content/dam/formsanddocuments/reference-dor-templates/ajila-forms-ubs/02_forms/UBS_Blank_DoR.xdp".into(),

            form_path,

            xml_snippets: HashMap::new(),
            snippet_templates: HashMap::new(),
            snippet_xfa_vars: HashMap::new(),
            snippet_user_vars: HashMap::new(),
            component_config: HashMap::new(),
            next_click_script: None,
            include_preview_button: true,
            form_dir_override: None,
        };

        config.dor_template_ref = config.compute_dor_template_ref();
        config
    }
}

// ============================================================================
// AEM Node Types
// ============================================================================

/// Alignment of options in checkbox / radio button groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionAlignment {
    Horizontal,
    Vertical,
}

/// A single option in a checkbox or radio button group.
#[derive(Debug, Clone)]
pub struct AemOption {
    /// Display label (may contain rich text HTML).
    pub label: String,
    /// Form value submitted when this option is selected.
    pub value: String,
}

/// A visibility condition rule that links a trigger field value to a
/// conditional panel.
///
/// When the trigger field's value matches `value`, the target panel's
/// visibility is set to the `show` value.
#[derive(Debug, Clone)]
pub struct ConditionRule {
    /// AEM `name` of the conditional panel to show/hide.
    pub target_panel_name: String,
    /// The value that, when matched, triggers the show/hide.
    pub value: InputValue,
    /// `true` → show panel when matched; `false` → hide.
    pub show: bool,
}

/// The intermediate AEM node tree.
///
/// Each variant maps to a specific AEM Adaptive Forms component and carries
/// all the data needed for XML serialization.
#[derive(Debug, Clone)]
pub enum AemNode {
    /// Top-level form container — produces the full JCR page structure.
    Root {
        title: String,
        children: Vec<AemNode>,
    },

    /// Generic panel container (`guidePanel`).
    Panel {
        uuid: Uuid,
        name: String,
        title: String,
        children: Vec<AemNode>,
        /// Whether this panel represents a page/wizard step.
        is_page: bool,
        /// Exclude from Document of Record.
        dor_exclude: bool,
        /// Whether the panel is visible. Default `true`.
        visible: bool,
        /// Number of columns for Document of Record layout (`dorNumCols`).
        /// Derived from `GridLayout.columns`. `None` means no `dorNumCols` attribute.
        dor_num_cols: Option<u32>,
        /// Adaptive-form responsive column width (out of 12).
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        /// Set on elements that are children of a `GridLayout` panel.
        dor_colspan: Option<u32>,
    },

    /// Single-line text input (`guideTextBox`).
    TextField {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        max_chars: Option<usize>,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
    },

    /// Numeric input (`guideNumberBox`).
    NumberField {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
    },

    /// Date picker (`guideDatePicker`).
    DatePicker {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
    },

    /// Drop-down / select list (`guideDropDownList`).
    Dropdown {
        uuid: Uuid,
        name: String,
        label: String,
        options: Vec<AemOption>,
        mandatory: bool,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
        /// The `FieldId` of the original structured field (for condition wiring).
        field_id: Option<FieldId>,
        /// Visibility condition rules populated during the second pass.
        conditions: Vec<ConditionRule>,
    },

    /// Checkbox group (`guideCheckBox`).
    Checkbox {
        uuid: Uuid,
        name: String,
        options: Vec<AemOption>,
        alignment: OptionAlignment,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
        /// The `FieldId` of the original structured field (for condition wiring).
        field_id: Option<FieldId>,
        /// Visibility condition rules populated during the second pass.
        conditions: Vec<ConditionRule>,
    },

    /// Radio button group (`guideRadioButton`).
    RadioButton {
        uuid: Uuid,
        name: String,
        label: String,
        options: Vec<AemOption>,
        alignment: OptionAlignment,
        mandatory: bool,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
        /// The `FieldId` of the original structured field (for condition wiring).
        field_id: Option<FieldId>,
        /// Visibility condition rules populated during the second pass.
        conditions: Vec<ConditionRule>,
    },

    /// Static text / heading (`guideTextDraw`).
    TextDraw {
        uuid: Uuid,
        name: String,
        content: String,
        dor_exclude: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
    },

    /// Title draw for h3–h6 headings (`guideTextDraw` with `headingLevel`).
    TitleDraw {
        uuid: Uuid,
        name: String,
        content: String,
        heading_level: u8,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
    },

    /// Multi-line text area (`guideTextBox` with `multiLine`).
    TextBoxMultiline {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
    },

    /// Repeatable panel with add/remove buttons.
    Repeatable {
        uuid: Uuid,
        name: String,
        title: String,
        children: Vec<AemNode>,
        min_occur: u32,
        max_occur: u32,
    },
}

// ============================================================================
// Helpers
// ============================================================================

/// Map an entity code (the second segment of the PDF filename) to the AEM
/// folder name, mirroring the Java `getEntityFolderName` method.
fn entity_folder_name(entity_code: &str) -> &'static str {
    match entity_code {
        "019" => "afforms_germany_all",
        "033" => "afforms_italy_all",
        "001" => "afforms_ch_all",
        _ => "afforms_global_all",
    }
}

impl AemNode {
    /// Get the element tag name used in JCR XML (e.g. `"panel_<uuid>"`).
    pub fn element_name(&self) -> String {
        match self {
            AemNode::Root { .. } => "jcr:root".into(),
            AemNode::Panel { uuid, .. } => format!("panel_{}", uuid.as_simple()),
            AemNode::TextField { uuid, .. } => format!("textbox_{}", uuid.as_simple()),
            AemNode::NumberField { uuid, .. } => format!("numericbox_{}", uuid.as_simple()),
            AemNode::DatePicker { uuid, .. } => format!("datepicker_{}", uuid.as_simple()),
            AemNode::Dropdown { uuid, .. } => format!("dropdownlist_{}", uuid.as_simple()),
            AemNode::Checkbox { uuid, .. } => format!("checkbox_{}", uuid.as_simple()),
            AemNode::RadioButton { uuid, .. } => format!("radiobutton_{}", uuid.as_simple()),
            AemNode::TextDraw { uuid, .. } => format!("textdraw_{}", uuid.as_simple()),
            AemNode::TitleDraw { uuid, .. } => format!("titledraw_{}", uuid.as_simple()),
            AemNode::TextBoxMultiline { uuid, .. } => {
                format!("textboxmultiline_{}", uuid.as_simple())
            }
            AemNode::Repeatable { uuid, .. } => format!("repeatable_{}", uuid.as_simple()),
        }
    }
}
