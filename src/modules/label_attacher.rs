//! Label attacher module.
//!
//! Associates text labels with their corresponding fields to create
//! LabeledField groups.
//!
//! Uses statistical analysis to determine the dominant label position
//! (above, below, or left of fields) based on the document layout.

use crate::document::{Document, GroupKind, GroupSource};
use super::AnalysisModule;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashMap;

/// The position of a label relative to its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelPosition {
    Above,
    Below,
    Left,
}

/// Attaches labels to fields based on spatial relationships.
///
/// Uses statistical analysis to determine the dominant label position:
/// 1. Analyzes all text-field spatial relationships
/// 2. Determines which position (above, below, left) is most common
/// 3. Uses that position to match labels to fields
pub struct LabelAttacher {
    /// Maximum vertical distance for label above/below field
    pub vertical_threshold: Decimal,
    /// Maximum horizontal distance for label to left of field
    pub horizontal_threshold: Decimal,
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
}

impl Default for LabelAttacher {
    fn default() -> Self {
        Self::new()
    }
}

impl LabelAttacher {
    pub fn new() -> Self {
        LabelAttacher {
            vertical_threshold: Decimal::from_str("30.0").unwrap(),
            horizontal_threshold: Decimal::from_str("150.0").unwrap(),
            line_tolerance: Decimal::from_str("8.0").unwrap(),
        }
    }
    
    /// Configure the vertical threshold.
    pub fn with_vertical_threshold(mut self, threshold: Decimal) -> Self {
        self.vertical_threshold = threshold;
        self
    }
    
    /// Configure the horizontal threshold.
    pub fn with_horizontal_threshold(mut self, threshold: Decimal) -> Self {
        self.horizontal_threshold = threshold;
        self
    }
    
    /// Check if text is above the field and return the gap distance.
    fn check_above(&self, 
        text_bounds: (Decimal, Decimal, Decimal, Decimal),
        field_bounds: (Decimal, Decimal, Decimal, Decimal)
    ) -> Option<Decimal> {
        let (text_x, text_y, text_w, text_h) = text_bounds;
        let (field_x, field_y, field_w, _field_h) = field_bounds;
        
        let text_bottom = text_y + text_h;
        
        // Text must be above field
        if text_bottom > field_y {
            return None;
        }
        
        let gap = field_y - text_bottom;
        if gap > self.vertical_threshold {
            return None;
        }
        
        // Check horizontal alignment
        let text_right = text_x + text_w;
        let field_right = field_x + field_w;
        
        // Text should overlap horizontally with field
        if text_right < field_x - self.line_tolerance || text_x > field_right + self.line_tolerance {
            return None;
        }
        
        Some(gap)
    }
    
    /// Check if text is below the field and return the gap distance.
    fn check_below(&self,
        text_bounds: (Decimal, Decimal, Decimal, Decimal),
        field_bounds: (Decimal, Decimal, Decimal, Decimal)
    ) -> Option<Decimal> {
        let (text_x, text_y, text_w, _text_h) = text_bounds;
        let (field_x, field_y, field_w, field_h) = field_bounds;
        
        let field_bottom = field_y + field_h;
        
        // Text must be below field
        if text_y < field_bottom {
            return None;
        }
        
        let gap = text_y - field_bottom;
        if gap > self.vertical_threshold {
            return None;
        }
        
        // Check horizontal alignment
        let text_right = text_x + text_w;
        let field_right = field_x + field_w;
        
        if text_right < field_x - self.line_tolerance || text_x > field_right + self.line_tolerance {
            return None;
        }
        
        Some(gap)
    }
    
    /// Check if text is to the left of the field and return the gap distance.
    fn check_left(&self,
        text_bounds: (Decimal, Decimal, Decimal, Decimal),
        field_bounds: (Decimal, Decimal, Decimal, Decimal)
    ) -> Option<Decimal> {
        let (text_x, text_y, text_w, text_h) = text_bounds;
        let (field_x, field_y, _field_w, field_h) = field_bounds;
        
        let text_right = text_x + text_w;
        
        // Text must be to the left of field
        if text_right > field_x {
            return None;
        }
        
        let gap = field_x - text_right;
        if gap > self.horizontal_threshold {
            return None;
        }
        
        // Check vertical alignment (same line)
        let text_center_y = text_y + text_h / Decimal::TWO;
        let field_center_y = field_y + field_h / Decimal::TWO;
        let y_diff = (text_center_y - field_center_y).abs();
        
        let max_y_diff = (text_h.max(field_h) / Decimal::TWO) + self.line_tolerance;
        
        if y_diff > max_y_diff {
            return None;
        }
        
        Some(gap)
    }
    
    /// Analyze all text-field relationships and determine the dominant label position.
    fn analyze_label_positions(&self, doc: &Document, text_groups: &[usize], field_groups: &[usize]) -> Option<LabelPosition> {
        let mut position_counts: HashMap<LabelPosition, usize> = HashMap::new();
        
        for &field_idx in field_groups {
            let Some(field_bounds) = doc.get_bounds(field_idx) else {
                continue;
            };
            
            for &text_idx in text_groups {
                let Some(text_bounds) = doc.get_bounds(text_idx) else {
                    continue;
                };
                
                // Check each position
                if self.check_above(text_bounds, field_bounds).is_some() {
                    *position_counts.entry(LabelPosition::Above).or_insert(0) += 1;
                }
                if self.check_below(text_bounds, field_bounds).is_some() {
                    *position_counts.entry(LabelPosition::Below).or_insert(0) += 1;
                }
                if self.check_left(text_bounds, field_bounds).is_some() {
                    *position_counts.entry(LabelPosition::Left).or_insert(0) += 1;
                }
            }
        }
        
        // Find the dominant position
        position_counts.into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(pos, _)| pos)
    }
    
    /// Find the best label for a field at the given position.
    fn find_label_at_position(&self, 
        doc: &Document, 
        field_idx: usize, 
        text_candidates: &[usize],
        position: LabelPosition
    ) -> Option<(usize, Decimal)> {
        let field_bounds = doc.get_bounds(field_idx)?;
        
        let mut best: Option<(usize, Decimal)> = None;
        
        for &text_idx in text_candidates {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };
            
            let gap = match position {
                LabelPosition::Above => self.check_above(text_bounds, field_bounds),
                LabelPosition::Below => self.check_below(text_bounds, field_bounds),
                LabelPosition::Left => self.check_left(text_bounds, field_bounds),
            };
            
            if let Some(g) = gap {
                if best.map(|(_, best_gap)| g < best_gap).unwrap_or(true) {
                    best = Some((text_idx, g));
                }
            }
        }
        
        best
    }
}

impl AnalysisModule for LabelAttacher {
    fn name(&self) -> &'static str {
        "LabelAttacher"
    }
    
    fn process(&self, doc: &mut Document) {
        // Get all root groups
        let roots = doc.roots();
        
        // Find TextBlock groups and Field groups
        let text_groups: Vec<usize> = roots.iter()
            .filter(|&&idx| doc.is_text_block(idx))
            .copied()
            .collect();
        
        let field_groups: Vec<usize> = roots.iter()
            .filter(|&&idx| doc.is_field(idx))
            .copied()
            .collect();
        
        if text_groups.is_empty() || field_groups.is_empty() {
            return;
        }
        
        // Step 1: Statistical analysis - determine dominant label position
        let Some(dominant_position) = self.analyze_label_positions(doc, &text_groups, &field_groups) else {
            return;
        };
        
        // Step 2: Match labels to fields using the dominant position
        let mut pairs: Vec<(usize, usize)> = Vec::new(); // (label_idx, field_idx)
        let mut used_labels: std::collections::HashSet<usize> = std::collections::HashSet::new();
        
        // Sort fields by position for consistent processing
        let mut sorted_fields = field_groups.clone();
        sorted_fields.sort_by(|&a, &b| {
            let bounds_a = doc.get_bounds(a);
            let bounds_b = doc.get_bounds(b);
            match (bounds_a, bounds_b) {
                (Some((x_a, y_a, _, _)), Some((x_b, y_b, _, _))) => {
                    y_a.cmp(&y_b).then_with(|| x_a.cmp(&x_b))
                }
                _ => std::cmp::Ordering::Equal,
            }
        });
        
        for field_idx in sorted_fields {
            // Filter out already-used labels
            let available_labels: Vec<_> = text_groups.iter()
                .filter(|idx| !used_labels.contains(idx))
                .copied()
                .collect();
            
            if let Some((label_idx, _gap)) = self.find_label_at_position(doc, field_idx, &available_labels, dominant_position) {
                pairs.push((label_idx, field_idx));
                used_labels.insert(label_idx);
            }
        }
        
        // Step 3: Create LabeledField groups
        for (label_idx, field_idx) in pairs {
            doc.merge(
                vec![label_idx, field_idx],
                GroupKind::LabeledField { label: 0, field: 1 },
                GroupSource::Inferred { module: self.name().to_string() },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;
    use crate::modules::{TextBlockGrouper, FieldGrouper, AnalysisModule};
    
    #[test]
    fn test_label_above_field() {
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Labels above fields (this pattern should be detected as dominant)
                FlattenedNode::new_text(
                    "First Name:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(100.0), num(60.0), num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_FirstName".to_string(), "".to_string(), "First Name".to_string(),
                    num(10.0), num(115.0), num(150.0), num(20.0),
                ),
                FlattenedNode::new_text(
                    "Last Name:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(150.0), num(60.0), num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_LastName".to_string(), "".to_string(), "Last Name".to_string(),
                    num(10.0), num(165.0), num(150.0), num(20.0),
                ),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);
        
        // Should have created LabeledFields
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 2);
    }
    
    #[test]
    fn test_label_left_of_field() {
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Labels to left of fields (this pattern should be detected as dominant)
                FlattenedNode::new_text(
                    "Name:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(100.0), num(35.0), num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_Name".to_string(), "".to_string(), "Name".to_string(),
                    num(60.0), num(98.0), num(150.0), num(20.0),
                ),
                FlattenedNode::new_text(
                    "Email:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(130.0), num(35.0), num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_Email".to_string(), "".to_string(), "Email".to_string(),
                    num(60.0), num(128.0), num(150.0), num(20.0),
                ),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);
        
        // Should have created LabeledFields
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 2);
        
        // Check that the right labels are attached
        assert_eq!(doc.get_label_text(labeled[0]), Some("Name:".to_string()));
        assert_eq!(doc.get_label_text(labeled[1]), Some("Email:".to_string()));
    }
    
    #[test]
    fn test_statistical_analysis_chooses_dominant_position() {
        // Mix of positions, but "above" should dominate (3 above vs 1 left)
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Three labels above
                FlattenedNode::new_text("A:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(50.0), num(20.0), num(12.0)),
                FlattenedNode::new_field("F_A".to_string(), "".to_string(), "A".to_string(),
                    num(10.0), num(65.0), num(100.0), num(20.0)),
                
                FlattenedNode::new_text("B:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(100.0), num(20.0), num(12.0)),
                FlattenedNode::new_field("F_B".to_string(), "".to_string(), "B".to_string(),
                    num(10.0), num(115.0), num(100.0), num(20.0)),
                    
                FlattenedNode::new_text("C:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(150.0), num(20.0), num(12.0)),
                FlattenedNode::new_field("F_C".to_string(), "".to_string(), "C".to_string(),
                    num(10.0), num(165.0), num(100.0), num(20.0)),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);
        
        // All 3 should be labeled
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 3);
    }
    
    #[test]
    fn test_no_label_for_distant_field() {
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Text at top
                FlattenedNode::new_text(
                    "Title".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(10.0), num(30.0), num(12.0),
                ),
                // Field far below (too far to be associated)
                FlattenedNode::new_field(
                    "SomeField".to_string(), "".to_string(), "Some Field".to_string(),
                    num(10.0), num(500.0), num(150.0), num(20.0),
                ),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);
        
        // Should NOT create a LabeledField (too far apart)
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 0);
    }
}
