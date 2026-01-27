//! Radio button detector module.
//!
//! Detects radio buttons based on their characteristics:
//! - Square fields (width == height)
//! - Labeled on the right side
//! - Typically small (checkbox/radio button size)

use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use super::AnalysisModule;
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
    
    /// Check if a field is small enough to be a radio button.
    fn is_radio_size(&self, width: Decimal, height: Decimal) -> bool {
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
        text_candidates: &[usize]
    ) -> Option<usize> {
        let field_bounds = doc.get_bounds(field_idx)?;
        
        let mut best: Option<(usize, Decimal)> = None;
        
        for &text_idx in text_candidates {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };
            
            if let Some(gap) = self.is_label_on_right(&field_bounds, &text_bounds) {
                if best.map(|(_, best_gap)| gap < best_gap).unwrap_or(true) {
                    best = Some((text_idx, gap));
                }
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
        let field_groups: Vec<usize> = roots.iter()
            .filter(|&&idx| doc.is_field(idx))
            .copied()
            .collect();
        
        let text_groups: Vec<usize> = roots.iter()
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
            
            // Must be square and small
            if !self.is_square(bounds.width, bounds.height) || !self.is_radio_size(bounds.width, bounds.height) {
                continue;
            }
            
            // Find label on the right
            let available_labels: Vec<_> = text_groups.iter()
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
                GroupSource::Inferred { module: self.name().to_string() },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind, Page};
    use crate::xfa::num;
    
    #[test]
    fn test_is_square() {
        let detector = RadioButtonDetector::new();
        
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
    fn test_radio_button_detection() {
        // Create a flattened document with a square field and label on right
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Small square field at (50, 100)
                FlattenedNode {
                    kind: FlattenedNodeKind::Field {
                        name: "radio1".to_string(),
                        value: "".to_string(),
                        label: "".to_string(),
                    },
                    x: num(50.0),
                    y: num(100.0),
                    width: num(12.0),
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
                // Text label to the right
                FlattenedNode {
                    kind: FlattenedNodeKind::Text {
                        content: "Option A".to_string(),
                        font_size: num(10.0),
                        font_name: "Helvetica".to_string(),
                        source_name: None,
                        rich_text: None,
                    },
                    x: num(65.0), // 3 points gap from field
                    y: num(100.0),
                    width: num(50.0),
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        
        // Process with FieldGrouper first
        use crate::modules::{FieldGrouper, TextBlockGrouper};
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
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Rectangular field
                FlattenedNode {
                    kind: FlattenedNodeKind::Field {
                        name: "text_field".to_string(),
                        value: "".to_string(),
                        label: "".to_string(),
                    },
                    x: num(50.0),
                    y: num(100.0),
                    width: num(100.0), // Wide
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
                // Text label to the right
                FlattenedNode {
                    kind: FlattenedNodeKind::Text {
                        content: "Name:".to_string(),
                        font_size: num(10.0),
                        font_name: "Helvetica".to_string(),
                        source_name: None,
                        rich_text: None,
                    },
                    x: num(155.0),
                    y: num(100.0),
                    width: num(30.0),
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        
        use crate::modules::{FieldGrouper, TextBlockGrouper};
        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        RadioButtonDetector::new().process(&mut doc);
        
        // Should NOT have created a RadioButton group (field is not square)
        let radio_buttons = doc.find_groups(|k| matches!(k, GroupKind::RadioButton { .. }));
        assert_eq!(radio_buttons.len(), 0);
    }
}
