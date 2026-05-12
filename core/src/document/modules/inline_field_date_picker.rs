//! Inline date picker detector module.
//!
//! Detects inline date patterns in text content and radio option labels,
//! such as "Löschung ab: 01..1" or "Änderung Zahlungsempfänger ab: 01. ."
//!
//! These patterns are converted to labelled date picker fields with generated names,
//! preserving any trailing content after the date pattern.
//!
//! # Examples
//!
//! - "Löschung ab: 01..1" → Label: "Löschung ab:", Field: InlineDate_Löschung_ab
//! - "Änderung Zahlungsempfänger ab: 01. ." → Label: "Änderung Zahlungsempfänger ab:", Field: InlineDate_Änderung_Zahlungsempfänger_ab

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::FlattenedNodeKind;
use regex_lite::Regex;
use std::sync::LazyLock;

/// Represents a date field transformation to be applied.
/// Contains the original group index, label, optional suffix text,
/// generated field name, and associated field indices.
type DateFieldTransform = (usize, String, Option<String>, String, Vec<usize>);

/// Regex patterns for inline date detection.
///
/// Pattern 1: `\d{1,2}\.\s*\.\s*\d*` matches "01..1", "01. .1", "1..2024", etc.
/// Pattern 2: `\d{1,2}\.\s+\.\s*` matches "01. .", "1.  .", etc. (with space after first dot)
static INLINE_DATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Combined pattern: day followed by dots with optional spaces and optional year fragment
    // Examples: "01..1", "01. .", "1..2024", "01.."
    Regex::new(r"\d{1,2}\.\s*\.\s*\d*").unwrap()
});

/// Detects inline date patterns in text blocks and radio option labels.
///
/// When a pattern like "Löschung ab: 01..1" is detected:
/// 1. The prefix "Löschung ab:" becomes the label
/// 2. The date pattern "01..1" is replaced with a date picker field
/// 3. Any suffix after the pattern is preserved
pub struct InlineFieldDatePicker;

impl Default for InlineFieldDatePicker {
    fn default() -> Self {
        Self::new()
    }
}

impl InlineFieldDatePicker {
    pub fn new() -> Self {
        InlineFieldDatePicker
    }

    /// Check if text contains an inline date pattern.
    /// Returns the match position if found: (start, end, matched_text)
    fn find_date_pattern(text: &str) -> Option<(usize, usize, String)> {
        INLINE_DATE_PATTERN
            .find(text)
            .map(|m| (m.start(), m.end(), m.as_str().to_string()))
    }

    /// Generate a field name from the label text.
    /// Strips non-alphanumeric characters, replaces spaces with underscores,
    /// and prefixes with "InlineDate_".
    ///
    /// Examples:
    /// - "Löschung ab:" → "InlineDate_Löschung_ab"
    /// - "Änderung Zahlungsempfänger ab:" → "InlineDate_Änderung_Zahlungsempfänger_ab"
    fn generate_field_name(label: &str) -> String {
        let sanitized: String = label
            .chars()
            .filter_map(|c| {
                if c.is_alphanumeric() {
                    Some(c)
                } else if c.is_whitespace() {
                    Some('_')
                } else {
                    None
                }
            })
            .collect();

        // Remove trailing underscores and collapse multiple underscores
        let collapsed: String = sanitized
            .split('_')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("_");

        format!("InlineDate_{}", collapsed)
    }

    /// Extract label (prefix), date pattern position, and suffix from text.
    /// Returns: (label_text, suffix_text)
    fn extract_parts(text: &str) -> Option<(String, Option<String>)> {
        let (start, end, _matched) = Self::find_date_pattern(text)?;

        let prefix = text[..start].trim();
        let suffix = text[end..].trim();

        // We need at least some prefix to use as a label
        if prefix.is_empty() {
            return None;
        }

        let suffix_opt = if suffix.is_empty() {
            None
        } else {
            Some(suffix.to_string())
        };

        Some((prefix.to_string(), suffix_opt))
    }

    /// Process a single text group to detect inline date patterns.
    /// Returns Some((label, suffix, generated_name)) if a pattern was found.
    fn analyze_text_content(&self, text: &str) -> Option<(String, Option<String>, String)> {
        let (label, suffix) = Self::extract_parts(text)?;
        let generated_name = Self::generate_field_name(&label);
        Some((label, suffix, generated_name))
    }
}

impl AnalysisModule for InlineFieldDatePicker {
    fn name(&self) -> &'static str {
        "InlineFieldDatePicker"
    }

    fn process(&self, doc: &mut Document) {
        // Collect text blocks that might contain inline date patterns
        let text_blocks: Vec<usize> = doc.root_groups_matching(|doc, idx| doc.is_text_block(idx));

        // Also collect radio button groups to check their option labels
        let radio_groups: Vec<usize> = doc.root_groups_matching(|doc, idx| {
            matches!(
                doc.get_group(idx).map(|g| &g.kind),
                Some(GroupKind::RadioButtonGroup) | Some(GroupKind::ExclGroup { .. })
            )
        });

        // Track groups to transform: (original_idx, label, suffix, generated_name, field_indices)
        let mut transforms: Vec<DateFieldTransform> = Vec::new();

        // Check text blocks for inline date patterns
        for &text_idx in &text_blocks {
            let text_content = doc.get_text_content(text_idx);
            if let Some((label, suffix, generated_name)) = self.analyze_text_content(&text_content)
            {
                transforms.push((text_idx, label, suffix, generated_name, vec![]));
            }
        }

        // Check radio option labels for inline date patterns
        for &radio_idx in &radio_groups {
            // Get child nodes to check their labels/values
            let nodes = doc.collect_nodes(radio_idx);

            for node in &nodes {
                if let FlattenedNodeKind::Field { name, .. } = &node.kind {
                    // Check if the field name contains a date pattern
                    if let Some((label, suffix, generated_name)) = self.analyze_text_content(name) {
                        // For radio options, we create a separate date field associated with this option
                        // The radio option itself remains unchanged
                        transforms.push((
                            radio_idx,
                            label,
                            suffix,
                            generated_name,
                            vec![radio_idx],
                        ));
                        break; // Only process first match per radio group
                    }
                }
            }
        }

        // Apply transforms - create InlineDateField groups
        for (original_idx, label, suffix, generated_name, field_indices) in transforms {
            // Get the children of the original group
            let children = if let Some(group) = doc.get_group(original_idx) {
                group.children.clone()
            } else {
                vec![original_idx]
            };

            // Create new InlineDateField group wrapping the original content
            doc.merge_inferred(
                children,
                GroupKind::InlineDateField {
                    label_text: label,
                    suffix_text: suffix,
                    field_indices,
                    generated_name,
                },
                self.name(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::TextBlockGrouper;
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_find_date_pattern_basic() {
        // Pattern: 01..1
        let result = InlineFieldDatePicker::find_date_pattern("Löschung ab: 01..1");
        assert!(result.is_some());
        let (start, end, matched) = result.unwrap();
        assert_eq!(matched, "01..1");
        assert_eq!(&"Löschung ab: 01..1"[start..end], "01..1");
    }

    #[test]
    fn test_find_date_pattern_with_spaces() {
        // Pattern: 01. .
        let result = InlineFieldDatePicker::find_date_pattern("Änderung ab: 01. .");
        assert!(result.is_some());
        let (_start, _end, matched) = result.unwrap();
        assert_eq!(matched, "01. .");
    }

    #[test]
    fn test_find_date_pattern_with_many_spaces() {
        // Pattern with many spaces like in the actual document: "01.      ."
        let result =
            InlineFieldDatePicker::find_date_pattern("Änderung Zahlungsempfänger ab: 01.      .");
        assert!(result.is_some());
        let (_start, _end, matched) = result.unwrap();
        assert_eq!(matched, "01.      .");
    }

    #[test]
    fn test_find_date_pattern_full_year() {
        // Pattern with full year hint: 01..2024
        let result = InlineFieldDatePicker::find_date_pattern("Start: 01..2024");
        assert!(result.is_some());
        let (_start, _end, matched) = result.unwrap();
        assert_eq!(matched, "01..2024");
    }

    #[test]
    fn test_find_date_pattern_no_match() {
        // Normal date should not match
        assert!(InlineFieldDatePicker::find_date_pattern("Date: 01.12.2024").is_none());
        // No pattern
        assert!(InlineFieldDatePicker::find_date_pattern("Just some text").is_none());
    }

    #[test]
    fn test_generate_field_name_basic() {
        let name = InlineFieldDatePicker::generate_field_name("Löschung ab:");
        assert_eq!(name, "InlineDate_Löschung_ab");
    }

    #[test]
    fn test_generate_field_name_complex() {
        let name = InlineFieldDatePicker::generate_field_name("Änderung Zahlungsempfänger ab:");
        assert_eq!(name, "InlineDate_Änderung_Zahlungsempfänger_ab");
    }

    #[test]
    fn test_generate_field_name_special_chars() {
        let name = InlineFieldDatePicker::generate_field_name("Test / Value (special):");
        assert_eq!(name, "InlineDate_Test_Value_special");
    }

    #[test]
    fn test_extract_parts_basic() {
        let result = InlineFieldDatePicker::extract_parts("Löschung ab: 01..1");
        assert!(result.is_some());
        let (label, suffix) = result.unwrap();
        assert_eq!(label, "Löschung ab:");
        assert!(suffix.is_none());
    }

    #[test]
    fn test_extract_parts_with_suffix() {
        let result = InlineFieldDatePicker::extract_parts("Start: 01..1 (required)");
        assert!(result.is_some());
        let (label, suffix) = result.unwrap();
        assert_eq!(label, "Start:");
        assert_eq!(suffix, Some("(required)".to_string()));
    }

    #[test]
    fn test_extract_parts_no_prefix() {
        // Date pattern at the start - no label, should return None
        let result = InlineFieldDatePicker::extract_parts("01..1 only");
        assert!(result.is_none());
    }

    #[test]
    fn test_detector_with_text_block() {
        // Create a text node with inline date pattern
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![FlattenedNode::new_text(
                "Löschung ab: 01..1".to_string(),
                num(10.0),
                "Helvetica".to_string(),
                num(10.0),
                num(100.0),
                num(200.0),
                num(20.0),
            )],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        InlineFieldDatePicker::new().process(&mut doc);

        // Find InlineDateField groups
        let inline_date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::InlineDateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(inline_date_fields.len(), 1);

        // Verify the extracted data
        if let Some(group) = doc.get_group(inline_date_fields[0]) {
            if let GroupKind::InlineDateField {
                label_text,
                suffix_text,
                generated_name,
                ..
            } = &group.kind
            {
                assert_eq!(label_text, "Löschung ab:");
                assert!(suffix_text.is_none());
                assert_eq!(generated_name, "InlineDate_Löschung_ab");
            } else {
                panic!("Expected InlineDateField group kind");
            }
        }
    }

    #[test]
    fn test_detector_multiple_patterns() {
        // Create multiple text nodes with date patterns
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Löschung ab: 01..1".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(200.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "Änderung ab: 01. .".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(150.0),
                    num(200.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        InlineFieldDatePicker::new().process(&mut doc);

        // Should find two InlineDateField groups
        let inline_date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::InlineDateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(inline_date_fields.len(), 2);
    }

    #[test]
    fn test_label_does_not_include_date_pattern() {
        let detector = InlineFieldDatePicker::new();

        // Test that the label extracted from "Löschung ab: 01..1" does NOT include "01..1"
        let result = detector.analyze_text_content("Löschung ab: 01..1");
        assert!(result.is_some());
        let (label, _suffix, _name) = result.unwrap();
        assert!(!label.contains("01"));
        assert!(!label.contains(".."));
        assert_eq!(label, "Löschung ab:");

        // Test for the other pattern
        let result = detector.analyze_text_content("Änderung Zahlungsempfänger ab: 01. .");
        assert!(result.is_some());
        let (label, _suffix, _name) = result.unwrap();
        assert!(!label.contains("01"));
        assert_eq!(label, "Änderung Zahlungsempfänger ab:");
    }

    #[test]
    fn test_aaab_inline_date_fields_detected() {
        use crate::document::modules::run_analysis_pipeline;
        use crate::extract_xfa_from_pdf;
        use crate::xfa::XfaNode;
        use crate::xfa::scripting::XfaForm;

        // Load AAAB PDF
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
        let pdf_path = format!("{}/input/AAAB_019_DE.pdf", manifest_dir);
        let xfa_data = extract_xfa_from_pdf(&pdf_path).expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");

        let nodes = XfaNode::parse(&xfa_data.unwrap()).expect("Failed to parse XFA structure");

        // Create XfaForm and select RB_3 (Löschung) to make the "Löschung ab:" section visible
        let mut form = XfaForm::new(nodes).expect("Failed to create XfaForm");

        // Select RB_3 to show the "Löschung" section
        form.select_radio_button("RB_3")
            .expect("Failed to select RB_3");
        form.refresh().expect("Failed to refresh form");

        // Get flattened from the form with RB_3 selected
        let flattened = form.flattened();

        // Create document and run full pipeline
        let mut doc = Document::from_flattened(flattened);
        run_analysis_pipeline(&mut doc);

        // Find all InlineDateField groups
        let inline_date_fields: Vec<_> = doc
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| matches!(g.kind, GroupKind::InlineDateField { .. }))
            .collect();

        // Collect the labels
        let labels: Vec<String> = inline_date_fields
            .iter()
            .filter_map(|(_, g)| {
                if let GroupKind::InlineDateField { label_text, .. } = &g.kind {
                    Some(label_text.clone())
                } else {
                    None
                }
            })
            .collect();

        println!("Found {} InlineDateField groups:", inline_date_fields.len());
        for (idx, group) in &inline_date_fields {
            if let GroupKind::InlineDateField {
                label_text,
                generated_name,
                ..
            } = &group.kind
            {
                println!(
                    "  [{}] Label: '{}', Name: '{}'",
                    idx, label_text, generated_name
                );
            }
        }

        // Assert that we found the expected patterns
        // "Löschung ab:" should appear exactly once
        let loeschung_count = labels.iter().filter(|l| l.contains("Löschung ab")).count();
        assert_eq!(
            loeschung_count, 1,
            "Expected exactly 1 'Löschung ab' label, found {}",
            loeschung_count
        );

        // "Änderung Zahlungsempfänger ab:" should appear exactly once
        let aenderung_count = labels
            .iter()
            .filter(|l| l.contains("Änderung Zahlungsempfänger ab"))
            .count();
        assert_eq!(
            aenderung_count, 1,
            "Expected exactly 1 'Änderung Zahlungsempfänger ab' label, found {}",
            aenderung_count
        );

        // Verify that labels do NOT contain the date-specific patterns
        for label in &labels {
            assert!(
                !label.contains("01"),
                "Label '{}' should not contain '01'",
                label
            );
            assert!(
                !label.contains(".."),
                "Label '{}' should not contain '..'",
                label
            );
        }
    }
}
