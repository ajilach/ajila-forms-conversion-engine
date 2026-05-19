//! Field-column table detector module.
//!
//! Detects "field-column tables": bordered single-column text rows where
//! field groups are aligned at the same Y positions, and bold text blocks
//! above the field columns serve as column headers.
//!
//! Pattern (visual per row):
//!   [ bordered text ] ... [ field ] [ field ]
//!
//! The text column becomes headings and each field gets labeled with its
//! column header text.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Bounds, FlattenedNode, FlattenedNodeKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

type Num = Decimal;

/// Tolerance for row clustering based on Y-position (in points).
const ROW_TOLERANCE: f64 = 5.0;

/// Detects field-column tables from bordered text blocks and aligned fields.
///
/// A field-column table is detected when:
/// 1. Single-cell bordered text rows exist (the "text column")
/// 2. Field groups are aligned at the same Y positions as the bordered rows
/// 3. Bold text blocks above the field columns serve as column headers
/// 4. All field columns have matching headers
///
/// Output: Text cells become headings, fields get labeled with their column
/// header text.
pub struct FieldColumnTableDetector {
    row_tolerance: Num,
}

impl Default for FieldColumnTableDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldColumnTableDetector {
    pub fn new() -> Self {
        FieldColumnTableDetector {
            row_tolerance: Decimal::from_str(&ROW_TOLERANCE.to_string()).unwrap(),
        }
    }

    /// Check if a flattened node has a visible horizontal border (top or bottom).
    fn has_horizontal_border(node: &FlattenedNode) -> bool {
        if let Some(border) = &node.style.border {
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
                if !doc.is_text_block(idx) || doc.is_heading(idx) {
                    return None;
                }

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

    /// Cluster blocks into rows based on similar Y-positions.
    fn cluster_rows(&self, blocks: &[(usize, Bounds)]) -> Vec<Vec<(usize, Bounds)>> {
        if blocks.is_empty() {
            return vec![];
        }

        let mut sorted: Vec<_> = blocks.to_vec();
        sorted.sort_by_key(|a| a.1.y);

        let mut rows: Vec<Vec<(usize, Bounds)>> = vec![];
        let mut current_row: Vec<(usize, Bounds)> = vec![];
        let mut current_y: Option<Num> = None;

        for (idx, bounds) in sorted {
            if let Some(y) = current_y {
                if (bounds.y - y).abs() <= self.row_tolerance {
                    current_row.push((idx, bounds));
                } else {
                    if !current_row.is_empty() {
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

    /// Detect field-column tables and apply mutations to the document.
    fn detect_field_column_tables(&self, doc: &mut Document) {
        let bordered_blocks = self.find_bordered_blocks(doc);
        if bordered_blocks.is_empty() {
            return;
        }

        let bordered_rows = self.cluster_rows(&bordered_blocks);

        // Only consider single-cell bordered rows (single text column)
        let single_col_rows: Vec<&Vec<(usize, Bounds)>> =
            bordered_rows.iter().filter(|row| row.len() == 1).collect();

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
                let row_center = row.text_bounds.y + row.text_bounds.height / Decimal::from(2);
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
                .filter(|bl| {
                    bl.bounds
                        .overlaps_horizontally(field_bounds, self.row_tolerance)
                })
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

        let header_node_indices: Vec<Vec<usize>> = header_nodes_per_col
            .into_iter()
            .map(|h| h.unwrap())
            .collect();

        // Collect mutations
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

        // Apply mutations
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
                        kind: GroupKind::Leaf {
                            node_index: node_idx,
                        },
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
}

impl AnalysisModule for FieldColumnTableDetector {
    fn name(&self) -> &'static str {
        "FieldColumnTableDetector"
    }

    fn process(&self, doc: &mut Document) {
        self.detect_field_column_tables(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_configuration() {
        let detector = FieldColumnTableDetector::new();
        assert_eq!(detector.row_tolerance, Decimal::from_str("5.0").unwrap());
    }
}
