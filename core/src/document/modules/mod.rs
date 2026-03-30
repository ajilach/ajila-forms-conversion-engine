//! Analysis modules that transform and enrich Document structure.
//!
//! Each module operates on a `Document`, analyzing the flattened nodes
//! and creating composite groups (merging leaf groups into higher-level structures).
//!
//! # Module Pipeline
//!
//! Modules are designed to be run in sequence. Use `run_analysis_pipeline()` to run
//! all modules in the correct order:
//!
//! ```text
//! Flattened
//!     │
//!     ▼
//! Document::from_flattened()  ─── creates Leaf groups
//!     │
//!     ▼
//! NoPrintDetector             ─── claims elements with relevant="-print" (FIRST)
//!     │
//!     ▼
//! MasterPageDetector          ─── identify header/footer from master page content
//!     │
//!     ▼
//! TextBlockGrouper            ─── wraps each text node in a TextBlock
//!     │
//!     ▼
//! PlaceholderFilter           ─── claims placeholder text ("...", "___")
//!     │
//!     ▼
//! FieldGrouper                ─── wraps fields in Field groups
//!     │
//!     ▼
//! RadioButtonDetector         ─── detects radio buttons (square fields with labels)
//!     │
//!     ▼
//! CheckboxDetector            ─── detects checkboxes (square fields with labels)
//!     │
//!     ▼
//! RadioButtonGrouper          ─── groups radio buttons on same line
//!     │
//!     ▼
//! SelectionInlineFieldDetector ── detects inline fields next to checkboxes/radio buttons
//!     │
//!     ▼
//! OverlappingTextBlockMerger  ─── merges text blocks contained within others
//!     │
//!     ▼
//! TextBlockMerger             ─── merges nearby unclaimed TextBlocks with same font
//!     │
//!     ▼
//! FieldTableDetector          ─── detects field tables with bold headers
//!     │
//!     ▼
//! InlineFieldDetector         ─── identify inline fields
//!     │
//!     ▼
//! LabelAttacher               ─── pairs labels with fields (gets first pick of text blocks)
//!     │
//!     ▼
//! HeadingDetector             ─── identifies headings (runs AFTER LabelAttacher)
//!     │
//!     ▼
//! RepeatableDetector          ─── detects repeatable sections (LAST - collects composites)
//!     │
//!     ▼
//! Document with rich group structure
//! ```
//!
//! # Global Context for Exhaustive Mode
//!
//! When running in exhaustive mode, modules can access global statistics computed
//! from ALL form states via `GlobalContext`. This ensures consistent heading
//! detection, etc. across different form states.
//!
//! # Example
//!
//! ```ignore
//! let flattened = Flattened::from_xfa(&nodes)?;
//! let mut doc = Document::from_flattened(&flattened);
//!
//! // Run the full analysis pipeline
//! run_analysis_pipeline(&mut doc);
//! ```

mod checkbox_content;
mod checkbox_detector;
mod date_field_detector;
mod field_grouper;
mod field_table_detector;
mod grid_template;
mod heading_detector;
mod inline_field_date_picker;
mod inline_field_detector;
mod label_attacher;
mod list_detector;
mod master_page_detector;
mod no_print_detector;
mod overlapping_text_block_merger;
mod placeholder_filter;
mod radio_button_content;
mod radio_button_detector;
mod radio_button_grouper;
mod repeatable_detector;
mod selection_inline_field;
mod text_block;
mod text_block_merger;
mod table_detector;

pub use checkbox_content::CheckboxContentDetector;
pub use checkbox_detector::CheckboxDetector;
pub use date_field_detector::DateFieldDetector;
pub use field_grouper::FieldGrouper;
pub use field_table_detector::FieldTableDetector;
pub use grid_template::GridTemplateDetector;
pub use heading_detector::{GlobalFontStats, HeadingDetector};
pub use inline_field_date_picker::InlineFieldDatePicker;
pub use inline_field_detector::InlineFieldDetector;
pub use label_attacher::LabelAttacher;
pub use list_detector::ListDetector;
pub use master_page_detector::MasterPageDetector;
pub use no_print_detector::NoPrintDetector;
pub use overlapping_text_block_merger::OverlappingTextBlockMerger;
pub use placeholder_filter::PlaceholderFilter;
pub use radio_button_content::RadioButtonContentDetector;
pub use radio_button_detector::RadioButtonDetector;
pub use radio_button_grouper::RadioButtonGrouper;
pub use repeatable_detector::{RepeatableDetector, RepeatableSection};
pub use selection_inline_field::SelectionInlineFieldDetector;
pub use text_block::TextBlockGrouper;
pub use text_block_merger::TextBlockMerger;
pub use table_detector::TableDetector;

use crate::flattened::Flattened;

/// Global context for analysis modules when running in exhaustive mode.
///
/// This struct holds all flattened form states, allowing modules
/// to compute global statistics that are consistent across all states.
pub struct GlobalContext {
    /// All flattened form states collected during exhaustive exploration
    pub all_flattened: Vec<Flattened>,
}

impl GlobalContext {
    /// Create a new global context from a vec of flattened values.
    pub fn new(all_flattened: Vec<Flattened>) -> Self {
        Self { all_flattened }
    }
}

/// Trait for analysis modules that process a Document.
pub trait AnalysisModule {
    /// Process the document, creating new groups as needed.
    fn process(&self, doc: &mut crate::document::Document);

    /// Process the document with access to global context from all form states.
    /// Default implementation ignores the global context and calls `process`.
    fn process_with_context(&self, doc: &mut crate::document::Document, _ctx: &GlobalContext) {
        self.process(doc);
    }

    /// Module name for tracking group sources.
    fn name(&self) -> &'static str;
}

/// Run the full analysis pipeline on a document.
///
/// This is a convenience wrapper that constructs a single-element `GlobalContext`
/// from the document's source and delegates to `run_analysis_pipeline_with_context`.
///
/// For processing multiple form states with consistent global statistics,
/// use `run_analysis_pipeline_with_context` directly with a `GlobalContext`
/// containing all flattened states.
pub fn run_analysis_pipeline(doc: &mut crate::document::Document) {
    let single = vec![doc.source.clone()];
    let ctx = GlobalContext::new(single);
    run_analysis_pipeline_with_context(doc, &ctx);
}

/// Run the full analysis pipeline with global context from all form states.
///
/// This is used in exhaustive mode to ensure consistent statistics (e.g., heading
/// detection) across all form states. Modules that support global context will
/// use the pre-computed statistics instead of computing local ones.
pub fn run_analysis_pipeline_with_context(
    doc: &mut crate::document::Document,
    ctx: &GlobalContext,
) {
    NoPrintDetector::new().process_with_context(doc, ctx);
    MasterPageDetector::new().process_with_context(doc, ctx);
    TextBlockGrouper::new().process_with_context(doc, ctx);
    PlaceholderFilter::new().process_with_context(doc, ctx);
    FieldGrouper::new().process_with_context(doc, ctx);
    DateFieldDetector::new().process_with_context(doc, ctx);
    InlineFieldDatePicker::new().process_with_context(doc, ctx);
    OverlappingTextBlockMerger::new().process_with_context(doc, ctx);
    RadioButtonDetector::new().process_with_context(doc, ctx);
    CheckboxDetector::new().process_with_context(doc, ctx);
    ListDetector::new().process_with_context(doc, ctx);
    RadioButtonGrouper::new().process_with_context(doc, ctx);
    SelectionInlineFieldDetector::new().process_with_context(doc, ctx);
    RadioButtonContentDetector::new().process_with_context(doc, ctx);
    CheckboxContentDetector::new().process_with_context(doc, ctx);
    TextBlockMerger::new().process_with_context(doc, ctx);

    FieldTableDetector::new().process_with_context(doc, ctx);

    HeadingDetector::new().process_with_context(doc, ctx);
    InlineFieldDetector::new().process_with_context(doc, ctx);
    LabelAttacher::new().process_with_context(doc, ctx);
    GridTemplateDetector::new().process_with_context(doc, ctx);

    TableDetector::new().process_with_context(doc, ctx);

    //FieldTableDetectorVertical::new().process_with_context(doc, ctx);

    RepeatableDetector::new().process_with_context(doc, ctx);
}
