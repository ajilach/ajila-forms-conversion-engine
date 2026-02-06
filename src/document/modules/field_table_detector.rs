//! Field table detector module.
//!
//! Detects tables of fields with column headers and converts them into
//! GridLayout groups containing LabeledField children where headers become labels.
//!
//! The detector works in two modes:
//!
//! ## Mode 1: Bold Header Tables (standalone fields)
//! 1. Finding unclaimed Field groups
//! 2. Finding unclaimed bold TextBlock groups (potential headers)
//! 3. Clustering by x-coordinate to detect columns
//! 4. Validating that each column has exactly one bold header above the fields
//! 5. Creating LabeledField groups pairing each field with its column header
//! 6. Wrapping each row of LabeledFields in a GridLayout
//!
//! ## Mode 2: Repeatable Section Tables (after RepeatableDetector)
//! 1. Finding RepeatableSection groups that contain fields
//! 2. Finding TextBlock groups immediately above the repeatable (within vertical threshold)
//! 3. Matching headers to fields by x-coordinate alignment
//! 4. Claiming the headers and creating LabeledFields inside the repeatable's GridLayout

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::FlattenedNodeKind;
use crate::xfa::FontWeight;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::str::FromStr;

/// Detects field tables with bold headers and creates GridLayout rows with LabeledFields.
///
/// Field tables are characterized by:
/// 1. Multiple fields aligned in columns (same x-coordinate)
/// 2. Bold text headers above each column
/// 3. At least 2 columns and 2 rows (1 header row + 1 data row)
pub struct FieldTableDetector {
    /// Tolerance for considering two x-coordinates as aligned (in points)
    pub alignment_tolerance: Decimal,
    /// Tolerance for considering elements on the same row (in points)
    pub row_tolerance: Decimal,
    /// Minimum number of data rows required (excluding header)
    pub min_data_rows: usize,
    /// Minimum number of columns required
    pub min_columns: usize,
}

impl Default for FieldTableDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldTableDetector {
    pub fn new() -> Self {
        FieldTableDetector {
            alignment_tolerance: Decimal::from_str("5.0").unwrap(),
            row_tolerance: Decimal::from_str("3.0").unwrap(),
            min_data_rows: 1, // 1 data row + 1 header row = 2 total rows
            min_columns: 2,
        }
    }

    /// Configure the alignment tolerance for column detection.
    pub fn with_alignment_tolerance(mut self, tolerance: Decimal) -> Self {
        self.alignment_tolerance = tolerance;
        self
    }

    /// Configure the row tolerance for same-row detection.
    pub fn with_row_tolerance(mut self, tolerance: Decimal) -> Self {
        self.row_tolerance = tolerance;
        self
    }

    /// Check if all text nodes in a group are bold.
    fn is_all_bold(&self, doc: &Document, group_idx: usize) -> bool {
        let nodes = doc.collect_nodes(group_idx);
        let text_nodes: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, FlattenedNodeKind::Text { .. }))
            .collect();

        if text_nodes.is_empty() {
            return false;
        }

        text_nodes.iter().all(|node| {
            node.style
                .font
                .as_ref()
                .map(|f| f.weight == FontWeight::Bold)
                .unwrap_or(false)
        })
    }

    /// Group indices by y-coordinate into rows.
    fn cluster_by_y(&self, doc: &Document, indices: &[usize]) -> Vec<(Decimal, Vec<usize>)> {
        let mut rows: Vec<(Decimal, Vec<usize>)> = Vec::new();

        for &idx in indices {
            let Some(bounds) = doc.get_bounds(idx) else {
                continue;
            };

            let y = bounds.y;
            let mut found = false;

            for (row_y, row_indices) in &mut rows {
                if (y - *row_y).abs() <= self.row_tolerance {
                    row_indices.push(idx);
                    found = true;
                    break;
                }
            }

            if !found {
                rows.push((y, vec![idx]));
            }
        }

        // Sort rows by y-coordinate (top to bottom)
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    /// Find field tables and create GridLayout groups with LabeledField children.
    fn detect_field_tables(&self, doc: &mut Document) {
        // Get unclaimed Field and DateField groups (both are valid table cell types)
        let fields = doc.root_fields();

        // Get unclaimed bold TextBlock groups (potential headers)
        let bold_text_blocks = doc.root_groups_matching(|doc, idx| {
            doc.is_text_block(idx) && self.is_all_bold(doc, idx)
        });

        if fields.is_empty() || bold_text_blocks.is_empty() {
            return;
        }

        // NEW APPROACH: First find header rows (groups of bold text on the same y-coordinate)
        let header_rows = self.cluster_by_y(doc, &bold_text_blocks);

        // Filter to header rows with at least min_columns headers
        let valid_header_rows: Vec<_> = header_rows
            .into_iter()
            .filter(|(_, headers)| headers.len() >= self.min_columns)
            .collect();

        if valid_header_rows.is_empty() {
            return;
        }

        // Sort header rows by y-coordinate (top to bottom)
        let mut sorted_header_rows = valid_header_rows;
        sorted_header_rows.sort_by(|a, b| a.0.cmp(&b.0));

        // Track which headers and fields have been used
        let mut used_headers: HashSet<usize> = HashSet::new();
        let mut used_fields: HashSet<usize> = HashSet::new();

        // For each header row, try to form a table
        for (i, (header_y, header_indices)) in sorted_header_rows.iter().enumerate() {
            // Skip if any header in this row is already used
            if header_indices.iter().any(|&h| used_headers.contains(&h)) {
                continue;
            }

            // Determine the y-boundary for this table's fields
            // Fields must be below header_y but above the next header row (if any)
            let max_field_y = if i + 1 < sorted_header_rows.len() {
                sorted_header_rows[i + 1].0
            } else {
                Decimal::MAX
            };

            // Sort headers by x-coordinate (left to right)
            let mut sorted_headers: Vec<(Decimal, usize)> = header_indices
                .iter()
                .filter_map(|&h| doc.get_bounds(h).map(|b| (b.x, h)))
                .collect();
            sorted_headers.sort_by(|a, b| a.0.cmp(&b.0));

            if sorted_headers.len() < self.min_columns {
                continue;
            }

            // Find fields that are below this header row and above the next header row
            let table_fields: Vec<usize> = fields
                .iter()
                .filter(|&&f| {
                    if used_fields.contains(&f) {
                        return false;
                    }
                    if let Some(fb) = doc.get_bounds(f) {
                        fb.y > *header_y && fb.y < max_field_y
                    } else {
                        false
                    }
                })
                .copied()
                .collect();

            if table_fields.is_empty() {
                continue;
            }

            // For each header, find fields that align with it (within tolerance)
            let mut column_fields: Vec<(usize, Vec<usize>)> = Vec::new(); // (header_idx, field_indices)

            for (header_x, header_idx) in &sorted_headers {
                let aligned_fields: Vec<usize> = table_fields
                    .iter()
                    .filter(|&&f| {
                        if let Some(fb) = doc.get_bounds(f) {
                            (fb.x - *header_x).abs() <= self.alignment_tolerance
                        } else {
                            false
                        }
                    })
                    .copied()
                    .collect();

                column_fields.push((*header_idx, aligned_fields));
            }

            // Check that we have at least min_columns with fields
            let columns_with_fields: Vec<_> = column_fields
                .iter()
                .filter(|(_, fields)| !fields.is_empty())
                .collect();

            if columns_with_fields.len() < self.min_columns {
                continue;
            }

            // Build the table: cluster fields by y to form rows
            let all_table_field_indices: Vec<usize> = column_fields
                .iter()
                .flat_map(|(_, f)| f.iter().copied())
                .collect();

            let field_rows = self.cluster_by_y(doc, &all_table_field_indices);

            if field_rows.len() < self.min_data_rows {
                continue;
            }

            // Mark headers as used
            for (_, header_idx) in &sorted_headers {
                used_headers.insert(*header_idx);
            }

            let num_columns = sorted_headers.len();

            // For each row of fields, create LabeledFields and wrap in GridLayout
            for (_row_y, row_field_indices) in field_rows {
                // Sort fields in this row by x-coordinate
                let mut row_fields: Vec<(Decimal, usize)> = row_field_indices
                    .iter()
                    .filter_map(|&idx| doc.get_bounds(idx).map(|b| (b.x, idx)))
                    .collect();
                row_fields.sort_by(|a, b| a.0.cmp(&b.0));

                // Match each field to its column header
                let mut labeled_field_indices: Vec<usize> = Vec::new();

                for (field_x, field_idx) in row_fields {
                    // Skip if already used
                    if used_fields.contains(&field_idx) {
                        continue;
                    }

                    // Find the column this field belongs to
                    let mut matched_header: Option<usize> = None;

                    for (header_x, header_idx) in &sorted_headers {
                        if (field_x - *header_x).abs() <= self.alignment_tolerance {
                            matched_header = Some(*header_idx);
                            break;
                        }
                    }

                    if let Some(header_idx) = matched_header {
                        // Create LabeledField with header as label
                        let labeled_idx =
                            doc.create_labeled_field(header_idx, field_idx, self.name());
                        labeled_field_indices.push(labeled_idx);
                        used_fields.insert(field_idx);
                    }
                }

                // Skip if we didn't get enough labeled fields for a valid row
                if labeled_field_indices.len() < self.min_columns {
                    continue;
                }

                // Create GridLayout for this row
                let spans = vec![1; labeled_field_indices.len()];
                doc.merge(
                    labeled_field_indices,
                    GroupKind::GridLayout {
                        columns: num_columns,
                        spans,
                    },
                    GroupSource::Inferred {
                        module: self.name().to_string(),
                    },
                );
            }
        }
    }

    /// Find repeatable sections with fields and attach column headers as labels.
    ///
    /// This method handles the case where:
    /// 1. RepeatableDetector has already created RepeatableSection groups
    /// 2. There are TextBlocks above the repeatable that serve as column headers
    /// 3. The fields inside the repeatable should be labeled with those headers
    ///
    /// The result is a NEW RepeatableSection containing LabeledFields in a GridLayout,
    /// which claims both the headers and the original repeatable.
    fn detect_repeatable_column_headers(&self, doc: &mut Document) {
        // Maximum vertical distance to look for headers above a repeatable
        let max_header_distance = Decimal::from_str("30.0").unwrap();

        // Find all RepeatableSection groups
        let all_roots = doc.roots();
        let repeatables: Vec<(usize, u32, Option<u32>)> = all_roots
            .iter()
            .filter_map(|&idx| {
                doc.get_group(idx).and_then(|g| {
                    if let GroupKind::RepeatableSection { min_occurrences, max_occurrences } = g.kind {
                        Some((idx, min_occurrences, max_occurrences))
                    } else {
                        None
                    }
                })
            })
            .collect();

        // Find all unclaimed TextBlock groups (potential headers)
        let text_blocks: Vec<usize> = all_roots
            .iter()
            .filter(|&&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::TextBlock))
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        if repeatables.is_empty() || text_blocks.is_empty() {
            return;
        }

        // Track which text blocks have been used as headers
        let mut used_headers: HashSet<usize> = HashSet::new();

        for (repeatable_idx, min_occurrences, max_occurrences) in repeatables {
            let Some(repeatable_bounds) = doc.get_bounds(repeatable_idx) else {
                continue;
            };

            // Find fields within this repeatable
            let fields_in_repeatable: Vec<usize> = self.find_fields_in_group(doc, repeatable_idx);
            if fields_in_repeatable.is_empty() {
                continue;
            }

            // Find text blocks that are above the repeatable (within max_header_distance)
            let mut potential_headers: Vec<(Decimal, usize)> = text_blocks
                .iter()
                .filter_map(|&tb_idx| {
                    if used_headers.contains(&tb_idx) {
                        return None;
                    }
                    let Some(tb_bounds) = doc.get_bounds(tb_idx) else {
                        return None;
                    };
                    // Header must be above the repeatable
                    let vertical_gap = repeatable_bounds.y - tb_bounds.bottom();
                    if vertical_gap < Decimal::ZERO || vertical_gap > max_header_distance {
                        return None;
                    }
                    Some((tb_bounds.x, tb_idx))
                })
                .collect();

            if potential_headers.is_empty() {
                continue;
            }

            // Sort headers by x-coordinate
            potential_headers.sort_by(|a, b| a.0.cmp(&b.0));

            // Cluster headers by y-coordinate (should be on same row)
            let header_rows = self.cluster_by_y(doc, &potential_headers.iter().map(|(_, idx)| *idx).collect::<Vec<_>>());
            
            // Find the header row closest to the repeatable (largest y that's still above)
            let Some((_, header_row)) = header_rows.iter().rev().find(|(row_y, headers)| {
                // Check that this row is above the repeatable and has enough columns
                *row_y < repeatable_bounds.y && headers.len() >= 2
            }) else {
                continue;
            };

            // Sort this header row by x
            let mut sorted_headers: Vec<(Decimal, usize)> = header_row
                .iter()
                .filter_map(|&h| doc.get_bounds(h).map(|b| (b.x, h)))
                .collect();
            sorted_headers.sort_by(|a, b| a.0.cmp(&b.0));

            // Sort fields by x-coordinate
            let mut sorted_fields: Vec<(Decimal, usize)> = fields_in_repeatable
                .iter()
                .filter_map(|&f| doc.get_bounds(f).map(|b| (b.x, f)))
                .collect();
            sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));

            // Match headers to fields by x-alignment
            // We need at least 2 matching pairs
            let mut matches: Vec<(usize, usize)> = Vec::new(); // (header_idx, field_idx)
            
            for (field_x, field_idx) in &sorted_fields {
                // Find the closest header
                let best_match = sorted_headers
                    .iter()
                    .filter(|(hx, _)| (*field_x - *hx).abs() <= self.alignment_tolerance)
                    .min_by_key(|(hx, _)| (*field_x - *hx).abs());
                
                if let Some((_, header_idx)) = best_match {
                    matches.push((*header_idx, *field_idx));
                }
            }

            if matches.len() < 2 {
                continue;
            }

            // Mark the matched headers as used
            for (header_idx, _) in &matches {
                used_headers.insert(*header_idx);
            }

            // Create LabeledFields for each matched pair
            let mut labeled_field_indices: Vec<usize> = Vec::new();
            for (header_idx, field_idx) in &matches {
                let labeled_idx = doc.create_labeled_field(*header_idx, *field_idx, self.name());
                labeled_field_indices.push(labeled_idx);
            }

            // Create a GridLayout containing the LabeledFields
            let num_columns = labeled_field_indices.len();
            let spans = vec![1; num_columns];
            let grid_idx = doc.merge(
                labeled_field_indices,
                GroupKind::GridLayout {
                    columns: num_columns,
                    spans,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );

            // Create a new RepeatableSection containing only the GridLayout
            // The original repeatable's fields are already claimed by the LabeledFields
            // We also claim the original repeatable_idx so it doesn't appear separately
            doc.merge(
                vec![grid_idx, repeatable_idx],
                GroupKind::RepeatableSection {
                    min_occurrences,
                    max_occurrences,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }

    /// Find all Field/DateField groups within a given group (recursively).
    fn find_fields_in_group(&self, doc: &Document, group_idx: usize) -> Vec<usize> {
        let mut fields = Vec::new();
        self.collect_fields_recursive(doc, group_idx, &mut fields);
        fields
    }

    fn collect_fields_recursive(&self, doc: &Document, group_idx: usize, fields: &mut Vec<usize>) {
        let Some(group) = doc.get_group(group_idx) else {
            return;
        };

        match &group.kind {
            GroupKind::Field | GroupKind::DateField { .. } => {
                fields.push(group_idx);
            }
            _ => {
                // Recurse into children
                let children: Vec<usize> = group.children.clone();
                for child_idx in children {
                    self.collect_fields_recursive(doc, child_idx, fields);
                }
            }
        }
    }
}

impl AnalysisModule for FieldTableDetector {
    fn name(&self) -> &'static str {
        "FieldTableDetector"
    }

    fn process(&self, doc: &mut Document) {
        // First, detect column headers above repeatable sections
        // (RepeatableDetector has already run)
        self.detect_repeatable_column_headers(doc);
        // Then detect standalone field tables with bold headers
        self.detect_field_tables(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::document::modules::{FieldGrouper, TextBlockGrouper};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::{Font, FontPosture, KerningMode, num};

    fn make_field_node(name: &str, x: f64, y: f64, width: f64, height: f64) -> FlattenedNode {
        FlattenedNode::new_field(
            name.to_string(),
            String::new(),
            name.to_string(),
            num(x),
            num(y),
            num(width),
            num(height),
        )
    }

    fn make_bold_text_node(content: &str, x: f64, y: f64) -> FlattenedNode {
        let mut node = FlattenedNode::new_text(
            content.to_string(),
            num(10.0),
            "Helvetica".to_string(),
            num(x),
            num(y),
            num(50.0),
            num(12.0),
        );
        node.style.font = Some(Font {
            typeface: "Helvetica".to_string(),
            size: num(10.0),
            weight: FontWeight::Bold,
            posture: FontPosture::Normal,
            underline: false,
            line_through: false,
            color: None,
            baseline_shift: None,
            letter_spacing: None,
            generic_family: None,
            kerning_mode: KerningMode::None,
            font_horizontal_scale: None,
            font_vertical_scale: None,
        });
        node
    }

    #[test]
    fn test_field_table_two_columns_two_rows() {
        // Create a simple 2x2 table:
        // [Header1] [Header2]
        // [Field1]  [Field2]
        // [Field3]  [Field4]

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Headers (y=50)
                make_bold_text_node("Column A", 10.0, 50.0),
                make_bold_text_node("Column B", 150.0, 50.0),
                // Row 1 (y=80)
                make_field_node("field_a1", 10.0, 80.0, 100.0, 20.0),
                make_field_node("field_b1", 150.0, 80.0, 100.0, 20.0),
                // Row 2 (y=110)
                make_field_node("field_a2", 10.0, 110.0, 100.0, 20.0),
                make_field_node("field_b2", 150.0, 110.0, 100.0, 20.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        FieldTableDetector::new().process(&mut doc);

        // Should have created 2 GridLayout groups (one per data row)
        let grid_layouts: Vec<_> = doc
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| matches!(g.kind, GroupKind::GridLayout { .. }))
            .collect();

        assert_eq!(
            grid_layouts.len(),
            2,
            "Expected 2 GridLayout rows, found {}",
            grid_layouts.len()
        );

        // Each GridLayout should have 2 children (LabeledFields)
        for (_, grid) in &grid_layouts {
            assert_eq!(
                grid.children.len(),
                2,
                "Each GridLayout row should have 2 LabeledFields"
            );

            if let GroupKind::GridLayout { columns, spans } = &grid.kind {
                assert_eq!(*columns, 2, "Grid should have 2 columns");
                assert_eq!(spans, &vec![1, 1], "Each element should have span 1");
            }
        }

        // Check that LabeledFields were created
        let labeled_fields: Vec<_> = doc
            .groups
            .iter()
            .filter(|g| matches!(g.kind, GroupKind::LabeledField { .. }))
            .collect();

        assert_eq!(
            labeled_fields.len(),
            4,
            "Expected 4 LabeledFields (2 cols × 2 rows)"
        );
    }

    #[test]
    fn test_no_table_without_bold_headers() {
        // Fields without bold headers should not be detected as a table
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Non-bold text (regular weight)
                FlattenedNode::new_text(
                    "Column A".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(50.0),
                    num(50.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "Column B".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(150.0),
                    num(50.0),
                    num(50.0),
                    num(12.0),
                ),
                // Fields
                make_field_node("field_a1", 10.0, 80.0, 100.0, 20.0),
                make_field_node("field_b1", 150.0, 80.0, 100.0, 20.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        FieldTableDetector::new().process(&mut doc);

        // Should NOT have created any GridLayout groups
        let grid_layouts: Vec<_> = doc
            .groups
            .iter()
            .filter(|g| matches!(g.kind, GroupKind::GridLayout { .. }))
            .collect();

        assert_eq!(
            grid_layouts.len(),
            0,
            "Should not detect table without bold headers"
        );
    }

    #[test]
    fn test_aaab_has_at_least_4_field_tables() {
        use crate::document::modules::run_analysis_pipeline;
        use crate::extract_xfa_from_pdf;
        use crate::flattened::Flattened;
        use crate::xfa::XfaNode;
        use crate::xfa::script_executor::ScriptExecutor;

        // Load AAAB PDF
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf").expect("Failed to read PDF");
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

        // Count GridLayout groups that contain LabeledFields (field tables)
        let field_table_rows: Vec<_> = doc
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                if let GroupKind::GridLayout { .. } = &g.kind {
                    // Check if children are LabeledFields
                    g.children.iter().any(|&child_idx| {
                        doc.get_group(child_idx)
                            .map(|child| matches!(child.kind, GroupKind::LabeledField { .. }))
                            .unwrap_or(false)
                    })
                } else {
                    false
                }
            })
            .collect();

        println!(
            "Found {} field table rows (GridLayout with LabeledFields) in AAAB",
            field_table_rows.len()
        );

        // Print labels for each field table row
        for (idx, group) in &field_table_rows {
            print!("  Row {}: ", idx);
            for &child_idx in &group.children {
                if let Some(child) = doc.get_group(child_idx) {
                    if let GroupKind::LabeledField { label, .. } = child.kind {
                        let label_idx = child.children[label];
                        let label_text = doc.get_text_content(label_idx);
                        print!("[{}] ", label_text.chars().take(20).collect::<String>());
                    }
                }
            }
            println!();
        }

        // We expect at least 4 field table rows
        assert!(
            field_table_rows.len() >= 4,
            "AAAB should have at least 4 field table rows, found {}",
            field_table_rows.len()
        );
    }

    #[test]
    fn test_nachname_vorname_not_detected_as_table() {
        use crate::document::modules::{
            DateFieldDetector, FieldGrouper, MasterPageDetector, NoPrintDetector,
            RadioButtonDetector, RadioButtonGrouper, TextBlockGrouper,
        };
        use crate::extract_xfa_from_pdf;
        use crate::flattened::Flattened;
        use crate::xfa::XfaNode;
        use crate::xfa::script_executor::ScriptExecutor;

        // Load AAAB PDF
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf").expect("Failed to read PDF");
        let mut nodes = XfaNode::parse(&xfa_data.unwrap()).expect("Failed to parse XFA structure");
        let script_result = ScriptExecutor::execute(&nodes);
        ScriptExecutor::apply_presence_changes(&mut nodes, &script_result.presence_changes);
        let flattened = Flattened::from_xfa(&nodes, &script_result.computed_values)
            .expect("Failed to flatten XFA");

        let mut doc = Document::from_flattened(&flattened);

        // Run modules up to and including FieldTableDetector (but not GridTemplateDetector)
        NoPrintDetector::new().process(&mut doc);
        MasterPageDetector::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        DateFieldDetector::new().process(&mut doc);
        RadioButtonDetector::new().process(&mut doc);
        RadioButtonGrouper::new().process(&mut doc);
        FieldTableDetector::new().process(&mut doc);

        // Find all GridLayout rows with LabeledField children created by FieldTableDetector
        let field_table_rows: Vec<_> = doc
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                // Only check GridLayouts created by FieldTableDetector
                if let GroupKind::GridLayout { .. } = &g.kind {
                    if let GroupSource::Inferred { module } = &g.source {
                        if module == "FieldTableDetector" {
                            return g.children.iter().any(|&child_idx| {
                                doc.get_group(child_idx)
                                    .map(|child| {
                                        matches!(child.kind, GroupKind::LabeledField { .. })
                                    })
                                    .unwrap_or(false)
                            });
                        }
                    }
                }
                false
            })
            .collect();

        // Check that none of the field table rows have "Nachname" or "Vorname" as labels
        // These are regular form labels, not bold table headers
        for (idx, group) in &field_table_rows {
            for &child_idx in &group.children {
                if let Some(child) = doc.get_group(child_idx) {
                    if let GroupKind::LabeledField { label, .. } = child.kind {
                        let label_idx = child.children[label];
                        let label_text = doc.get_text_content(label_idx).to_lowercase();
                        assert!(
                            !label_text.contains("nachname") && !label_text.contains("vorname"),
                            "FieldTableDetector should not create table row {} with 'Nachname' or 'Vorname' as labels - these are not bold headers. Found: {}",
                            idx,
                            label_text
                        );
                    }
                }
            }
        }

        // Also verify we DO detect the expected tables (Fondsprovider, ISIN, Satz, Ab)
        let mut found_fondsprovider = false;
        let mut found_isin = false;
        for (_, group) in &field_table_rows {
            for &child_idx in &group.children {
                if let Some(child) = doc.get_group(child_idx) {
                    if let GroupKind::LabeledField { label, .. } = child.kind {
                        let label_idx = child.children[label];
                        let label_text = doc.get_text_content(label_idx).to_lowercase();
                        if label_text.contains("fondsprovider") {
                            found_fondsprovider = true;
                        }
                        if label_text.contains("isin") {
                            found_isin = true;
                        }
                    }
                }
            }
        }
        assert!(
            found_fondsprovider,
            "Should detect tables with 'Fondsprovider' header"
        );
        assert!(found_isin, "Should detect tables with 'ISIN' header");
    }
}
