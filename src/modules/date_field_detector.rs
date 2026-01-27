//! Date field detector module.
//!
//! Detects date fields composed of multiple input fields separated by delimiters.
//! Common patterns:
//! - Month field + "." + Year field
//! - Day field + "." + Month field + "." + Year field

use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use super::AnalysisModule;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects date fields by identifying multiple field groups separated by delimiters.
///
/// Date fields are characterized by:
/// 1. Multiple field groups close to each other horizontally
/// 2. Text separators (typically ".", "/", "-") between them
/// 3. Fields should be on the same line
pub struct DateFieldDetector {
    /// Maximum horizontal gap between field and separator
    pub max_separator_gap: Decimal,
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
    /// Valid separators for date fields
    pub valid_separators: Vec<String>,
}

impl Default for DateFieldDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl DateFieldDetector {
    pub fn new() -> Self {
        DateFieldDetector {
            max_separator_gap: Decimal::from_str("10.0").unwrap(),
            line_tolerance: Decimal::from_str("8.0").unwrap(),
            valid_separators: vec![".".to_string(), "/".to_string(), "-".to_string()],
        }
    }
    
    /// Check if two elements are on the same line (vertically aligned).
    fn are_on_same_line(&self, bounds1: &Bounds, bounds2: &Bounds) -> bool {
        bounds1.is_horizontally_aligned(bounds2, self.line_tolerance)
    }
    
    /// Check if text is a valid date separator.
    fn is_date_separator(&self, text: &str) -> bool {
        let trimmed = text.trim();
        self.valid_separators.contains(&trimmed.to_string())
    }
    
    /// Find text separator between two fields.
    fn find_separator_between(
        &self,
        doc: &Document,
        field1_idx: usize,
        field2_idx: usize,
        text_groups: &[usize]
    ) -> Option<usize> {
        let field1_bounds = doc.get_bounds(field1_idx)?;
        let field2_bounds = doc.get_bounds(field2_idx)?;
        
        // Field2 should be to the right of field1
        if field2_bounds.x <= field1_bounds.right() {
            return None;
        }
        
        // Find text elements between the two fields
        for &text_idx in text_groups {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };
            
            // Text should be between the two fields
            if text_bounds.x >= field1_bounds.right() && text_bounds.right() <= field2_bounds.x {
                // Check if it's on the same line
                if self.are_on_same_line(&field1_bounds, &text_bounds) {
                    // Check gap from field1
                    let gap1 = text_bounds.x - field1_bounds.right();
                    if gap1 <= self.max_separator_gap {
                        // Get the text content
                        let text_content = doc.get_text_content(text_idx);
                        if self.is_date_separator(&text_content) {
                            return Some(text_idx);
                        }
                    }
                }
            }
        }
        
        None
    }
    
    /// Try to build a date field starting from a given field.
    fn try_build_date_field(
        &self,
        doc: &Document,
        start_field_idx: usize,
        field_groups: &[usize],
        text_groups: &[usize],
        used_fields: &std::collections::HashSet<usize>,
        used_texts: &std::collections::HashSet<usize>
    ) -> Option<(Vec<usize>, Vec<usize>)> {
        if used_fields.contains(&start_field_idx) {
            return None;
        }
        
        let start_bounds = doc.get_bounds(start_field_idx)?;
        
        let mut date_fields = vec![start_field_idx];
        let mut separators = Vec::new();
        let mut current_field_idx = start_field_idx;
        
        // Try to find up to 2 more fields (for day.month.year pattern)
        for _ in 0..2 {
            // Find the next field to the right
            let mut next_field: Option<(usize, Decimal)> = None;
            
            for &field_idx in field_groups {
                if used_fields.contains(&field_idx) || date_fields.contains(&field_idx) {
                    continue;
                }
                
                let Some(field_bounds) = doc.get_bounds(field_idx) else {
                    continue;
                };
                
                // Must be on the same line
                if !self.are_on_same_line(&start_bounds, &field_bounds) {
                    continue;
                }
                
                let current_bounds = doc.get_bounds(current_field_idx)?;
                
                // Must be to the right
                if field_bounds.x <= current_bounds.right() {
                    continue;
                }
                
                // Check for separator between current and this field
                if let Some(sep_idx) = self.find_separator_between(doc, current_field_idx, field_idx, text_groups) {
                    if !used_texts.contains(&sep_idx) && !separators.contains(&sep_idx) {
                        let distance = field_bounds.x - current_bounds.right();
                        if next_field.map(|(_, d)| distance < d).unwrap_or(true) {
                            next_field = Some((field_idx, distance));
                        }
                    }
                }
            }
            
            if let Some((field_idx, _)) = next_field {
                // Found a next field with separator
                if let Some(sep_idx) = self.find_separator_between(doc, current_field_idx, field_idx, text_groups) {
                    date_fields.push(field_idx);
                    separators.push(sep_idx);
                    current_field_idx = field_idx;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        
        // We need at least 2 fields with 1 separator to make a date field
        if date_fields.len() >= 2 && separators.len() >= 1 {
            Some((date_fields, separators))
        } else {
            None
        }
    }
}

impl AnalysisModule for DateFieldDetector {
    fn name(&self) -> &'static str {
        "DateFieldDetector"
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
        
        let mut used_fields: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut used_texts: std::collections::HashSet<usize> = std::collections::HashSet::new();
        
        // Sort fields by position (left to right, top to bottom)
        let mut sorted_fields = field_groups.clone();
        sorted_fields.sort_by(|&a, &b| {
            let bounds_a = doc.get_bounds(a);
            let bounds_b = doc.get_bounds(b);
            match (bounds_a, bounds_b) {
                (Some(a), Some(b)) => {
                    a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x))
                }
                _ => std::cmp::Ordering::Equal,
            }
        });
        
        // Try to build date fields
        for &field_idx in &sorted_fields {
            if let Some((date_fields, separators)) = self.try_build_date_field(
                doc,
                field_idx,
                &field_groups,
                &text_groups,
                &used_fields,
                &used_texts
            ) {
                // Mark fields and separators as used
                for &f in &date_fields {
                    used_fields.insert(f);
                }
                for &s in &separators {
                    used_texts.insert(s);
                }
                
                // Merge fields and separators into a DateField group
                let mut children = Vec::new();
                
                // Interleave fields and separators
                for i in 0..date_fields.len() {
                    children.push(date_fields[i]);
                    if i < separators.len() {
                        children.push(separators[i]);
                    }
                }
                
                doc.merge(
                    children,
                    GroupKind::DateField {
                        num_fields: date_fields.len(),
                    },
                    GroupSource::Inferred { module: self.name().to_string() },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind, Page};
    use crate::xfa::num;
    use crate::modules::{FieldGrouper, TextBlockGrouper};
    
    #[test]
    fn test_month_dot_year_detection() {
        // Create a flattened document with month field + "." + year field
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Month field
                FlattenedNode {
                    kind: FlattenedNodeKind::Field {
                        name: "month".to_string(),
                        value: "".to_string(),
                        label: "".to_string(),
                    },
                    x: num(50.0),
                    y: num(100.0),
                    width: num(30.0),
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
                // Separator "."
                FlattenedNode {
                    kind: FlattenedNodeKind::Text {
                        content: ".".to_string(),
                        font_size: num(10.0),
                        font_name: "Helvetica".to_string(),
                        source_name: None,
                        rich_text: None,
                    },
                    x: num(82.0),
                    y: num(100.0),
                    width: num(5.0),
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
                // Year field
                FlattenedNode {
                    kind: FlattenedNodeKind::Field {
                        name: "year".to_string(),
                        value: "".to_string(),
                        label: "".to_string(),
                    },
                    x: num(90.0),
                    y: num(100.0),
                    width: num(40.0),
                    height: num(12.0),
                    rotate: 0,
                    style: Default::default(),
                },
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        
        // Process with required modules
        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);
        
        // Should have created a DateField group
        let date_fields = doc.find_groups(|k| matches!(k, GroupKind::DateField { .. }));
        assert_eq!(date_fields.len(), 1);
        
        // The group should contain 2 fields and 1 separator
        let group = doc.get_group(date_fields[0]).unwrap();
        assert_eq!(group.children.len(), 3); // field + separator + field
        
        if let GroupKind::DateField { num_fields } = group.kind {
            assert_eq!(num_fields, 2);
        } else {
            panic!("Expected DateField group");
        }
    }
}
