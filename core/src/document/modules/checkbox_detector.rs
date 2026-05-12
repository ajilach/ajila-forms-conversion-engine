//! Checkbox detector module.
//!
//! Detects checkboxes based on their characteristics:
//! - Square fields (width == height)
//! - Labeled on the right side, OR overlapping label that starts at same x position
//! - Typically small (checkbox size)
//! - Has WidgetType hint indicating Checkbox

use super::AnalysisModule;
use super::radio_button_detector::find_best_label_on_right;
use crate::document::{Document, GroupKind};
use crate::flattened::{Bounds, WidgetKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects checkboxes by identifying square fields with labels on the right.
///
/// Checkboxes are characterized by:
/// 1. Being Field groups (not text)
/// 2. Having equal width and height (square)
/// 3. Having a text label positioned to the right OR overlapping (starting at same x)
/// 4. Being relatively small (typical checkbox size)
/// 5. Having a WidgetType(Checkbox) hint (from XFA ui/checkButton with square shape)
pub struct CheckboxDetector {
    /// Maximum size for a checkbox (width/height in points)
    pub max_size: Decimal,
    /// Tolerance for width/height equality (as ratio)
    pub square_tolerance: Decimal,
    /// Maximum horizontal distance between field and label
    pub max_label_distance: Decimal,
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
    /// Tolerance for overlapping label x-position matching
    pub x_position_tolerance: Decimal,
}

impl Default for CheckboxDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckboxDetector {
    pub fn new() -> Self {
        CheckboxDetector {
            max_size: Decimal::from_str("25.0").unwrap(), // 25 points max
            square_tolerance: Decimal::from_str("0.1").unwrap(), // 10% tolerance
            max_label_distance: Decimal::from_str("150.0").unwrap(),
            line_tolerance: Decimal::from_str("8.0").unwrap(),
            x_position_tolerance: Decimal::from_str("2.0").unwrap(), // 2pt tolerance for x matching
        }
    }

    /// Configure the maximum size for checkboxes.
    pub fn with_max_size(mut self, size: Decimal) -> Self {
        self.max_size = size;
        self
    }

    /// Configure the square tolerance ratio.
    pub fn with_square_tolerance(mut self, tolerance: Decimal) -> Self {
        self.square_tolerance = tolerance;
        self
    }

    /// Check if a field has a Checkbox widget type hint.
    fn is_checkbox_field(&self, doc: &Document, field_idx: usize) -> bool {
        doc.widget_kind(field_idx) == Some(WidgetKind::Checkbox)
    }

    /// Find an overlapping label for a checkbox.
    ///
    /// Handles the case where the label text starts at approximately the same
    /// x position as the checkbox (within tolerance) and extends beyond it.
    /// This is common in XFA forms where the checkbox caption overlaps with
    /// or starts at the same position as the checkbox widget.
    ///
    /// Returns the index of the best overlapping label, or None if none found.
    fn find_overlapping_label(
        &self,
        doc: &Document,
        field_idx: usize,
        text_candidates: &[usize],
    ) -> Option<usize> {
        let field_bounds = doc.get_bounds(field_idx)?;

        let mut best: Option<(usize, Decimal)> = None;

        for &text_idx in text_candidates {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };

            // Check if text starts at approximately the same x position as the checkbox
            let x_diff = (text_bounds.x - field_bounds.x).abs();
            if x_diff > self.x_position_tolerance {
                continue;
            }

            // Text must extend beyond the checkbox's right edge
            if text_bounds.right() <= field_bounds.right() {
                continue;
            }

            // Must be on the same line (vertical alignment)
            if !self.is_on_same_line(&field_bounds, &text_bounds) {
                continue;
            }

            // Prefer text with smaller x_diff (closer to exact alignment)
            if best
                .map(|(_, best_diff)| x_diff < best_diff)
                .unwrap_or(true)
            {
                best = Some((text_idx, x_diff));
            }
        }

        best.map(|(idx, _)| idx)
    }

    /// Check if two bounds are on the same line (vertical alignment within tolerance).
    fn is_on_same_line(&self, a: &Bounds, b: &Bounds) -> bool {
        a.is_on_same_line(b, self.line_tolerance)
    }
}

impl AnalysisModule for CheckboxDetector {
    fn name(&self) -> &'static str {
        "CheckboxDetector"
    }

    fn process(&self, doc: &mut Document) {
        // Get all root groups
        let roots = doc.roots();

        // Find Field groups and TextBlock groups
        let field_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| doc.is_field(idx))
            .copied()
            .collect();

        let text_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| doc.is_text_block(idx))
            .copied()
            .collect();

        if field_groups.is_empty() || text_groups.is_empty() {
            return;
        }

        // Identify checkboxes and create Checkbox groups
        let mut checkboxes: Vec<(usize, usize)> = Vec::new(); // (field_idx, label_idx)
        let mut used_labels: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for field_idx in field_groups {
            // Check if this field looks like a checkbox
            let Some(bounds) = doc.get_bounds(field_idx) else {
                continue;
            };

            // Must be square and small
            if !bounds.is_square(self.square_tolerance) || !bounds.fits_within_size(self.max_size) {
                continue;
            }

            // Must have Checkbox widget type hint
            if !self.is_checkbox_field(doc, field_idx) {
                continue;
            }

            // Find label on the right
            let available_labels: Vec<_> = text_groups
                .iter()
                .filter(|idx| !used_labels.contains(idx))
                .copied()
                .collect();

            // First try finding a label to the right of the checkbox
            let label_idx = find_best_label_on_right(
                doc,
                field_idx,
                &available_labels,
                self.max_label_distance,
                self.line_tolerance,
                Decimal::ZERO,
                false,
            )
            // If no label to the right, try finding an overlapping label
            // (text that starts at the same x position and extends beyond the checkbox)
            .or_else(|| self.find_overlapping_label(doc, field_idx, &available_labels));

            if let Some(label_idx) = label_idx {
                checkboxes.push((field_idx, label_idx));
                used_labels.insert(label_idx);
            }
        }

        // Create Checkbox groups
        for (field_idx, label_idx) in checkboxes {
            doc.merge_inferred(
                vec![field_idx, label_idx],
                GroupKind::Checkbox { field: 0, label: 1 },
                self.name(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Bounds, Flattened, FlattenedNode, Hint, Page, WidgetKind};
    use crate::xfa::num;

    #[test]
    fn test_is_square() {
        let detector = CheckboxDetector::new();

        // Perfect square
        assert!(
            Bounds::new(num(0.0), num(0.0), num(10.0), num(10.0))
                .is_square(detector.square_tolerance)
        );

        // Within tolerance (10%)
        assert!(
            Bounds::new(num(0.0), num(0.0), num(10.0), num(10.5))
                .is_square(detector.square_tolerance)
        );
        assert!(
            Bounds::new(num(0.0), num(0.0), num(10.5), num(10.0))
                .is_square(detector.square_tolerance)
        );

        // Outside tolerance
        assert!(
            !Bounds::new(num(0.0), num(0.0), num(10.0), num(12.0))
                .is_square(detector.square_tolerance)
        );
        assert!(
            !Bounds::new(num(0.0), num(0.0), num(10.0), num(20.0))
                .is_square(detector.square_tolerance)
        );
    }

    #[test]
    fn test_checkbox_detection() {
        // Create a flattened document with a square field and label on right
        let mut checkbox_node = FlattenedNode::new_field(
            "checkbox1".to_string(),
            "".to_string(),
            "".to_string(),
            num(100.0),
            num(100.0),
            num(10.0),
            num(10.0),
        );
        checkbox_node.add_hint(Hint::WidgetType(WidgetKind::Checkbox));

        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                // Small square field at (100, 100) with Checkbox widget hint
                checkbox_node,
                // Text label to the right
                FlattenedNode::new_text(
                    "Accept terms".to_string(),
                    num(12.0),
                    "Arial".to_string(),
                    num(115.0),
                    num(100.0),
                    num(80.0),
                    num(10.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        // Process with FieldGrouper first
        use crate::document::modules::{FieldGrouper, TextBlockGrouper};
        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);

        // Now detect checkboxes
        CheckboxDetector::new().process(&mut doc);

        // Should have created a Checkbox group
        let checkboxes = doc.find_groups(|k| matches!(k, GroupKind::Checkbox { .. }));
        assert_eq!(checkboxes.len(), 1, "Should detect one checkbox");

        // Verify structure
        let checkbox_group = doc.get_group(checkboxes[0]).unwrap();
        if let GroupKind::Checkbox { field, label } = checkbox_group.kind {
            assert_eq!(field, 0);
            assert_eq!(label, 1);
        } else {
            panic!("Expected Checkbox kind");
        }
    }
}
