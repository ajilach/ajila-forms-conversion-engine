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
//! RepeatableDetector          ─── detects repeatable sections from XFA occur hints
//!     │
//!     ▼
//! HeadingDetector             ─── identifies headings (must run BEFORE LabelAttacher)
//!     │
//!     ▼
//! LabelAttacher               ─── pairs labels with fields (uses only non-heading text)
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

mod text_block;
mod field_grouper;
mod label_attacher;
mod heading_detector;
mod radio_button_detector;
mod radio_button_grouper;
mod date_field_detector;
mod repeatable_detector;
mod master_page_detector;
mod inline_field_detector;

pub use text_block::TextBlockGrouper;
pub use field_grouper::FieldGrouper;
pub use label_attacher::LabelAttacher;
pub use heading_detector::HeadingDetector;
pub use radio_button_detector::RadioButtonDetector;
pub use radio_button_grouper::RadioButtonGrouper;
pub use date_field_detector::DateFieldDetector;
pub use repeatable_detector::{RepeatableDetector, RepeatableSection};
pub use master_page_detector::MasterPageDetector;
pub use inline_field_detector::InlineFieldDetector;

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
/// 1. MasterPageDetector - identify header/footer from master page content (FIRST)
/// 2. TextBlockGrouper - merge adjacent text into TextBlocks
/// 3. FieldGrouper - wrap fields in Field groups  
/// 4. DateFieldDetector - detect date fields
/// 5. RadioButtonDetector - detect radio buttons
/// 6. RadioButtonGrouper - group radio buttons on same line
/// 7. HeadingDetector - identify headings (MUST run before LabelAttacher)
/// 8. LabelAttacher - pair labels with fields (only uses non-heading text)
/// 9. InlineFieldDetector - identify inline fields (text before/after but no label above/below)
/// 10. RepeatableDetector - detect repeatable sections (MUST run last to collect outermost groups)
///
/// The order is important: 
/// - MasterPageDetector must run first to tag master page content
/// - HeadingDetector must run before LabelAttacher so headings aren't attached as labels
/// - InlineFieldDetector must run after LabelAttacher to identify unlabeled fields with adjacent text
/// - RepeatableDetector must run last so it can collect composite groups (LabeledField, etc.)
pub fn run_analysis_pipeline(doc: &mut crate::document::Document) {
    TextBlockGrouper::new().process(doc);
    FieldGrouper::new().process(doc);
    HeadingDetector::new().process(doc);
    //DateFieldDetector::new().process(doc);
    RadioButtonDetector::new().process(doc);
    RadioButtonGrouper::new().process(doc);
    InlineFieldDetector::new().process(doc);
    LabelAttacher::new().process(doc);
    RepeatableDetector::new().process(doc);
    MasterPageDetector::new().process(doc);
}
