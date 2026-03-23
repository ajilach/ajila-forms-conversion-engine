//! Date field detector module.
//!
//! Detects date field patterns consisting of multiple fields and/or text elements
//! arranged horizontally (e.g., DD.MM.YYYY patterns).
//!
//! Supports:
//! - Field + delimiter + Field + delimiter + Field (e.g., [day] "." [month] "." [year])
//! - Text placeholder + delimiter + Field + delimiter + Field (e.g., "dd." [month] "." [year])
//! - Field + delimiter + Field + text placeholder (e.g., [day] "." [month] ".yyyy")
//! - Partial dates with 2 fields (e.g., [month] "/" [year])
//! - Various delimiters: ".", "/", "-"
//! - Hardcoded numeric values: "01", "12", "2024", etc.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Represents an element in a potential date pattern.
#[derive(Debug, Clone)]
enum DateElement {
    /// A Field group (interactive input)
    Field(usize),
    /// A TextBlock containing a delimiter (., /, -)
    Delimiter(usize),
    /// A TextBlock containing a date placeholder (dd, mm, yyyy) or numeric literal (01, 12, 2024)
    /// The bool indicates if it has a trailing delimiter (e.g., "01." vs "01")
    Placeholder(usize, bool),
}

/// Detects and groups date field patterns.
///
/// Scans for horizontally-aligned sequences of fields and text elements
/// that form date patterns like DD.MM.YYYY.
pub struct DateFieldDetector {
    /// Maximum horizontal gap between adjacent elements
    pub horizontal_threshold: Decimal,
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
}

impl Default for DateFieldDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl DateFieldDetector {
    pub fn new() -> Self {
        DateFieldDetector {
            horizontal_threshold: Decimal::from_str("15.0").unwrap(),
            line_tolerance: Decimal::from_str("8.0").unwrap(),
        }
    }

    /// Check if text content is a date delimiter.
    fn is_delimiter(text: &str) -> bool {
        let trimmed = text.trim();
        matches!(trimmed, "." | "/" | "-")
    }

    /// Check if text content is a date placeholder or numeric literal.
    /// Recognizes: dd, mm, yyyy, yy, tt, jj, jjjj (German), or numeric values like 01, 12, 2024
    /// Returns (is_placeholder, has_trailing_delimiter)
    fn is_date_placeholder(text: &str) -> (bool, bool) {
        let trimmed = text.trim().to_lowercase();

        // Check for trailing delimiter
        let has_trailing =
            trimmed.ends_with('.') || trimmed.ends_with('/') || trimmed.ends_with('-');

        // Check for placeholder patterns (with optional trailing delimiter)
        let without_delimiter = trimmed
            .trim_end_matches('.')
            .trim_end_matches('/')
            .trim_end_matches('-');

        // Day placeholders
        if matches!(without_delimiter, "dd" | "tt" | "d" | "t") {
            return (true, has_trailing);
        }

        // Month placeholders
        if matches!(without_delimiter, "mm" | "m") {
            return (true, has_trailing);
        }

        // Year placeholders
        if matches!(without_delimiter, "yyyy" | "yy" | "jjjj" | "jj" | "y" | "j") {
            return (true, has_trailing);
        }

        // Numeric literals (1-4 digits, representing day/month/year)
        if !without_delimiter.is_empty()
            && without_delimiter.len() <= 4
            && without_delimiter.chars().all(|c| c.is_ascii_digit())
        {
            return (true, has_trailing);
        }

        (false, false)
    }

    /// Check if text content contains ONLY a delimiter with optional placeholder.
    /// E.g., "." or "dd." or ".yyyy"
    fn classify_text(text: &str) -> Option<TextClassification> {
        let trimmed = text.trim();

        // Pure delimiter
        if Self::is_delimiter(trimmed) {
            return Some(TextClassification::Delimiter);
        }

        // Placeholder (possibly with trailing delimiter like "dd." or "01.")
        let (is_placeholder, has_trailing) = Self::is_date_placeholder(trimmed);
        if is_placeholder {
            return Some(TextClassification::Placeholder {
                has_trailing_delimiter: has_trailing,
            });
        }

        None
    }

    /// Check if two bounds are adjacent horizontally on the same line.
    fn is_adjacent(&self, doc: &Document, left_idx: usize, right_idx: usize) -> bool {
        let Some(left_bounds) = doc.get_bounds(left_idx) else {
            return false;
        };
        let Some(right_bounds) = doc.get_bounds(right_idx) else {
            return false;
        };

        // Must be on same line
        if !left_bounds.is_on_same_line(&right_bounds, self.line_tolerance) {
            return false;
        }

        // Right must be to the right of left
        let Some(gap) = left_bounds.horizontal_gap_to(&right_bounds) else {
            return false;
        };

        gap <= self.horizontal_threshold
    }

    /// Collect all root groups sorted by x position for a given line.
    fn collect_line_elements(&self, doc: &Document, roots: &[usize]) -> Vec<Vec<(usize, Decimal)>> {
        // Group roots by approximate y-position (line)
        let mut lines: Vec<Vec<(usize, Decimal, Decimal)>> = Vec::new();

        for &idx in roots {
            let Some(bounds) = doc.get_bounds(idx) else {
                continue;
            };

            // Find existing line or create new one
            let mut found = false;
            for line in &mut lines {
                if let Some((_, _, ref_y)) = line.first() {
                    // Check if on same line (use first element's y as reference)
                    let ref_bounds = crate::flattened::Bounds {
                        x: Decimal::ZERO,
                        y: *ref_y,
                        width: Decimal::ONE,
                        height: bounds.height,
                    };
                    if bounds.is_on_same_line(&ref_bounds, self.line_tolerance) {
                        line.push((idx, bounds.x, bounds.y));
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                lines.push(vec![(idx, bounds.x, bounds.y)]);
            }
        }

        // Sort each line by x position and return (idx, x)
        lines
            .into_iter()
            .map(|mut line| {
                line.sort_by(|a, b| a.1.cmp(&b.1));
                line.into_iter().map(|(idx, x, _)| (idx, x)).collect()
            })
            .collect()
    }

    /// Try to find a date pattern starting from a given position in the line.
    /// Returns the elements that form the date pattern and the number of fields.
    fn find_date_pattern(
        &self,
        doc: &Document,
        line: &[(usize, Decimal)],
        start: usize,
    ) -> Option<(Vec<usize>, usize)> {
        if start >= line.len() {
            return None;
        }

        let mut elements: Vec<DateElement> = Vec::new();
        let mut current = start;
        let mut field_count = 0;

        // First element determines valid pattern start:
        // - A Placeholder (with or without trailing delimiter) can start a pattern
        // - A Field can only start a pattern if followed by a pure Delimiter
        let (first_idx, _) = line[current];

        if doc.is_field(first_idx) {
            // Check if next element is a pure delimiter (not a placeholder)
            if current + 1 >= line.len() {
                return None;
            }
            let (next_idx, _) = line[current + 1];
            if !self.is_adjacent(doc, first_idx, next_idx) {
                return None;
            }
            if !doc.is_text_block(next_idx) {
                return None;
            }
            let next_text = doc.get_text_content(next_idx);
            if !matches!(
                Self::classify_text(&next_text),
                Some(TextClassification::Delimiter)
            ) {
                return None; // Field not followed by delimiter - can't start date pattern here
            }
            // Valid start: Field followed by Delimiter
            elements.push(DateElement::Field(first_idx));
            field_count += 1;
            current += 1;
        } else if doc.is_text_block(first_idx) {
            let text = doc.get_text_content(first_idx);
            match Self::classify_text(&text) {
                Some(TextClassification::Placeholder {
                    has_trailing_delimiter,
                }) => {
                    // Valid start: Placeholder
                    elements.push(DateElement::Placeholder(first_idx, has_trailing_delimiter));
                    current += 1;
                }
                _ => return None, // Can't start with delimiter or non-date text
            }
        } else {
            return None;
        }

        // Continue building the sequence
        loop {
            if current >= line.len() {
                break;
            }

            let (idx, _) = line[current];

            // Check adjacency with previous element
            if !elements.is_empty() {
                let prev_idx = match elements.last().unwrap() {
                    DateElement::Field(i)
                    | DateElement::Delimiter(i)
                    | DateElement::Placeholder(i, _) => *i,
                };
                if !self.is_adjacent(doc, prev_idx, idx) {
                    break;
                }
            }

            // Classify this element
            if doc.is_field(idx) {
                elements.push(DateElement::Field(idx));
                field_count += 1;
                current += 1;
            } else if doc.is_text_block(idx) {
                let text = doc.get_text_content(idx);
                match Self::classify_text(&text) {
                    Some(TextClassification::Delimiter) => {
                        // Delimiter must follow a Field or Placeholder (without trailing delimiter)
                        if elements.is_empty() {
                            break;
                        }
                        match elements.last() {
                            Some(DateElement::Delimiter(_)) => break, // Can't have two delimiters in a row
                            Some(DateElement::Placeholder(_, true)) => break, // Placeholder already has delimiter
                            _ => {
                                elements.push(DateElement::Delimiter(idx));
                                current += 1;
                            }
                        }
                    }
                    Some(TextClassification::Placeholder {
                        has_trailing_delimiter,
                    }) => {
                        elements.push(DateElement::Placeholder(idx, has_trailing_delimiter));
                        current += 1;
                    }
                    None => break, // Not a date-related text
                }
            } else {
                break; // Unknown element type
            }
        }

        // Validate the pattern:
        // - Must have at least 2 fields/placeholders
        // - Must have at least 1 actual Field
        // - Pattern should be: (Field|Placeholder) (Delimiter (Field|Placeholder))+
        // - A Placeholder with trailing delimiter can be followed directly by Field/Placeholder

        let total_inputs = elements
            .iter()
            .filter(|e| matches!(e, DateElement::Field(_) | DateElement::Placeholder(_, _)))
            .count();

        if total_inputs < 2 || field_count < 1 {
            return None;
        }

        // Verify pattern structure: inputs separated by delimiters
        // A Placeholder with has_trailing_delimiter=true acts as both input and delimiter
        let mut expect_input = true;
        for elem in &elements {
            match (expect_input, elem) {
                (true, DateElement::Field(_)) => {
                    expect_input = false;
                }
                (true, DateElement::Placeholder(_, has_trailing)) => {
                    // If placeholder has trailing delimiter, next can be input or delimiter
                    // If not, next must be delimiter
                    expect_input = !has_trailing;
                }
                (false, DateElement::Delimiter(_)) => {
                    expect_input = true;
                }
                (false, DateElement::Field(_)) => {
                    // This can happen after a Placeholder with trailing delimiter
                    expect_input = false;
                }
                (false, DateElement::Placeholder(_, has_trailing)) => {
                    // This can happen after a Placeholder with trailing delimiter
                    expect_input = !has_trailing;
                }
                _ => return None, // Invalid pattern
            }
        }

        // Pattern should end with an input, not a delimiter
        if expect_input {
            // Last element was a delimiter, remove it
            if let Some(DateElement::Delimiter(_)) = elements.last() {
                elements.pop();
            }
        }

        // Re-check after potential removal
        let total_inputs = elements
            .iter()
            .filter(|e| matches!(e, DateElement::Field(_) | DateElement::Placeholder(_, _)))
            .count();
        let field_count = elements
            .iter()
            .filter(|e| matches!(e, DateElement::Field(_)))
            .count();

        if total_inputs < 2 || field_count < 1 {
            return None;
        }

        // Extract group indices
        let indices: Vec<usize> = elements
            .iter()
            .map(|e| match e {
                DateElement::Field(i)
                | DateElement::Delimiter(i)
                | DateElement::Placeholder(i, _) => *i,
            })
            .collect();

        Some((indices, field_count))
    }
}

#[derive(Debug, Clone, Copy)]
enum TextClassification {
    Delimiter,
    Placeholder { has_trailing_delimiter: bool },
}

impl AnalysisModule for DateFieldDetector {
    fn name(&self) -> &'static str {
        "DateFieldDetector"
    }

    fn process(&self, doc: &mut Document) {
        let roots = doc.roots();

        // Filter to only Fields and TextBlocks (candidates for date patterns)
        let candidates: Vec<usize> = roots
            .iter()
            .filter(|&&idx| doc.is_field(idx) || doc.is_text_block(idx))
            .copied()
            .collect();

        if candidates.is_empty() {
            return;
        }

        // Group by lines
        let lines = self.collect_line_elements(doc, &candidates);

        // Find date patterns in each line
        let mut date_groups: Vec<(Vec<usize>, usize)> = Vec::new();

        for line in &lines {
            let mut pos = 0;
            while pos < line.len() {
                if let Some((indices, field_count)) = self.find_date_pattern(doc, line, pos) {
                    let consumed = indices.len();
                    date_groups.push((indices, field_count));
                    pos += consumed;
                } else {
                    pos += 1;
                }
            }
        }

        // Create DateField groups
        for (indices, field_count) in date_groups {
            doc.merge_inferred(
                indices,
                GroupKind::DateField {
                    num_fields: field_count,
                },
                self.name(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{FieldGrouper, TextBlockGrouper};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_three_fields_with_dot_delimiters() {
        // Pattern: [DayField] "." [MonthField] "." [YearField]
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "Day".to_string(),
                    "".to_string(),
                    "Day".to_string(),
                    num(10.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(42.0),
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Month".to_string(),
                    "".to_string(),
                    "Month".to_string(),
                    num(50.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(82.0),
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Year".to_string(),
                    "".to_string(),
                    "Year".to_string(),
                    num(90.0),
                    num(100.0),
                    num(50.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        // Should have created 1 DateField with 3 fields
        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 1);

        if let Some(g) = doc.get_group(date_fields[0]) {
            if let GroupKind::DateField { num_fields } = g.kind {
                assert_eq!(num_fields, 3);
            } else {
                panic!("Expected DateField");
            }
        }
    }

    #[test]
    fn test_placeholder_dd_with_fields() {
        // Pattern: "dd." [MonthField] "." [YearField]
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "dd.".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(20.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Month".to_string(),
                    "".to_string(),
                    "Month".to_string(),
                    num(35.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(67.0),
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Year".to_string(),
                    "".to_string(),
                    "Year".to_string(),
                    num(75.0),
                    num(100.0),
                    num(50.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 1);

        if let Some(g) = doc.get_group(date_fields[0]) {
            if let GroupKind::DateField { num_fields } = g.kind {
                assert_eq!(num_fields, 2); // Only 2 actual fields
            }
        }
    }

    #[test]
    fn test_fields_with_yyyy_placeholder() {
        // Pattern: [DayField] "." [MonthField] ".yyyy"
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "Day".to_string(),
                    "".to_string(),
                    "Day".to_string(),
                    num(10.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(42.0),
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Month".to_string(),
                    "".to_string(),
                    "Month".to_string(),
                    num(50.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".yyyy".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(82.0),
                    num(100.0),
                    num(30.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 1);

        if let Some(g) = doc.get_group(date_fields[0]) {
            if let GroupKind::DateField { num_fields } = g.kind {
                assert_eq!(num_fields, 2); // Only 2 actual fields
            }
        }
    }

    #[test]
    fn test_partial_date_month_year() {
        // Pattern: [MonthField] "/" [YearField]
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "Month".to_string(),
                    "".to_string(),
                    "Month".to_string(),
                    num(10.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "/".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(42.0),
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Year".to_string(),
                    "".to_string(),
                    "Year".to_string(),
                    num(50.0),
                    num(100.0),
                    num(50.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 1);

        if let Some(g) = doc.get_group(date_fields[0]) {
            if let GroupKind::DateField { num_fields } = g.kind {
                assert_eq!(num_fields, 2);
            }
        }
    }

    #[test]
    fn test_numeric_literal_day() {
        // Pattern: "01." [MonthField] "." [YearField]
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "01.".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(20.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Month".to_string(),
                    "".to_string(),
                    "Month".to_string(),
                    num(35.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(67.0),
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Year".to_string(),
                    "".to_string(),
                    "Year".to_string(),
                    num(75.0),
                    num(100.0),
                    num(50.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 1);
    }

    #[test]
    fn test_no_date_for_unrelated_fields() {
        // Two fields on the same line but no delimiter between them
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "FirstName".to_string(),
                    "".to_string(),
                    "First Name".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(20.0),
                ),
                FlattenedNode::new_field(
                    "LastName".to_string(),
                    "".to_string(),
                    "Last Name".to_string(),
                    num(120.0),
                    num(100.0),
                    num(100.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        // Should NOT create a DateField (no delimiters)
        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 0);
    }

    #[test]
    fn test_distant_fields_not_grouped() {
        // Fields with delimiters but too far apart
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_field(
                    "Day".to_string(),
                    "".to_string(),
                    "Day".to_string(),
                    num(10.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    ".".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(100.0), // Far from the Day field
                    num(100.0),
                    num(5.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "Month".to_string(),
                    "".to_string(),
                    "Month".to_string(),
                    num(110.0),
                    num(100.0),
                    num(30.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);

        // Should NOT create a DateField (elements too far apart)
        let date_fields: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::DateField { .. }))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(date_fields.len(), 0);
    }

    #[test]
    fn test_aaab_has_at_least_5_date_fields() {
        use crate::document::modules::run_analysis_pipeline;
        use crate::extract_xfa_from_pdf;
        use crate::flattened::Flattened;
        use crate::xfa::XfaNode;
        use crate::xfa::script_executor::ScriptExecutor;

        // Load AAAB PDF
        let pdf_path = format!("{}/input/AAAB_019_DE.pdf", env!("CARGO_MANIFEST_DIR"));
        let xfa_data = extract_xfa_from_pdf(&pdf_path).expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");

        let mut nodes = XfaNode::parse(&xfa_data.unwrap()).expect("Failed to parse XFA structure");

        // Execute scripts to populate dynamic content
        let script_result = ScriptExecutor::execute(&nodes);
        ScriptExecutor::apply_presence_changes(&mut nodes, &script_result.presence_changes);

        // Flatten the XFA with computed values
        let flattened = Flattened::from_xfa(&nodes, &script_result.computed_values)
            .expect("Failed to flatten XFA");

        // Create document and run full pipeline
        let mut doc = Document::from_flattened(&flattened);
        run_analysis_pipeline(&mut doc);

        // Count DateField groups ANYWHERE in the document (not just roots)
        // DateFields may be wrapped in LabeledField groups by LabelAttacher
        let date_fields = doc.date_fields();

        println!("Found {} date fields in AAAB", date_fields.len());

        assert!(
            date_fields.len() >= 5,
            "AAAB should have at least 5 date fields, found {}",
            date_fields.len()
        );
    }
}
