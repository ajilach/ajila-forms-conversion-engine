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

mod date_field_detector;
mod field_grouper;
mod grid_template;
mod heading_detector;
mod inline_field_detector;
mod label_attacher;
mod master_page_detector;
mod no_print_detector;
mod radio_button_detector;
mod radio_button_grouper;
mod repeatable_detector;
mod text_block;

pub use date_field_detector::DateFieldDetector;
pub use field_grouper::FieldGrouper;
pub use grid_template::GridTemplateDetector;
pub use heading_detector::{GlobalFontStats, HeadingDetector};
pub use inline_field_detector::InlineFieldDetector;
pub use label_attacher::LabelAttacher;
pub use master_page_detector::MasterPageDetector;
pub use no_print_detector::NoPrintDetector;
pub use radio_button_detector::RadioButtonDetector;
pub use radio_button_grouper::RadioButtonGrouper;
pub use repeatable_detector::{RepeatableDetector, RepeatableSection};
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
/// This is a convenience wrapper that constructs a single-element `GlobalContext`
/// from the document's source and delegates to `run_analysis_pipeline_with_context`.
///
/// For processing multiple form states with consistent global statistics,
/// use `run_analysis_pipeline_with_context` directly with a `GlobalContext`
/// containing all flattened states.
pub fn run_analysis_pipeline(doc: &mut crate::document::Document) {
    let single: &[&Flattened] = &[doc.source];
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
    HeadingDetector::new().process_with_context(doc, ctx);
    TextBlockGrouper::new().process_with_context(doc, ctx);
    FieldGrouper::new().process_with_context(doc, ctx);
    DateFieldDetector::new().process_with_context(doc, ctx);
    RadioButtonDetector::new().process_with_context(doc, ctx);
    RadioButtonGrouper::new().process_with_context(doc, ctx);
    InlineFieldDetector::new().process_with_context(doc, ctx);
    LabelAttacher::new().process_with_context(doc, ctx);
    GridTemplateDetector::new().process_with_context(doc, ctx);
    RepeatableDetector::new().process_with_context(doc, ctx);
}
