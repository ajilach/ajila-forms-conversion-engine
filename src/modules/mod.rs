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
//! TextBlockGrouper            ─── merges adjacent text into TextBlocks
//!     │
//!     ▼
//! FieldGrouper                ─── wraps fields in Field groups
//!     │
//!     ▼
//! RadioButtonDetector         ─── detects radio buttons (square fields with labels)
//!     │
//!     ▼
//! RadioButtonGrouper          ─── groups radio buttons on same line
//!     │
//!     ▼
//! HeadingDetector             ─── identifies headings (must run BEFORE LabelAttacher)
//!     │
//!     ▼
//! InlineFieldDetector         ─── identify inline fields
//!     │
//!     ▼
//! LabelAttacher               ─── pairs labels with fields (uses only non-heading text)
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

mod field_grouper;
mod heading_detector;
mod html_generator;
mod inline_field_detector;
mod label_attacher;
mod master_page_detector;
mod merge_structured;
mod no_print_detector;
mod radio_button_detector;
mod radio_button_grouper;
mod repeatable_detector;
mod structured_converter;
mod text_block;

pub use field_grouper::FieldGrouper;
pub use heading_detector::{GlobalFontStats, HeadingDetector};
pub use html_generator::{HtmlConfig, generate_form_body, generate_html};
pub use inline_field_detector::InlineFieldDetector;
pub use label_attacher::LabelAttacher;
pub use master_page_detector::MasterPageDetector;
pub use merge_structured::{MergeInput, merge_structured_trees};
pub use no_print_detector::NoPrintDetector;
pub use radio_button_detector::RadioButtonDetector;
pub use radio_button_grouper::RadioButtonGrouper;
pub use repeatable_detector::{RepeatableDetector, RepeatableSection};
pub use structured_converter::convert as convert_to_structured;
pub use text_block::TextBlockGrouper;

use crate::flattened::Flattened;

/// Global context for analysis modules when running in exhaustive mode.
///
/// This struct holds references to all flattened form states, allowing modules
/// to compute global statistics that are consistent across all states.
pub struct GlobalContext<'a> {
    /// All flattened form states collected during exhaustive exploration
    pub all_flattened: &'a [&'a Flattened],
    /// Pre-computed global font statistics (computed once, used by HeadingDetector)
    pub font_stats: Option<GlobalFontStats>,
}

impl<'a> GlobalContext<'a> {
    /// Create a new global context from a slice of flattened references.
    pub fn new(all_flattened: &'a [&'a Flattened]) -> Self {
        Self {
            all_flattened,
            font_stats: None,
        }
    }

    /// Create a global context with pre-computed font statistics.
    pub fn with_font_stats(
        all_flattened: &'a [&'a Flattened],
        font_stats: GlobalFontStats,
    ) -> Self {
        Self {
            all_flattened,
            font_stats: Some(font_stats),
        }
    }

    /// Compute global font statistics from all flattened states.
    pub fn compute_font_stats(&self) -> GlobalFontStats {
        GlobalFontStats::from_flattened_iter(self.all_flattened.iter().copied())
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
/// This runs all analysis modules in the correct order:
/// 1. NoPrintDetector - claim elements with relevant="-print" (MUST run first)
/// 2. MasterPageDetector - identify header/footer from master page content
/// 3. TextBlockGrouper - merge adjacent text into TextBlocks
/// 4. FieldGrouper - wrap fields in Field groups  
/// 6. RadioButtonDetector - detect radio buttons
/// 7. RadioButtonGrouper - group radio buttons on same line
/// 8. HeadingDetector - identify headings (MUST run before LabelAttacher)
/// 9. InlineFieldDetector - identify inline fields (text before/after but no label above/below)
/// 10. LabelAttacher - pair labels with fields (only uses non-heading text)
/// 11. RepeatableDetector - detect repeatable sections (MUST run last to collect outermost groups)
///
/// The order is important:
/// - NoPrintDetector must run first to claim screen-only elements before other modules
/// - MasterPageDetector must run early to tag master page content
/// - HeadingDetector must run before LabelAttacher so headings aren't attached as labels
/// - InlineFieldDetector must run after LabelAttacher to identify unlabeled fields with adjacent text
/// - RepeatableDetector must run last so it can collect composite groups (LabeledField, etc.)
pub fn run_analysis_pipeline(doc: &mut crate::document::Document) {
    NoPrintDetector::new().process(doc);
    MasterPageDetector::new().process(doc);
    TextBlockGrouper::new().process(doc);
    FieldGrouper::new().process(doc);
    RadioButtonDetector::new().process(doc);
    RadioButtonGrouper::new().process(doc);
    HeadingDetector::new().process(doc);
    InlineFieldDetector::new().process(doc);
    LabelAttacher::new().process(doc);
    RepeatableDetector::new().process(doc);
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
    FieldGrouper::new().process_with_context(doc, ctx);
    RadioButtonDetector::new().process_with_context(doc, ctx);
    RadioButtonGrouper::new().process_with_context(doc, ctx);
    HeadingDetector::new().process_with_context(doc, ctx);
    InlineFieldDetector::new().process_with_context(doc, ctx);
    LabelAttacher::new().process_with_context(doc, ctx);
    RepeatableDetector::new().process_with_context(doc, ctx);
}
