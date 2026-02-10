//! Checkbox detector module.
//!
//! Detects checkboxes based on their characteristics:
//! - Square fields (width == height)
//! - Labeled on the right side
//! - Typically small (checkbox size)
//! - Has WidgetType hint indicating Checkbox

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Bounds, FlattenedNodeKind, Hint, WidgetKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects checkboxes by identifying square fields with labels on the right.
///
/// Checkboxes are characterized by:
/// 1. Being Field groups (not text)
/// 2. Having equal width and height (square)
/// 3. Having a text label positioned to the right
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

    /// Check if a field is square (width ≈ height).
    fn is_square(&self, width: Decimal, height: Decimal) -> bool {
        if width.is_zero() || height.is_zero() {
            return false;
        }

        let ratio = if width > height {
            width / height
        } else {
            height / width
        };

        // ratio should be close to 1.0
        let diff = (ratio - Decimal::ONE).abs();
        diff <= self.square_tolerance
    }

    /// Check if a field is small enough to be a checkbox.
    fn is_checkbox_size(&self, width: Decimal, height: Decimal) -> bool {
        width <= self.max_size && height <= self.max_size
    }

    /// Check if text is to the right of the field and on the same line.
    fn is_label_on_right(&self, field_bounds: &Bounds, text_bounds: &Bounds) -> Option<Decimal> {
        // Text must be to the right of field
        let gap = field_bounds.horizontal_gap_to(text_bounds)?;

        if gap > self.max_label_distance {
            return None;
        }

        // Check vertical alignment (same line)
        if !field_bounds.is_on_same_line(text_bounds, self.line_tolerance) {
            return None;
        }

        Some(gap)
    }

    /// Find the best label on the right of a field.
    fn find_label_on_right(
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

            if let Some(gap) = self.is_label_on_right(&field_bounds, &text_bounds)
                && best.map(|(_, best_gap)| gap < best_gap).unwrap_or(true)
            {
                best = Some((text_idx, gap));
            }
        }

        best.map(|(idx, _)| idx)
    }

    /// Check if a field has a Checkbox widget type hint.
    fn is_checkbox_field(&self, doc: &Document, field_idx: usize) -> bool {
        // Get the leaf node indices from this group
        let node_indices = doc.collect_node_indices(field_idx);

        // Check if any node has a Checkbox widget type hint
        for &node_idx in &node_indices {
            if let Some(node) = doc.get_node(node_idx) {
                if node
                    .hints
                    .iter()
                    .any(|hint| matches!(hint, Hint::WidgetType(WidgetKind::Checkbox)))
                {
                    return true;
                }
            }
        }
        false
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
            if !self.is_square(bounds.width, bounds.height)
                || !self.is_checkbox_size(bounds.width, bounds.height)
            {
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

            if let Some(label_idx) = self.find_label_on_right(doc, field_idx, &available_labels) {
                checkboxes.push((field_idx, label_idx));
                used_labels.insert(label_idx);
            }
        }

        // Create Checkbox groups
        for (field_idx, label_idx) in checkboxes {
            doc.merge(
                vec![field_idx, label_idx],
                GroupKind::Checkbox { field: 0, label: 1 },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind, Hint, Page, WidgetKind};
    use crate::xfa::num;

    #[test]
    fn test_is_square() {
        let detector = CheckboxDetector::new();

        // Perfect square
        assert!(detector.is_square(num(10.0), num(10.0)));

        // Within tolerance (10%)
        assert!(detector.is_square(num(10.0), num(10.5)));
        assert!(detector.is_square(num(10.5), num(10.0)));

        // Outside tolerance
        assert!(!detector.is_square(num(10.0), num(12.0)));
        assert!(!detector.is_square(num(10.0), num(20.0)));
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
            Page {
                width: num(595.0),
                height: num(842.0),
            },
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
