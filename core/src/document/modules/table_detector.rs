//! Table detector module.
//!
//! Detects text-only tables by analyzing horizontal borders and spatial alignment.
//! Tables are identified by text blocks that have horizontal borders (row separators)
//! and are aligned in a grid pattern.
//!
//! Detection strategy:
//! 1. Find text blocks with visible horizontal borders (top and/or bottom)
//! 2. Cluster bordered blocks into rows based on Y-position
//! 3. For each bordered row, also include unbordered text blocks on the same row
//!    that fall within the table's X-extent
//! 4. Determine column structure from X-position alignment within rows
//! 5. Build tables from contiguous groups of bordered rows

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Bounds, FlattenedNode, FlattenedNodeKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashSet;

type Num = Decimal;

/// Tolerance for row clustering based on Y-position (in points).
const ROW_TOLERANCE: f64 = 5.0;

/// Minimum columns for a table.
const MIN_COLUMNS: usize = 2;

/// Minimum rows for a table.
const MIN_ROWS: usize = 2;

/// Maximum gap between table rows (in points).
const MAX_ROW_GAP: f64 = 60.0;

/// Detects text-only tables from bordered text blocks.
pub struct TableDetector {
    row_tolerance: Num,
    min_columns: usize,
    min_rows: usize,
    max_row_gap: Num,
}

impl Default for TableDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TableDetector {
    pub fn new() -> Self {
        TableDetector {
            row_tolerance: Decimal::from_str(&ROW_TOLERANCE.to_string()).unwrap(),
            min_columns: MIN_COLUMNS,
            min_rows: MIN_ROWS,
            max_row_gap: Decimal::from_str(&MAX_ROW_GAP.to_string()).unwrap(),
        }
    }

    /// Check if a flattened node has a visible horizontal border (top or bottom).
    fn has_horizontal_border(node: &FlattenedNode) -> bool {
        if let Some(border) = &node.style.border {
            // Check top edge (index 0) and bottom edge (index 2)
            let has_top = border.get_edge(0).is_some_and(|e| {
                e.presence != "hidden" && e.thickness.is_some_and(|t| t > Decimal::ZERO)
            });
            let has_bottom = border.get_edge(2).is_some_and(|e| {
                e.presence != "hidden" && e.thickness.is_some_and(|t| t > Decimal::ZERO)
            });
            has_top || has_bottom
        } else {
            false
        }
    }

    /// Find text blocks that have visible horizontal borders.
    fn find_bordered_blocks(&self, doc: &Document) -> Vec<(usize, Bounds)> {
        doc.roots()
            .into_iter()
            .filter_map(|idx| {
                // Only consider text blocks, not headings or fields
                if !doc.is_text_block(idx) || doc.is_heading(idx) {
                    return None;
                }

                // Check if any node in the group has a horizontal border
                let nodes = doc.collect_nodes(idx);
                let has_border = nodes.iter().any(|n| {
                    if let FlattenedNodeKind::Text { .. } = &n.kind {
                        Self::has_horizontal_border(n)
                    } else {
                        false
                    }
                });

                if has_border {
                    doc.get_bounds(idx).map(|b| (idx, b))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Find ALL text blocks (bordered or not) for potential table inclusion.
    fn find_all_text_blocks(&self, doc: &Document) -> Vec<(usize, Bounds)> {
        doc.roots()
            .into_iter()
            .filter_map(|idx| {
                // Only consider text blocks, not headings or fields
                if !doc.is_text_block(idx) || doc.is_heading(idx) {
                    return None;
                }
                doc.get_bounds(idx).map(|b| (idx, b))
            })
            .collect()
    }

    /// Cluster blocks into rows based on similar Y-positions.
    fn cluster_rows(&self, blocks: &[(usize, Bounds)]) -> Vec<Vec<(usize, Bounds)>> {
        if blocks.is_empty() {
            return vec![];
        }

        // Sort by Y position (top of block)
        let mut sorted: Vec<_> = blocks.to_vec();
        sorted.sort_by_key(|a| a.1.y);

        let mut rows: Vec<Vec<(usize, Bounds)>> = vec![];
        let mut current_row: Vec<(usize, Bounds)> = vec![];
        let mut current_y: Option<Num> = None;

        for (idx, bounds) in sorted {
            if let Some(y) = current_y {
                // Check if this block is on the same row (within tolerance)
                if (bounds.y - y).abs() <= self.row_tolerance {
                    current_row.push((idx, bounds));
                } else {
                    // Start a new row
                    if !current_row.is_empty() {
                        // Sort current row by X position before adding
                        current_row.sort_by_key(|a| a.1.x);
                        rows.push(std::mem::take(&mut current_row));
                    }
                    current_row.push((idx, bounds));
                    current_y = Some(bounds.y);
                }
            } else {
                current_row.push((idx, bounds));
                current_y = Some(bounds.y);
            }
        }

        if !current_row.is_empty() {
            current_row.sort_by_key(|a| a.1.x);
            rows.push(current_row);
        }

        rows
    }

    /// Detect contiguous table regions from clustered rows.
    /// A region is a group of consecutive rows with consistent column structure.
    fn detect_table_regions(&self, rows: &[Vec<(usize, Bounds)>]) -> Vec<Vec<usize>> {
        if rows.is_empty() {
            return vec![];
        }

        let mut regions: Vec<Vec<usize>> = vec![];
        let mut current_region: Vec<usize> = vec![];
        let mut last_row_bottom: Option<Num> = None;
        let mut expected_cols: Option<usize> = None;

        for (idx, row) in rows.iter().enumerate() {
            // Skip single-cell rows
            if row.len() < self.min_columns {
                // Flush current region if it's valid
                if current_region.len() >= self.min_rows {
                    regions.push(std::mem::take(&mut current_region));
                } else {
                    current_region.clear();
                }
                last_row_bottom = None;
                expected_cols = None;
                continue;
            }

            let row_y = row.iter().map(|(_, b)| b.y).min().unwrap_or(Num::ZERO);
            let row_bottom = row
                .iter()
                .map(|(_, b)| b.bottom())
                .max()
                .unwrap_or(Num::ZERO);

            // Check if this row is contiguous with the previous
            let is_contiguous = match last_row_bottom {
                Some(prev_bottom) => (row_y - prev_bottom).abs() <= self.max_row_gap,
                None => true,
            };

            // Check if column count matches expected
            let cols_match = match expected_cols {
                Some(cols) => row.len() == cols,
                None => true,
            };

            if is_contiguous && cols_match {
                current_region.push(idx);
                last_row_bottom = Some(row_bottom);
                if expected_cols.is_none() {
                    expected_cols = Some(row.len());
                }
            } else {
                // Flush current region if valid
                if current_region.len() >= self.min_rows {
                    regions.push(std::mem::take(&mut current_region));
                } else {
                    current_region.clear();
                }

                // Start a new potential region
                current_region.push(idx);
                last_row_bottom = Some(row_bottom);
                expected_cols = Some(row.len());
            }
        }

        // Don't forget the last region
        if current_region.len() >= self.min_rows {
            regions.push(current_region);
        }

        regions
    }

    /// For a given table region (defined by bordered rows), find all text blocks
    /// that should be included in the table rows (including unbordered ones).
    fn expand_rows_with_unbordered_blocks(
        &self,
        _doc: &Document,
        bordered_rows: &[Vec<(usize, Bounds)>],
        region: &[usize],
        all_text_blocks: &[(usize, Bounds)],
    ) -> Vec<Vec<(usize, Bounds)>> {
        if region.is_empty() {
            return vec![];
        }

        // Get the bordered blocks in this region
        let _bordered_in_region: HashSet<usize> = region
            .iter()
            .flat_map(|&row_idx| bordered_rows[row_idx].iter().map(|(idx, _)| *idx))
            .collect();

        // Calculate the X-extent of the table (from leftmost to rightmost bordered block)
        let mut min_x = Num::MAX;
        let mut max_x = Num::MIN;
        for &row_idx in region {
            for (_, bounds) in &bordered_rows[row_idx] {
                if bounds.x < min_x {
                    min_x = bounds.x;
                }
                if bounds.right() > max_x {
                    max_x = bounds.right();
                }
            }
        }

        // Build expanded rows
        let mut expanded_rows: Vec<Vec<(usize, Bounds)>> = Vec::new();

        for &row_idx in region {
            let bordered_row = &bordered_rows[row_idx];
            if bordered_row.is_empty() {
                continue;
            }

            // Get the Y-position of this row
            let row_y = bordered_row[0].1.y;

            // Find all text blocks (bordered or not) that are on this row
            // and within the table's X-extent
            let mut row_blocks: Vec<(usize, Bounds)> = all_text_blocks
                .iter()
                .filter(|(_idx, bounds)| {
                    // Within Y tolerance
                    let y_match = (bounds.y - row_y).abs() <= self.row_tolerance;

                    // Block overlaps with table X-extent (at least partially within it)
                    let x_overlap = bounds.x < max_x && bounds.right() > min_x;

                    y_match && x_overlap
                })
                .cloned()
                .collect();

            // Sort by X position
            row_blocks.sort_by_key(|a| a.1.x);

            expanded_rows.push(row_blocks);
        }

        expanded_rows
    }

    /// Find all root Field groups with their bounds.
    fn find_root_field_groups(&self, doc: &Document) -> Vec<(usize, Bounds)> {
        doc.roots()
            .into_iter()
            .filter(|&idx| doc.is_field(idx))
            .filter_map(|idx| doc.get_bounds(idx).map(|b| (idx, b)))
            .collect()
    }

    /// Find bold root text blocks (potential column headers).
    fn find_bold_text_blocks(&self, doc: &Document) -> Vec<(usize, Bounds)> {
        doc.roots()
            .into_iter()
            .filter(|&idx| doc.is_text_block(idx) && !doc.is_heading(idx) && doc.is_bold_group(idx))
            .filter_map(|idx| doc.get_bounds(idx).map(|b| (idx, b)))
            .collect()
    }

    /// Detect "field-column tables": bordered single-column text rows where
    /// field groups are aligned at the same Y positions, and bold text blocks
    /// above the field columns serve as column headers.
    ///
    /// Pattern (visual per row):
    ///   [ bordered text ] ... [ field ] [ field ]
    ///
    /// The text column becomes headings and each field gets labeled with its
    /// column header text.
    fn process_field_column_tables(&self, doc: &mut Document) {
        // Phase 1: collect data (immutable borrows)
        let bordered_blocks = self.find_bordered_blocks(doc);
        if bordered_blocks.is_empty() {
            return;
        }

        let bordered_rows = self.cluster_rows(&bordered_blocks);

        // Only consider single-cell bordered rows (single text column)
        let single_col_rows: Vec<&Vec<(usize, Bounds)>> = bordered_rows
            .iter()
            .filter(|row| row.len() == 1)
            .collect();

        if single_col_rows.len() < 2 {
            return;
        }

        let field_groups = self.find_root_field_groups(doc);
        if field_groups.is_empty() {
            return;
        }

        // Build combined rows: single text block + fields on the same Y line.
        // Each field is assigned to its nearest row to avoid duplicates when
        // rows are close together.
        struct CombinedRow {
            text_idx: usize,
            text_bounds: Bounds,
            fields: Vec<(usize, Bounds)>,
        }

        let mut combined: Vec<CombinedRow> = single_col_rows
            .iter()
            .map(|text_row| {
                let (text_idx, text_bounds) = text_row[0];
                CombinedRow {
                    text_idx,
                    text_bounds,
                    fields: Vec::new(),
                }
            })
            .collect();

        // Assign each field to its closest row
        for &(field_idx, field_bounds) in &field_groups {
            let field_center = field_bounds.y + field_bounds.height / Decimal::from(2);
            let best_row = combined.iter_mut().min_by_key(|row| {
                let row_center =
                    row.text_bounds.y + row.text_bounds.height / Decimal::from(2);
                (field_center - row_center).abs()
            });
            if let Some(row) = best_row {
                let row_y = row.text_bounds.y;
                let row_bottom = row.text_bounds.bottom();
                if field_center >= row_y - self.row_tolerance
                    && field_center <= row_bottom + self.row_tolerance
                {
                    row.fields.push((field_idx, field_bounds));
                }
            }
        }

        for row in &mut combined {
            row.fields.sort_by_key(|(_, b)| b.x);
        }

        // Identify data rows (rows that have fields)
        let data_rows: Vec<&CombinedRow> =
            combined.iter().filter(|r| !r.fields.is_empty()).collect();

        if data_rows.len() < 2 {
            return;
        }

        // All data rows must have the same number of field columns
        let num_field_cols = data_rows[0].fields.len();
        if num_field_cols == 0 || !data_rows.iter().all(|r| r.fields.len() == num_field_cols) {
            return;
        }

        // Field columns must be clearly to the right of the text column.
        // This avoids triggering on forms where inline fields sit within or
        // adjacent to the bordered text blocks.
        let text_right = data_rows
            .iter()
            .map(|r| r.text_bounds.x + r.text_bounds.width)
            .max()
            .unwrap();
        let fields_start = data_rows[0].fields[0].1.x;
        if fields_start <= text_right + self.row_tolerance {
            return;
        }

        // Find column headers: bold sub-groups above the first data row whose
        // center-X is closest to each field column's center-X.
        // We match individual children of bold TextBlock groups (not whole groups)
        // because the TextBlockGrouper may have merged adjacent header texts
        // (e.g. "Alt" and "Neu") into a single TextBlock.
        let first_data_y = data_rows[0].text_bounds.y;
        let bold_blocks = self.find_bold_text_blocks(doc);

        struct BoldLeaf {
            node_indices: Vec<usize>,
            bounds: Bounds,
        }
        let mut bold_leaves: Vec<BoldLeaf> = Vec::new();
        for &(group_idx, _) in &bold_blocks {
            for &child_idx in &doc.groups[group_idx].children {
                if let Some(leaf_bounds) = doc.get_bounds(child_idx) {
                    if leaf_bounds.bottom() < first_data_y {
                        let node_indices = doc.collect_node_indices(child_idx);
                        if !node_indices.is_empty() {
                            bold_leaves.push(BoldLeaf {
                                node_indices,
                                bounds: leaf_bounds,
                            });
                        }
                    }
                }
            }
        }

        // For each field column, find the bold sub-group whose center-X is closest
        let mut header_nodes_per_col: Vec<Option<Vec<usize>>> = vec![None; num_field_cols];
        for col in 0..num_field_cols {
            let field_bounds = &data_rows[0].fields[col].1;
            let field_cx = field_bounds.x + field_bounds.width / Decimal::from(2);
            let best = bold_leaves
                .iter()
                .filter(|bl| bl.bounds.overlaps_horizontally(field_bounds, self.row_tolerance))
                .min_by_key(|bl| {
                    let leaf_cx = bl.bounds.x + bl.bounds.width / Decimal::from(2);
                    (leaf_cx - field_cx).abs()
                });
            header_nodes_per_col[col] = best.map(|bl| bl.node_indices.clone());
        }

        // All field columns must have a matching header
        if header_nodes_per_col.iter().any(|h| h.is_none()) {
            return;
        }

        let header_node_indices: Vec<Vec<usize>> =
            header_nodes_per_col.into_iter().map(|h| h.unwrap()).collect();

        // Phase 2: collect mutations
        struct RowMutation {
            text_group_idx: usize,
            field_mutations: Vec<(usize, usize)>, // (field_group_idx, column_index)
        }

        let mutations: Vec<RowMutation> = data_rows
            .iter()
            .map(|row| RowMutation {
                text_group_idx: row.text_idx,
                field_mutations: row
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, (field_idx, _))| (*field_idx, i))
                    .collect(),
            })
            .collect();

        // Phase 3: apply mutations
        for mutation in mutations {
            // Text cell → heading
            doc.create_heading(mutation.text_group_idx, 3, self.name());

            // Each field → LabeledField with a synthetic label from column header
            for (field_idx, col_idx) in mutation.field_mutations {
                let header_nodes = &header_node_indices[col_idx];

                let mut leaf_indices = Vec::new();
                for &node_idx in header_nodes {
                    let leaf_idx = doc.groups.len();
                    doc.groups.push(crate::document::Group {
                        kind: GroupKind::Leaf { node_index: node_idx },
                        children: vec![],
                        source: GroupSource::Inferred {
                            module: self.name().to_string(),
                        },
                    });
                    leaf_indices.push(leaf_idx);
                }

                let label_idx = doc.create_text_block(leaf_indices, self.name());
                doc.create_labeled_field(label_idx, field_idx, self.name());
            }
        }
    }

    fn process_tables(&self, doc: &mut Document) {
        // Find blocks with horizontal borders (primary table detection)
        let bordered_blocks = self.find_bordered_blocks(doc);

        if bordered_blocks.len() < self.min_columns * self.min_rows {
            return;
        }

        // Also get all text blocks for row expansion
        let all_text_blocks = self.find_all_text_blocks(doc);

        // Cluster bordered blocks into rows
        let bordered_rows = self.cluster_rows(&bordered_blocks);

        // Find contiguous table regions based on bordered blocks
        let regions = self.detect_table_regions(&bordered_rows);

        // Track which groups are claimed by tables
        let mut claimed: HashSet<usize> = HashSet::new();

        // Build tables from detected regions
        for region in regions {
            // Expand rows to include unbordered blocks within the table extent
            let expanded_rows = self.expand_rows_with_unbordered_blocks(
                doc,
                &bordered_rows,
                &region,
                &all_text_blocks,
            );

            if expanded_rows.is_empty() {
                continue;
            }

            // Determine column count from the row with the most columns
            // (this handles cases where some rows might have missing cells)
            let num_cols = expanded_rows.iter().map(|r| r.len()).max().unwrap_or(0);

            if num_cols < self.min_columns {
                continue;
            }

            // Check if rows have consistent column counts
            // (allow rows with exactly num_cols columns)
            let valid_rows: Vec<&Vec<(usize, Bounds)>> = expanded_rows
                .iter()
                .filter(|r| r.len() == num_cols)
                .collect();

            if valid_rows.len() < self.min_rows {
                continue;
            }

            // Collect all children in row-major order
            let mut children: Vec<usize> = vec![];
            let mut has_header = false;

            for (i, row) in valid_rows.iter().enumerate() {
                // Check if first row is a header (all cells are bold)
                if i == 0 {
                    has_header = row.iter().all(|(idx, _)| doc.is_bold_group(*idx));
                }

                for (idx, _) in *row {
                    children.push(*idx);
                }
            }

            // Skip if any child is already claimed
            if children.iter().any(|&idx| claimed.contains(&idx)) {
                continue;
            }

            // Create the table group
            doc.groups.push(crate::document::Group {
                kind: GroupKind::Table {
                    columns: num_cols,
                    has_header,
                },
                children: children.clone(),
                source: GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            });

            // Mark all children as claimed
            for child in children {
                claimed.insert(child);
            }
        }
    }
}

impl AnalysisModule for TableDetector {
    fn process(&self, doc: &mut Document) {
        self.process_field_column_tables(doc);
        self.process_tables(doc);
    }

    fn name(&self) -> &'static str {
        "TableDetector"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detector_creation() {
        let detector = TableDetector::new();
        assert_eq!(detector.min_columns, MIN_COLUMNS);
        assert_eq!(detector.min_rows, MIN_ROWS);
    }
}
