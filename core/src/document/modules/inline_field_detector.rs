//! Inline field detector module.
//!
//! Identifies fields that have text directly before or after them on the same line,
//! without having a text block aligned above or below (no separate label).
//!
//! These are typically fields embedded within flowing text, like:
//! "Please enter your name: [___] and press submit."
//!
//! Also detects fields that are spatially contained within a TextBlock's bounds,
//! indicating the field is embedded in flowing text.

use super::AnalysisModule;
use crate::document::Document;
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use std::str::FromStr;

/// Detects inline fields - fields with text directly before/after but no label above/below.
///
/// An inline field is detected if:
/// - Field has text on the same line (directly before or after) AND no label above/below, OR
/// - Field is spatially contained within a TextBlock's bounds (embedded in text)
///
/// This is useful for identifying fields that are part of flowing text rather than
/// traditional form layouts with labels positioned above, below, or to the left.
pub struct InlineFieldDetector {
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
    /// Maximum horizontal gap for adjacent text
    pub horizontal_threshold: Decimal,
    /// Maximum vertical gap to check for labels above/below
    pub vertical_threshold: Decimal,
}

impl Default for InlineFieldDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl InlineFieldDetector {
    /// Minimum number of characters of adjacent text on the same line for a
    /// field to be considered "inline".  If the total adjacent text is shorter
    /// than this, the text is likely a simple label (e.g. "Name", "Vorname")
    /// rather than flowing prose, and the field should be left for the
    /// LabelAttacher instead.
    const MIN_INLINE_TEXT_CHARS: usize = 20;

    /// Maximum number of characters for text above/below a field to be
    /// considered a label. Text longer than this (e.g. paragraphs, section
    /// descriptions) is not treated as a field label and won't prevent
    /// inline field detection.
    const MAX_LABEL_ABOVE_BELOW_CHARS: usize = 100;

    pub fn new() -> Self {
        InlineFieldDetector {
            line_tolerance: Decimal::from_str("8.0").unwrap(),
            horizontal_threshold: Decimal::from_str("50.0").unwrap(),
            vertical_threshold: Decimal::from_str("30.0").unwrap(),
        }
    }

    /// Configure the line tolerance.
    pub fn with_line_tolerance(mut self, tolerance: Decimal) -> Self {
        self.line_tolerance = tolerance;
        self
    }

    /// Configure the horizontal threshold.
    pub fn with_horizontal_threshold(mut self, threshold: Decimal) -> Self {
        self.horizontal_threshold = threshold;
        self
    }

    /// Configure the vertical threshold.
    pub fn with_vertical_threshold(mut self, threshold: Decimal) -> Self {
        self.vertical_threshold = threshold;
        self
    }

    /// Check if text is directly to the left of the field on the same line.
    fn has_text_left(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> bool {
        text_bounds
            .is_left_within(field_bounds, self.horizontal_threshold, self.line_tolerance)
            .is_some()
    }

    /// Check if text is directly to the right of the field on the same line.
    fn has_text_right(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> bool {
        text_bounds
            .is_right_within(field_bounds, self.horizontal_threshold, self.line_tolerance)
            .is_some()
    }

    /// Check if text is aligned above the field (potential label position).
    fn has_text_above(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> bool {
        text_bounds
            .is_above_within(field_bounds, self.vertical_threshold, self.line_tolerance)
            .is_some()
    }

    /// Check if text is aligned below the field (potential label position).
    fn has_text_below(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> bool {
        text_bounds
            .is_below_within(field_bounds, self.vertical_threshold, self.line_tolerance)
            .is_some()
    }

    /// Check if a field is an inline field based on adjacent text.
    ///
    /// Returns true if:
    /// - Field has text directly before or after on the same line
    /// - Field does NOT have a text block aligned above or below
    /// - The adjacent text is long enough to look like flowing text (not just a label)
    fn is_inline_by_adjacency(
        &self,
        doc: &Document,
        field_idx: usize,
        text_groups: &[usize],
    ) -> bool {
        let Some(field_bounds) = doc.get_bounds(field_idx) else {
            return false;
        };

        let mut has_adjacent_text = false;
        let mut has_label_above_or_below = false;
        let mut adjacent_char_count: usize = 0;

        for &text_idx in text_groups {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };

            let text_content = doc.get_text_content(text_idx);
            let trimmed = text_content.trim();

            // Skip empty text
            if trimmed.is_empty() {
                continue;
            }

            // Check for text on same line (left or right)
            if self.has_text_left(&text_bounds, &field_bounds)
                || self.has_text_right(&text_bounds, &field_bounds)
            {
                has_adjacent_text = true;
                adjacent_char_count += trimmed.chars().count();
            }

            // Check for text above or below (potential label)
            if self.has_text_above(&text_bounds, &field_bounds)
                || self.has_text_below(&text_bounds, &field_bounds)
            {
                has_label_above_or_below = true;
            }
        }

        // Inline field: has adjacent text, the adjacent text is long enough
        // to look like flowing text (not just a short label like "Name" or
        // "Vorname"), and no label above/below.
        // Short adjacent text (< MIN_INLINE_TEXT_CHARS chars) is likely a label
        // intended for the LabelAttacher, not inline field context.
        has_adjacent_text
            && !has_label_above_or_below
            && adjacent_char_count >= Self::MIN_INLINE_TEXT_CHARS
    }

    /// Check if a field is contained within a TextBlock's bounds,
    /// without having a label above or below.
    ///
    /// This detects fields that are spatially embedded within text,
    /// where the field's bounds are contained within the TextBlock's bounding box
    /// and there's no text block aligned above or below (which would be a label).
    fn is_contained_in_text_block(
        &self,
        doc: &Document,
        field_idx: usize,
        text_groups: &[usize],
    ) -> bool {
        let Some(field_bounds) = doc.get_bounds(field_idx) else {
            return false;
        };

        let mut has_overlapping_text_on_same_line = false;
        let mut has_label_above_or_below = false;

        for &text_idx in text_groups {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };

            // Skip empty text
            let text_content = doc.get_text_content(text_idx);
            let trimmed = text_content.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Check if field is on the same line and horizontally overlaps/contained
            if text_bounds.is_on_same_line(&field_bounds, self.line_tolerance) {
                // Field is contained if it's within or overlapping the text's horizontal span
                // and on the same line
                if field_bounds.overlaps_horizontally(&text_bounds, self.line_tolerance) {
                    has_overlapping_text_on_same_line = true;
                }
            }

            // Check for text above or below (potential label).
            // Only short text is considered a potential label — long text
            // (>= MAX_LABEL_ABOVE_BELOW_CHARS) is likely a paragraph or section
            // heading, not a field label.
            if trimmed.chars().count() < Self::MAX_LABEL_ABOVE_BELOW_CHARS {
                if self.has_text_above(&text_bounds, &field_bounds)
                    || self.has_text_below(&text_bounds, &field_bounds)
                {
                    has_label_above_or_below = true;
                }
            }
        }

        // Only inline if has overlapping text on same line AND no label above/below
        has_overlapping_text_on_same_line && !has_label_above_or_below
    }
}

impl AnalysisModule for InlineFieldDetector {
    fn name(&self) -> &'static str {
        "InlineFieldDetector"
    }

    fn process(&self, doc: &mut Document) {
        // Get all root groups
        let roots = doc.roots();

        // Find TextBlock groups that are NOT headings
        let text_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| doc.is_text_block(idx) && !doc.is_heading(idx))
            .copied()
            .collect();

        // Find Field groups that are not already part of a labeled field, radio button, etc.
        // Also consider MultiField groups.
        let field_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| doc.is_field(idx))
            .copied()
            .collect();

        if text_groups.is_empty() || field_groups.is_empty() {
            return;
        }

        // Collect inline fields with their before/after text groups
        struct InlineFieldInfo {
            field_idx: usize,
            before: Vec<usize>,
            after: Vec<usize>,
        }

        let mut inline_infos: Vec<InlineFieldInfo> = Vec::new();

        for &field_idx in &field_groups {
            if !self.is_inline_by_adjacency(doc, field_idx, &text_groups)
                && !self.is_contained_in_text_block(doc, field_idx, &text_groups)
            {
                continue;
            }

            let Some(field_bounds) = doc.get_bounds(field_idx) else {
                continue;
            };

            let mut before = Vec::new();
            let mut after = Vec::new();

            for &text_idx in &text_groups {
                let Some(text_bounds) = doc.get_bounds(text_idx) else {
                    continue;
                };
                let text_content = doc.get_text_content(text_idx);
                let trimmed = text_content.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Only collect text that is long enough to look like flowing prose.
                // Short text (< MIN_INLINE_TEXT_CHARS) is likely a label for another
                // field and should stay available for the LabelAttacher.
                if trimmed.chars().count() < Self::MIN_INLINE_TEXT_CHARS {
                    continue;
                }

                // Only collect text on the same line as the field (left or right).
                // By definition, inline fields have no label above/below,
                // so we only look at horizontally adjacent or overlapping text.
                if self.has_text_left(&text_bounds, &field_bounds) {
                    before.push(text_idx);
                } else if self.has_text_right(&text_bounds, &field_bounds) {
                    after.push(text_idx);
                }
                // For containment detection: text on same line whose center is left goes before,
                // otherwise after.
                else if text_bounds.is_on_same_line(&field_bounds, self.line_tolerance)
                    && field_bounds.overlaps_horizontally(&text_bounds, self.line_tolerance)
                {
                    let text_center_x = text_bounds.x + text_bounds.width / Decimal::TWO;
                    let field_center_x = field_bounds.x + field_bounds.width / Decimal::TWO;
                    if text_center_x < field_center_x {
                        before.push(text_idx);
                    } else {
                        after.push(text_idx);
                    }
                }
            }

            inline_infos.push(InlineFieldInfo {
                field_idx,
                before,
                after,
            });
        }

        // Create inline field groups — collect first, then mutate doc
        for info in inline_infos {
            doc.add_inline_field(info.field_idx, info.before, info.after);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{AnalysisModule, FieldGrouper, TextBlockGrouper};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_inline_field_with_text_left() {
        // Field with text to the left, no text above/below
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "Enter name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(60.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(75.0),
                    num(98.0),
                    num(100.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        InlineFieldDetector::new().process(&mut doc);

        // Should have detected the inline field
        let inline_fields = doc.inline_fields();
        assert_eq!(inline_fields.len(), 1);
    }

    #[test]
    fn test_inline_field_with_text_right() {
        // Field with text to the right, no text above/below
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(10.0),
                    num(98.0),
                    num(100.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "is required".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(115.0),
                    num(100.0),
                    num(60.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        InlineFieldDetector::new().process(&mut doc);

        // Should have detected the inline field
        let inline_fields = doc.inline_fields();
        assert_eq!(inline_fields.len(), 1);
    }

    #[test]
    fn test_not_inline_when_label_above() {
        // Field with text to the left AND text above (should NOT be inline)
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Label above
                FlattenedNode::new_text(
                    "Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(75.0),
                    num(80.0),
                    num(40.0),
                    num(12.0),
                ),
                // Text to the left
                FlattenedNode::new_text(
                    "Enter:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(40.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(75.0),
                    num(98.0),
                    num(100.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        InlineFieldDetector::new().process(&mut doc);

        // Should NOT be inline (has label above)
        let inline_fields = doc.inline_fields();
        assert_eq!(inline_fields.len(), 0);
    }

    #[test]
    fn test_not_inline_without_adjacent_text() {
        // Field alone, no text nearby
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(200.0),
                    num(200.0),
                    num(100.0),
                    num(20.0),
                ),
                // Text far away
                FlattenedNode::new_text(
                    "Some text".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(500.0),
                    num(60.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        InlineFieldDetector::new().process(&mut doc);

        // Should NOT be inline (no adjacent text)
        let inline_fields = doc.inline_fields();
        assert_eq!(inline_fields.len(), 0);
    }

    #[test]
    fn test_inline_field_contained_in_text_block() {
        // Field that overlaps horizontally with text on the same line
        // This simulates a field embedded within flowing text
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Text that spans a wide area
                FlattenedNode::new_text(
                    "Please enter your".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(12.0),
                ),
                // Field embedded in the text flow (overlapping horizontal range)
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(50.0),
                    num(98.0),
                    num(60.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        InlineFieldDetector::new().process(&mut doc);

        // Should be inline (field overlaps with text on same line)
        let inline_fields = doc.inline_fields();
        assert_eq!(inline_fields.len(), 1);
    }

    #[test]
    fn test_not_inline_when_label_above_and_text_overlapping() {
        // Field with label above AND text overlapping on the same line
        // This should NOT be inline because it has a label above
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Label ABOVE the field (vertically aligned)
                FlattenedNode::new_text(
                    "Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(50.0),
                    num(75.0),
                    num(40.0),
                    num(12.0),
                ),
                // Text on the SAME LINE as field that overlaps horizontally
                FlattenedNode::new_text(
                    "required".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(35.0),
                    num(12.0),
                ),
                // Field with label above
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(50.0),
                    num(98.0),
                    num(100.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        InlineFieldDetector::new().process(&mut doc);

        // Should NOT be inline (has label above, even though text overlaps on same line)
        let inline_fields = doc.inline_fields();
        assert_eq!(inline_fields.len(), 0);
    }
}
