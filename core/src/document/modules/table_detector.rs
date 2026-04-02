//! Table detector module.
//!
//! Detects text-only tables by analyzing horizontal borders and spatial alignment.
//! Tables are identified by text blocks that have horizontal borders (row separators)
//! and are aligned in a grid pattern.
//!
//! Detection strategy:
//! 1. Find text blocks with visible horizontal borders (top and/or bottom)
//! 2. Cluster bordered blocks into rows based on Y-position
//! 3. Determine column structure from X-position alignment within rows
//! 4. Build tables from contiguous groups of bordered rows

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

    /// Cluster blocks into rows based on similar Y-positions.
    fn cluster_rows(&self, blocks: &[(usize, Bounds)]) -> Vec<Vec<(usize, Bounds)>> {
        if blocks.is_empty() {
            return vec![];
        }

        // Sort by Y position (top of block)
        let mut sorted: Vec<_> = blocks.to_vec();
        sorted.sort_by(|a, b| a.1.y.cmp(&b.1.y));

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
                        current_row.sort_by(|a, b| a.1.x.cmp(&b.1.x));
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
            current_row.sort_by(|a, b| a.1.x.cmp(&b.1.x));
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
            let row_bottom = row.iter().map(|(_, b)| b.bottom()).max().unwrap_or(Num::ZERO);

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

    /// Build a table from a detected region.
    fn build_table(
        &self,
        doc: &Document,
        rows: &[Vec<(usize, Bounds)>],
        region: &[usize],
    ) -> Option<(Vec<usize>, usize, bool)> {
        if region.is_empty() {
            return None;
        }

        // Determine column count from first row
        let first_row = &rows[region[0]];
        let num_cols = first_row.len();

        if num_cols < self.min_columns {
            return None;
        }

        // Collect all children in row-major order
        let mut children: Vec<usize> = vec![];
        let mut has_header = false;

        for (i, &row_idx) in region.iter().enumerate() {
            let row = &rows[row_idx];

            // Skip rows that don't have the right number of items
            if row.len() != num_cols {
                continue;
            }

            // Check if first row is a header (all cells are bold)
            if i == 0 {
                has_header = row.iter().all(|(idx, _)| doc.is_bold_group(*idx));
            }

            for (idx, _) in row {
                children.push(*idx);
            }
        }

        // Verify we have enough cells for a valid table
        let actual_rows = children.len() / num_cols;
        if actual_rows < self.min_rows {
            return None;
        }

        Some((children, num_cols, has_header))
    }

    fn process_tables(&self, doc: &mut Document) {
        // Find blocks with horizontal borders (primary table detection)
        let bordered_blocks = self.find_bordered_blocks(doc);
        
        if bordered_blocks.len() < self.min_columns * self.min_rows {
            return;
        }

        // Cluster into rows
        let rows = self.cluster_rows(&bordered_blocks);

        // Find contiguous table regions
        let regions = self.detect_table_regions(&rows);

        // Track which groups are claimed by tables
        let mut claimed: HashSet<usize> = HashSet::new();

        // Build tables from detected regions
        for region in regions {
            if let Some((children, num_cols, has_header)) = self.build_table(doc, &rows, &region) {
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
}

impl AnalysisModule for TableDetector {
    fn process(&self, doc: &mut Document) {
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
