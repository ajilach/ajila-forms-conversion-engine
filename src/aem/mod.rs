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
//! let config = AemConfig {
//!     form_title: "My Form".into(),
//!     form_code: "MYFORM_001".into(),
//!     ..Default::default()
//! };
//!
//! let root = convert_to_aem(&structured_nodes, &config);
//! let xml = generate_aem_xml(&root, &config);
//! ```

mod converter;
mod package_writer;
mod xml_writer;

pub use converter::convert_to_aem;
pub use package_writer::{collect_languages, detect_master_language, generate_aem_package};
pub use xml_writer::generate_aem_xml;

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

impl Default for AemConfig {
    fn default() -> Self {
        Self {
            form_title: "Untitled Form".into(),
            form_code: "FORM_001".into(),
            languages: vec!["en".into()],
            master_language: "en".into(),

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
            theme_ref: "/content/dam/formsanddocuments-themes/ajila-forms-ubs/standard-theme".into(),
            dor_template_ref: String::new(),
            redirect_url: "/content/forms/af/afforms_global_common/confirm-successful-submission"
                .into(),

            action_type: "ajila-forms-customers/ajila-forms-ubs/components/actions/submit".into(),
            client_lib_ref: "ajila-forms-ubs".into(),
            wizard_layout: "ajila-forms-customers/ajila-forms-ubs/layouts/panel/wizard".into(),
            form_type: " ".into(),
            meta_template_ref: "/content/dam/formsanddocuments/reference-dor-templates/ajila-forms-ubs/02_forms/UBS_Blank_DoR.xdp".into(),

            form_path: "ajila-forms-ubs/output/Germany_Tranch_1".into(),

            include_preview_panel: false,
            metadata_entity: String::new(),
            metadata_cdokinfo: String::new(),
            metadata_releasedate: String::new(),
            metadata_version: String::new(),
            metadata_partnerlevel: "false".into(),
            metadata_clpmandatory: "false".into(),
        }
    }
}

impl AemConfig {
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

    /// Compute the DOR template ref from `form_path` and `form_code`.
    ///
    /// Produces a path like:
    /// `/content/dam/formsanddocuments/{form_path}/{form_code}/jcr:content/renditions/dorTemplate`
    pub fn compute_dor_template_ref(&self) -> String {
        format!(
            "/content/dam/formsanddocuments/{}/{}/jcr:content/renditions/dorTemplate",
            self.form_path, self.form_code
        )
    }

    /// Populate `form_code`, `form_title`, and `dor_template_ref` from a
    /// document filename and structured content.
    ///
    /// `doc_name` is the filename stem (e.g. `"AAEI_019_DE"`).
    /// The form code is extracted as the part before the first `_`.
    /// The form title is extracted from the first H1 heading in the content.
    pub fn populate_from_document(&mut self, doc_name: &str, content: &[crate::StructuredNode]) {
        // Extract form code: everything before the first '_'
        let form_code = doc_name.split('_').next().unwrap_or(doc_name).to_string();
        self.form_code = form_code;

        // Extract form title from first H1 heading
        for node in content {
            if let crate::StructuredNode::Heading(h) = node {
                if matches!(h.level, crate::HeadingLevel::H1) {
                    self.form_title = h.content.as_plain_text().trim().to_string();
                    break;
                }
            }
        }

        // Compute dor_template_ref from form_path and form_code
        self.dor_template_ref = self.compute_dor_template_ref();
    }

    /// Populate metadata fields from a `Context` (XFA text variables).
    ///
    /// This enables the preview panel in the AEM output which contains
    /// the metadata element with FormRange attributes.
    pub fn populate_from_context(&mut self, ctx: &crate::Context) {
        self.include_preview_panel = true;

        if let Some(v) = ctx.get_variable("formrange_entity") {
            self.metadata_entity = v.to_string();
        }
        if let Some(v) = ctx.get_variable("formrange_version") {
            self.metadata_version = v.to_string();
        }
        // Try formrange_cdokinfo first, fall back to Footer_Line_txtformid
        if let Some(v) = ctx.get_variable("formrange_cdokinfo") {
            self.metadata_cdokinfo = v.to_string();
        } else if let Some(v) = ctx.get_variable("Footer_Line_txtformid") {
            self.metadata_cdokinfo = v.to_string();
        }
        if let Some(v) = ctx.get_variable("formrange_releasedate") {
            self.metadata_releasedate = v.to_string();
        } else if let Some(v) = ctx.get_variable("Footer_Line_txtversiondate") {
            self.metadata_releasedate = v.to_string();
        }
        if let Some(v) = ctx.get_variable("formrange_partnerlevel") {
            self.metadata_partnerlevel = v.to_string();
        }
        if let Some(v) = ctx.get_variable("formrange_clpmandatory") {
            self.metadata_clpmandatory = v.to_string();
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
            AemNode::TextBoxMultiline { uuid, .. } => {
                format!("textboxmultiline_{}", uuid.as_simple())
            }
            AemNode::Repeatable { uuid, .. } => format!("repeatable_{}", uuid.as_simple()),
        }
    }
}
