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
            theme_ref: String::new(),
            dor_template_ref: String::new(),
            redirect_url: "/content/forms/af/afforms_global_common/confirm-successful-submission"
                .into(),

            form_path: "ajila-forms-ubs/output/Germany_Tranch_1".into(),
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
    pub fn css_class(&self, component: &str) -> String {
        format!("{}{}", self.css_prefix, component)
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
    },

    /// Checkbox group (`guideCheckBox`).
    Checkbox {
        uuid: Uuid,
        name: String,
        options: Vec<AemOption>,
        alignment: OptionAlignment,
        visible: bool,
        colspan: u32,
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
