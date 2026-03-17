//! Radio button detector module.
//!
//! Detects radio buttons based on their characteristics:
//! - Square fields (width == height)
//! - Labeled on the right side
//! - Typically small (checkbox/radio button size)

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Bounds, Hint, WidgetKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects radio buttons by identifying square fields with labels on the right.
///
/// Radio buttons are characterized by:
/// 1. Being Field groups (not text)
/// 2. Having equal width and height (square)
/// 3. Having a text label positioned to the right
/// 4. Being relatively small (typical checkbox/radio size)
pub struct RadioButtonDetector {
    /// Maximum size for a radio button (width/height in points)
    pub max_size: Decimal,
    /// Tolerance for width/height equality (as ratio)
    pub square_tolerance: Decimal,
    /// Maximum horizontal distance between field and label
    pub max_label_distance: Decimal,
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
    /// How far the label may start inside the field's right edge (in points).
    /// XFA forms sometimes position the caption text slightly overlapping the
    /// radio-button circle widget.  A small positive tolerance (default 5 pt)
    /// handles such cases without being overly permissive.
    pub label_overlap_tolerance: Decimal,
}

impl Default for RadioButtonDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl RadioButtonDetector {
    pub fn new() -> Self {
        RadioButtonDetector {
            max_size: Decimal::from_str("25.0").unwrap(), // 25 points max
            square_tolerance: Decimal::from_str("0.1").unwrap(), // 10% tolerance
            max_label_distance: Decimal::from_str("150.0").unwrap(),
            line_tolerance: Decimal::from_str("8.0").unwrap(),
            label_overlap_tolerance: Decimal::from_str("5.0").unwrap(),
        }
    }

    /// Configure the maximum size for radio buttons.
    pub fn with_max_size(mut self, size: Decimal) -> Self {
        self.max_size = size;
        self
    }

    /// Configure the square tolerance ratio.
    pub fn with_square_tolerance(mut self, tolerance: Decimal) -> Self {
        self.square_tolerance = tolerance;
        self
    }

    /// Check if a field has an EXPLICIT `WidgetType(Radio)` hint.
    /// Returns true only when the hint is present; false when absent or Checkbox.
    fn has_explicit_radio_hint(&self, doc: &Document, field_idx: usize) -> bool {
        let node_indices = doc.collect_node_indices(field_idx);
        for &node_idx in &node_indices {
            if let Some(node) = doc.get_node(node_idx) {
                for hint in &node.hints {
                    if let Hint::WidgetType(WidgetKind::Radio) = hint {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if a field has a Radio widget type hint (and NOT Checkbox).
    /// If no widget hint is present, assume it could be a radio button (legacy behavior).
    /// But explicitly reject fields with Checkbox widget hint.
    fn is_radio_field(&self, doc: &Document, field_idx: usize) -> bool {
        let node_indices = doc.collect_node_indices(field_idx);

        for &node_idx in &node_indices {
            if let Some(node) = doc.get_node(node_idx) {
                // Check for explicit widget type
                for hint in &node.hints {
                    match hint {
                        Hint::WidgetType(WidgetKind::Checkbox) => {
                            // Explicitly a checkbox - not a radio button
                            return false;
                        }
                        Hint::WidgetType(WidgetKind::Radio) => {
                            // Explicitly a radio button
                            return true;
                        }
                        _ => {}
                    }
                }
            }
        }

        // No explicit widget type found - could be radio (legacy behavior)
        // But we'll be conservative and return true to maintain backward compatibility
        true
    }

    /// Check if text is to the right of the field and on the same line.
    fn is_label_on_right(&self, field_bounds: &Bounds, text_bounds: &Bounds) -> Option<Decimal> {
        // Text must be to the right of field (allowing a small overlap for XFA forms
        // that position the caption text slightly inside the button widget's right edge).
        let gap = if text_bounds.x >= field_bounds.right() - self.label_overlap_tolerance {
            // Gap is positive when text is clear of the field; negative means slight overlap.
            // We normalise to 0 when within tolerance so the sorting still works.
            let raw = text_bounds.x - field_bounds.right();
            if raw < Decimal::ZERO {
                Decimal::ZERO
            } else {
                raw
            }
        } else {
            return None;
        };

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
}

impl AnalysisModule for RadioButtonDetector {
    fn name(&self) -> &'static str {
        "RadioButtonDetector"
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

        // Identify radio buttons and create RadioButton groups
        let mut radio_buttons: Vec<(usize, usize)> = Vec::new(); // (field_idx, label_idx)
        let mut used_labels: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for field_idx in field_groups {
            // Check if this field looks like a radio button
            let Some(bounds) = doc.get_bounds(field_idx) else {
                continue;
            };

            // If the field has an explicit WidgetType(Radio) hint we trust the form
            // author's intent and skip the heuristic square/size checks.  Only for
            // fields without an explicit hint do we fall back to shape-based detection.
            let is_explicit_radio = self.has_explicit_radio_hint(doc, field_idx);

            if !is_explicit_radio {
                // Must be square and small for heuristic detection
                if !bounds.is_square(self.square_tolerance)
                    || !bounds.fits_within_size(self.max_size)
                {
                    continue;
                }
            }

            // Must be a radio button (not a checkbox) based on widget type hint
            if !self.is_radio_field(doc, field_idx) {
                continue;
            }

            // Find label on the right
            let available_labels: Vec<_> = text_groups
                .iter()
                .filter(|idx| !used_labels.contains(idx))
                .copied()
                .collect();

            if let Some(label_idx) = self.find_label_on_right(doc, field_idx, &available_labels) {
                radio_buttons.push((field_idx, label_idx));
                used_labels.insert(label_idx);
            }
        }

        // Create RadioButton groups
        for (field_idx, label_idx) in radio_buttons {
            doc.merge(
                vec![field_idx, label_idx],
                GroupKind::RadioButton { field: 0, label: 1 },
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
    use crate::flattened::{Bounds, Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_is_square() {
        let detector = RadioButtonDetector::new();

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
    fn test_radio_button_detection() {
        // Create a flattened document with a square field and label on right
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Small square field at (50, 100)
                FlattenedNode::new_field(
                    "radio1".to_string(),
                    "".to_string(),
                    "".to_string(),
                    num(50.0),
                    num(100.0),
                    num(12.0),
                    num(12.0),
                ),
                // Text label to the right
                FlattenedNode::new_text(
                    "Option A".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(65.0),
                    num(100.0),
                    num(50.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        // Process with FieldGrouper first
        use crate::document::modules::{FieldGrouper, TextBlockGrouper};
        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);

        // Now detect radio buttons
        RadioButtonDetector::new().process(&mut doc);

        // Should have created a RadioButton group
        let radio_buttons = doc.find_groups(|k| matches!(k, GroupKind::RadioButton { .. }));
        assert_eq!(radio_buttons.len(), 1);
    }

    #[test]
    fn test_non_square_field_not_detected() {
        // Create a non-square field
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Rectangular field
                FlattenedNode::new_field(
                    "text_field".to_string(),
                    "".to_string(),
                    "".to_string(),
                    num(50.0),
                    num(100.0),
                    num(100.0),
                    num(12.0),
                ),
                // Text label to the right
                FlattenedNode::new_text(
                    "Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(155.0),
                    num(100.0),
                    num(30.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        use crate::document::modules::{FieldGrouper, TextBlockGrouper};
        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        RadioButtonDetector::new().process(&mut doc);

        // Should NOT have created a RadioButton group (field is not square)
        let radio_buttons = doc.find_groups(|k| matches!(k, GroupKind::RadioButton { .. }));
        assert_eq!(radio_buttons.len(), 0);
    }
}
