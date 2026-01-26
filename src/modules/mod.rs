//! Analysis modules that transform and enrich Document structure.
//!
//! Each module operates on a `Document`, analyzing the flattened nodes
//! and creating composite groups (merging leaf groups into higher-level structures).
//!
//! # Module Pipeline
//!
//! Modules are designed to be run in sequence:
//!
//! ```text
//! Flattened
//!     │
//!     ▼
//! Document::from_flattened()  ─── creates Leaf groups
//!     │
//!     ▼
//! TextBlockGrouper::process() ─── merges adjacent text into TextBlocks
//!     │
//!     ▼
//! FieldGrouper::process()     ─── groups radio buttons into ExclGroups
//!     │
//!     ▼
//! LabelAttacher::process()    ─── pairs labels with fields
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
//! TextBlockGrouper::new().process(&mut doc);
//! FieldGrouper::new().process(&mut doc);
//! LabelAttacher::new().process(&mut doc);
//! ```

mod text_block;
mod field_grouper;
mod label_attacher;
mod heading_detector;
mod radio_button_detector;
mod radio_button_grouper;
mod date_field_detector;

pub use text_block::TextBlockGrouper;
pub use field_grouper::FieldGrouper;
pub use label_attacher::LabelAttacher;
pub use heading_detector::HeadingDetector;
pub use radio_button_detector::RadioButtonDetector;
pub use radio_button_grouper::RadioButtonGrouper;
pub use date_field_detector::DateFieldDetector;

/// Trait for analysis modules that process a Document.
pub trait AnalysisModule {
    /// Process the document, creating new groups as needed.
    fn process(&self, doc: &mut crate::document::Document);
    
    /// Module name for tracking group sources.
    fn name(&self) -> &'static str;
}
