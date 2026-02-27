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
mod xml_writer;

pub use converter::convert_to_aem;
pub use package_writer::{collect_languages, detect_master_language, generate_aem_package};
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

    // -- Metadata (populated from XFA context) --------------------------------
    /// When `true`, the preview panel (with metadata, message boxes, carousel,
    /// etc.) is emitted as the last page in the root panel items.
    /// Automatically set to `true` by `populate_from_context()`.
    pub include_preview_panel: bool,

    /// FormRange entity code (e.g. `"019"`).
    pub metadata_entity: String,
    /// FormRange CDOK info (e.g. `"61137"`).
    pub metadata_cdokinfo: String,
    /// FormRange release date (e.g. `"31.10.2019"`).
    pub metadata_releasedate: String,
    /// FormRange version (e.g. `"V0"`).
    pub metadata_version: String,
    /// FormRange partner level (e.g. `"false"`).
    pub metadata_partnerlevel: String,
    /// FormRange CLP mandatory flag (e.g. `"false"`).
    pub metadata_clpmandatory: String,
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
    /// | `formrange_entity`   | `metadata_entity` / `form_path` |
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

            include_preview_panel: true,
            metadata_entity: entity_code,
            metadata_cdokinfo: ctx
                .get_variable("formrange_cdokinfo")
                .or_else(|| ctx.get_variable("Footer_Line_txtformid"))
                .unwrap_or("")
                .to_string(),
            metadata_releasedate: ctx
                .get_variable("formrange_releasedate")
                .or_else(|| ctx.get_variable("Footer_Line_txtversiondate"))
                .unwrap_or("")
                .to_string(),
            metadata_version: ctx
                .get_variable("formrange_version")
                .unwrap_or("")
                .to_string(),
            metadata_partnerlevel: ctx
                .get_variable("formrange_partnerlevel")
                .unwrap_or("false")
                .to_string(),
            metadata_clpmandatory: ctx
                .get_variable("formrange_clpmandatory")
                .unwrap_or("false")
                .to_string(),
        };

        config.dor_template_ref = config.compute_dor_template_ref();

        Ok(config)
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
    /// When a custom resource type base is configured, returns the
    /// component-specific widget class that matches the AEM component
    /// library (e.g. `widget_ajila-forms-ubs-textbox`).
    /// Otherwise falls back to `"{css_prefix}{component}"`.
    pub fn css_class(&self, component: &str) -> String {
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

    /// The JCR folder name for this form: `"AF_" + form_code`.
    ///
    /// Matches the Java convention where the terminal folder is
    /// `AF_AAAI`, `AF_AAEI`, etc.
    pub fn form_dir(&self) -> String {
        format!("AF_{}", self.form_code)
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

            include_preview_panel: false,
            metadata_entity: entity_code.into(),
            metadata_cdokinfo: String::new(),
            metadata_releasedate: String::new(),
            metadata_version: String::new(),
            metadata_partnerlevel: "false".into(),
            metadata_clpmandatory: "false".into(),
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
    },

    /// Numeric input (`guideNumberBox`).
    NumberField {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
    },

    /// Date picker (`guideDatePicker`).
    DatePicker {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
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
    },

    /// Title draw for h3–h6 headings (`guideTextDraw` with `headingLevel`).
    TitleDraw {
        uuid: Uuid,
        name: String,
        content: String,
        heading_level: u8,
        colspan: u32,
    },

    /// Multi-line text area (`guideTextBox` with `multiLine`).
    TextBoxMultiline {
        uuid: Uuid,
        name: String,
        label: String,
        mandatory: bool,
        visible: bool,
        colspan: u32,
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
