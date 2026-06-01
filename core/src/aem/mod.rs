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
pub mod fragment_parser;
mod package_writer;
pub mod parser;
pub mod profile;
pub mod script_engine;
pub mod template;
pub mod to_structured;
mod xml_writer;

pub use converter::convert_to_aem;
pub use fragment_parser::{ParsedFragment, parse_fragment_content, scan_fragments};
pub use package_writer::{collect_languages, generate_aem_package};
pub use parser::{
    AemScript, ParsedAemPackage, TranslationData, VisibilityCondition, detect_aem_zip,
    parse_aem_zip,
};
pub use profile::AemProfile;
pub use script_engine::AemScriptEngine;
pub use to_structured::aem_to_structured;
pub use xml_writer::generate_aem_xml;

use regex_lite::Regex;
use std::collections::HashMap;
use uuid::Uuid;

use crate::structured::{FieldId, InputValue};
use crate::xsd::XsdConfig;

// ============================================================================
// Configuration
// ============================================================================

/// A compiled custom element rule ready for matching.
#[derive(Debug, Clone)]
pub struct ResolvedCustomElement {
    /// Compiled regex pattern for matching element labels/titles.
    pub pattern: Regex,
    /// Name of the custom template to use.
    pub template: String,
    /// Optional target page index for moving the element.
    pub page: Option<i32>,
    /// Templates this rule depends on; the rule is skipped unless every
    /// listed template is also matched somewhere in the form.
    pub depends_on: Vec<String>,
}

/// Configuration for AEM Forms XML generation.
///
/// Created from an [`AemProfile`] directory.  All template-rendered output
/// is driven by Tera template files loaded from the profile directory.
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

    // -- Authoring metadata --------------------------------------------------
    /// Value for `jcr:createdBy` / `jcr:lastModifiedBy`.
    /// Default: `"blueprint"`.
    pub author: String,

    // -- Operational ---------------------------------------------------------
    /// When `true`, UUIDs are derived deterministically from the node name
    /// (UUID v5 with a fixed namespace), making the output reproducible
    /// across runs.
    pub deterministic_uuids: bool,

    /// Total number of grid columns (used by the converter).  Default: `12`.
    pub grid_columns: u32,

    /// Default minimum occurrences for repeatable panels.  Default: `1`.
    pub repeatable_min_occur: u32,

    /// Default maximum occurrences for repeatable panels.  Default: `20`.
    pub repeatable_max_occur: u32,

    // -- Package paths -------------------------------------------------------
    /// JCR path segment between `content/forms/af/` and the form directory.
    pub form_path: String,

    /// JCR folder name for this form (from the profile's `form_dir` template).
    pub form_dir: String,

    /// JCR file path of the generated XSD used in DAM metadata `xsdRef`
    /// (e.g. `/content/dam/formsanddocuments/.../AF_AAAI.xsd`).
    /// `None` when the profile does not specify an `xsd_path`.
    pub xsd_path: Option<String>,

    // -- Package writer metadata (derived from profile variables) -------------
    /// DOR template reference path (from `variables.dor_template_ref`).
    pub dor_template_ref: String,

    /// DOR generation type.  Default: `"generate"`.
    pub dor_type: String,

    /// Path to the theme client library (from `variables.theme_ref`).
    pub theme_ref: String,

    // -- Template-based XML generation ---------------------------------------
    /// Tera template strings keyed by component name
    /// (e.g. `"root"`, `"panel"`, `"textbox"`, …).
    /// Loaded from `*.xml` files in the profile directory.
    pub component_templates: HashMap<String, String>,

    // -- Variables (available in Tera templates) ------------------------------
    /// Raw XFA variables extracted from the PDF (`xfa.*` in templates).
    pub xfa_vars: HashMap<String, String>,

    /// Resolved user-defined variables (`variables.*` in templates).
    pub user_vars: HashMap<String, String>,

    // -- XSD binding ---------------------------------------------------------
    /// When `true`, `bindRef` attributes are added to all form fields/panels
    /// pointing to the matching element path in the generated XSD schema.
    /// The XSD is also bundled into the AEM content package.
    pub bind_to_xsd: bool,

    /// XSD configuration used both for schema generation and for computing
    /// `bindRef` paths.  Must be `Some` when `bind_to_xsd` is `true`.
    pub xsd_config: Option<XsdConfig>,

    // -- Fragment support ----------------------------------------------------
    /// When `true`, panels whose XSD type matches a known fragment are
    /// replaced by `AemNode::Fragment` references.
    pub use_fragments: bool,

    /// JCR path prefix for constructing fragment `fragRef` values.
    pub fragment_ref_prefix: String,

    /// Optional list of fragment paths (relative to `fragments/`) to scan.
    /// Each path can be a directory (scanned recursively) or a specific fragment.
    /// When empty, all subdirectories of `fragments/` are scanned.
    pub fragment_paths: Vec<String>,

    /// Parsed fragments loaded from the `fragments/` subdirectory.
    pub fragments: Vec<ParsedFragment>,

    /// Default translations for predefined UI elements (toolbar buttons,
    /// message boxes, etc.) that are not part of the form content tree.
    ///
    /// Merged into the Sling i18n dictionaries at package generation time.
    /// Form-content translations take precedence over defaults.
    pub default_translations: HashMap<String, HashMap<String, String>>,

    // -- Custom elements -----------------------------------------------------
    /// Compiled custom element replacement rules.
    pub custom_elements: Vec<ResolvedCustomElement>,

    /// Custom element templates loaded from the `custom/` subdirectory.
    /// Key = template name (file stem), Value = Tera template string.
    pub custom_templates: HashMap<String, String>,
}

impl AemConfig {
    /// Create an `AemConfig` from an [`AemProfile`], component templates,
    /// and a [`Context`](crate::Context).
    ///
    /// The profile provides customer-specific settings. XFA variables from
    /// the context are available in template expressions as `xfa.*`.
    /// Component templates (loaded from `*.xml` files in the profile directory)
    /// drive all XML output.
    pub fn from_profile(
        profile: &AemProfile,
        templates: HashMap<String, String>,
        custom_templates: HashMap<String, String>,
        ctx: &crate::Context,
    ) -> Result<Self, crate::Error> {
        let xfa_vars = ctx.variables.clone();
        let user_vars = template::resolve_variables(&profile.variables, &xfa_vars)?;
        let tera_ctx = template::build_context(&xfa_vars, &user_vars);

        // --- form identity ---
        let form_title = template::render_string(&profile.title, &tera_ctx)?;

        let form_code = form_title.clone();

        let form_path = match &profile.form_path {
            Some(tmpl) => template::render_string(tmpl, &tera_ctx)?,
            None => String::new(),
        };

        let form_dir = template::render_string(&profile.form_dir, &tera_ctx)?;

        let bind_to_xsd = profile.bind_to_xsd.unwrap_or(false);
        let xsd_path = match &profile.xsd_path {
            Some(tmpl) => Some(template::render_string(tmpl, &tera_ctx)?),
            None => None,
        };

        Ok(Self {
            form_title,
            form_code,
            languages: vec!["en".into()],
            master_language: profile
                .master_language
                .clone()
                .unwrap_or_else(|| "en".into()),
            language_synonyms: profile.language_synonyms.clone(),

            author: "blueprint".into(),
            deterministic_uuids: false,
            grid_columns: 12,
            repeatable_min_occur: 1,
            repeatable_max_occur: 20,

            form_path,
            form_dir,
            xsd_path,

            dor_template_ref: user_vars
                .get("dor_template_ref")
                .cloned()
                .unwrap_or_default(),
            dor_type: user_vars
                .get("dor_type")
                .cloned()
                .unwrap_or_else(|| "generate".into()),
            theme_ref: user_vars.get("theme_ref").cloned().unwrap_or_default(),

            component_templates: templates,
            xfa_vars,
            user_vars,

            bind_to_xsd,
            xsd_config: None,

            use_fragments: profile.use_fragments.unwrap_or(false),
            fragment_ref_prefix: profile
                .fragment_ref_prefix
                .clone()
                .unwrap_or_else(|| "/content/dam/formsanddocuments/".into()),
            fragment_paths: match &profile.fragment_paths {
                Some(tmpl) => {
                    let rendered = template::render_string(tmpl, &tera_ctx)?;
                    rendered
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                None => Vec::new(),
            },
            fragments: Vec::new(),

            default_translations: profile.default_translations.clone(),

            custom_elements: profile
                .custom_elements
                .iter()
                .map(|rule| {
                    let pattern = Regex::new(&rule.field_name).map_err(|e| {
                        crate::Error::AemConfig(format!(
                            "Invalid regex in custom_elements.field_name '{}': {}",
                            rule.field_name, e
                        ))
                    })?;
                    Ok(ResolvedCustomElement {
                        pattern,
                        template: rule.template.clone(),
                        page: rule.page,
                        depends_on: rule.depends_on.clone(),
                    })
                })
                .collect::<Result<Vec<_>, crate::Error>>()?,
            custom_templates,
        })
    }

    /// The JCR folder name for this form (from the profile's `form_dir` template).
    pub fn form_dir(&self) -> String {
        self.form_dir.clone()
    }

    /// Return the canonical DAM XSD reference path for metadata attributes.
    /// Returns `None` when `xsd_path` is not configured.
    pub fn xsd_ref(&self) -> Option<String> {
        self.xsd_path.as_ref().map(|p| {
            if p.starts_with('/') {
                p.clone()
            } else {
                format!("/{}", p)
            }
        })
    }

    /// Return the ZIP entry path where the XSD file should be written.
    /// Returns `None` when `xsd_path` is not configured.
    pub fn xsd_zip_path(&self) -> Option<String> {
        self.xsd_ref()
            .map(|r| format!("jcr_root/{}", r.trim_start_matches('/')))
    }

    /// Expand `languages` to include synonyms.
    ///
    /// For each language in `self.languages`, if it has synonyms defined in
    /// `language_synonyms`, those are added to the result. The result is
    /// sorted alphabetically.
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

#[cfg(test)]
impl AemConfig {
    /// Create an `AemConfig` for testing without requiring a real `Context`.
    ///
    /// The caller provides the form code directly.
    /// All other fields use sensible defaults.
    pub fn test_default(form_code: &str) -> Self {
        Self {
            form_title: form_code.into(),
            form_code: form_code.into(),
            languages: vec!["en".into()],
            master_language: "en".into(),
            language_synonyms: {
                let mut map = HashMap::new();
                map.insert("de".into(), vec!["de-ch".into()]);
                map.insert("sp".into(), vec!["es".into()]);
                map
            },

            author: "blueprint".into(),
            deterministic_uuids: false,
            grid_columns: 12,
            repeatable_min_occur: 1,
            repeatable_max_occur: 20,

            form_path: "test/path".into(),
            form_dir: format!("AF_{form_code}"),
            xsd_path: Some("/content/dam/formsanddocuments/test/path/AF_TEST/schema.xsd".into()),

            dor_template_ref: String::new(),
            dor_type: "generate".into(),
            theme_ref: String::new(),

            component_templates: HashMap::new(),
            xfa_vars: HashMap::new(),
            user_vars: HashMap::new(),

            bind_to_xsd: false,
            xsd_config: None,

            use_fragments: false,
            fragment_ref_prefix: "/content/dam/formsanddocuments/".into(),
            fragment_paths: Vec::new(),
            fragments: Vec::new(),

            default_translations: HashMap::new(),

            custom_elements: Vec::new(),
            custom_templates: HashMap::new(),
        }
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
        /// Whether this panel wraps a conditional branch.
        is_conditional: bool,
        /// Number of columns for Document of Record layout (`dorNumCols`).
        /// Derived from `GridLayout.columns`. `None` means no `dorNumCols` attribute.
        dor_num_cols: Option<u32>,
        /// Adaptive-form responsive column width (out of 12).
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        /// Set on elements that are children of a `GridLayout` panel.
        dor_colspan: Option<u32>,
        /// XSD path for `bindRef` attribute (e.g. `/form/personal_data`).
        /// `None` when `bind_to_xsd` is `false` or the panel has no corresponding XSD element.
        bind_ref: Option<String>,
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
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
    },

    /// Numeric input (`guideNumericBox`).
    NumberField {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
        /// Column span in Document of Record layout (`dorColspan`).
        dor_colspan: Option<u32>,
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
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
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
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
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
    },

    /// Checkbox group (`guideCheckBox`).
    Checkbox {
        uuid: Uuid,
        name: String,
        /// Group label from `jcr:title` (empty for single-option Bool checkboxes).
        label: String,
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
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
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
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
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

    /// Repeatable panel with add/remove buttons.
    Repeatable {
        uuid: Uuid,
        name: String,
        title: String,
        children: Vec<AemNode>,
        min_occur: u32,
        max_occur: u32,
        /// XSD path for `bindRef` attribute on the repeatable inner panel.
        bind_ref: Option<String>,
    },

    /// Fragment reference — replaces a panel whose XSD type matches a
    /// known fragment. The fragment's internal structure is loaded by AEM
    /// at runtime from the `fragRef` path.
    Fragment {
        uuid: Uuid,
        name: String,
        /// JCR path to the fragment (e.g.
        /// `"/content/dam/formsanddocuments/afforms_ubs_fragmentlib/affrg_Address1"`).
        frag_ref: String,
        /// XSD path for `bindRef` attribute.
        bind_ref: Option<String>,
    },

    /// Optional profile-driven snippet inserted as the first item in the
    /// first page panel when the `preface` template exists.
    Preface { uuid: Uuid, name: String },

    /// Optional profile-driven snippet inserted as the last item in the
    /// last page panel when the `appendix` template exists.
    Appendix { uuid: Uuid, name: String },

    /// Footnote placeholder component, placed at the end of a page panel
    /// that contains inline footnote references. AEM renders collected
    /// footnotes at this location.
    FootnotePlaceholder {
        uuid: Uuid,
        name: String,
        colspan: u32,
    },

    /// Custom element — replaces a matched field/panel with a custom template.
    /// Created by the `apply_custom_elements()` pass when a node's label/title
    /// matches a `[[custom_elements]]` rule.
    Custom {
        uuid: Uuid,
        name: String,
        /// The custom template key (file stem from `custom/` directory).
        template_key: String,
        /// The label or title of the matched element.
        label: String,
        /// Options from the original element (if it was a Dropdown/RadioButton/Checkbox).
        options: Vec<AemOption>,
        /// Whether the original element was mandatory.
        mandatory: bool,
        /// Whether the original element was visible.
        visible: bool,
        /// Column span of the original element.
        colspan: u32,
        /// DOR column span from the original element.
        dor_colspan: Option<u32>,
        /// Bind ref from the original element.
        bind_ref: Option<String>,
    },
}

// ============================================================================
// Helpers
// ============================================================================

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
            AemNode::Repeatable { uuid, .. } => format!("repeatable_{}", uuid.as_simple()),
            AemNode::Fragment { uuid, .. } => format!("fragment_{}", uuid.as_simple()),
            AemNode::Preface { uuid, .. } => format!("preface_{}", uuid.as_simple()),
            AemNode::Appendix { uuid, .. } => format!("appendix_{}", uuid.as_simple()),
            AemNode::FootnotePlaceholder { uuid, .. } => {
                format!("guidefootnoteplaceho_{}", uuid.as_simple())
            }
            AemNode::Custom { uuid, name, .. } => {
                if name.is_empty() {
                    format!("custom_{}", uuid.as_simple())
                } else {
                    name.clone()
                }
            }
        }
    }
}
