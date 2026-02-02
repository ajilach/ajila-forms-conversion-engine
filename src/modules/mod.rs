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
//! DateFieldDetector           ─── detects date fields (day.month.year)
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
mod heading_detector;
mod inline_field_detector;
mod label_attacher;
mod master_page_detector;
mod no_print_detector;
mod radio_button_detector;
mod radio_button_grouper;
mod repeatable_detector;
mod structured_converter;
mod text_block;

pub use date_field_detector::DateFieldDetector;
pub use field_grouper::FieldGrouper;
pub use heading_detector::HeadingDetector;
pub use inline_field_detector::InlineFieldDetector;
pub use label_attacher::LabelAttacher;
pub use master_page_detector::MasterPageDetector;
pub use no_print_detector::NoPrintDetector;
pub use radio_button_detector::RadioButtonDetector;
pub use radio_button_grouper::RadioButtonGrouper;
pub use repeatable_detector::{RepeatableDetector, RepeatableSection};
pub use structured_converter::convert as convert_to_structured;
pub use text_block::TextBlockGrouper;

/// Trait for analysis modules that process a Document.
pub trait AnalysisModule {
    /// Process the document, creating new groups as needed.
    fn process(&self, doc: &mut crate::document::Document);

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
/// 5. DateFieldDetector - detect date fields
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
    //DateFieldDetector::new().process(doc);  // Disabled for now
    RadioButtonDetector::new().process(doc);
    RadioButtonGrouper::new().process(doc);
    HeadingDetector::new().process(doc);
    InlineFieldDetector::new().process(doc);
    LabelAttacher::new().process(doc);
    RepeatableDetector::new().process(doc);
}
