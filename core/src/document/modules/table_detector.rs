//! Table detector module.
//!
//! Detects text-only tables by analyzing the spatial layout of text blocks.
//! Tables are identified when text blocks form a grid pattern.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashSet;

type Num = Decimal;

/// Tolerance for row clustering (in points).
const ROW_TOLERANCE: f64 = 15.0;

/// Tolerance for column alignment (in points).
const COLUMN_TOLERANCE: f64 = 30.0;

/// Minimum columns for a table.
const MIN_COLUMNS: usize = 2;

/// Minimum rows for a table.
const MIN_ROWS: usize = 2;

/// Maximum gap between table rows (in points).
const MAX_ROW_GAP: f64 = 50.0;

/// Detects text-only tables from spatially aligned text blocks.
pub struct TableDetector {
    row_tolerance: Num,
    column_tolerance: Num,
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
            column_tolerance: Decimal::from_str(&COLUMN_TOLERANCE.to_string()).unwrap(),
            min_columns: MIN_COLUMNS,
            min_rows: MIN_ROWS,
            max_row_gap: Decimal::from_str(&MAX_ROW_GAP.to_string()).unwrap(),
        }
    }

    fn is_bold_text(&self, doc: &Document, group_idx: usize) -> bool {
        doc.collect_nodes(group_idx).iter().any(|n| n.is_bold())
    }

    fn find_candidate_blocks(&self, doc: &Document) -> Vec<(usize, Bounds)> {
        doc.roots()
            .into_iter()
            .filter_map(|idx| {
                if doc.is_text_block(idx) && !doc.is_heading(idx) {
                    doc.get_bounds(idx).map(|b| (idx, b))
                } else {
                    None
                }
            })
            .collect()
    }

    fn cluster_rows(&self, blocks: &[(usize, Bounds)]) -> Vec<Vec<(usize, Bounds)>> {
        if blocks.is_empty() {
            return vec![];
        }

        let mut sorted: Vec<_> = blocks.to_vec();
        sorted.sort_by(|a, b| a.1.center_y().cmp(&b.1.center_y()));

        let mut rows: Vec<Vec<(usize, Bounds)>> = vec![];
        let mut current_row: Vec<(usize, Bounds)> = vec![];
        let mut current_y: Option<Num> = None;

        for (idx, bounds) in sorted {
            let center_y = bounds.center_y();

            if let Some(y) = current_y {
                if (center_y - y).abs() <= self.row_tolerance {
                    current_row.push((idx, bounds));
                } else {
                    if !current_row.is_empty() {
                        rows.push(std::mem::take(&mut current_row));
                    }
                    current_row.push((idx, bounds));
                    current_y = Some(center_y);
                }
            } else {
                current_row.push((idx, bounds));
                current_y = Some(center_y);
            }
        }

        if !current_row.is_empty() {
            rows.push(current_row);
        }

        for row in &mut rows {
            row.sort_by(|a, b| a.1.x.cmp(&b.1.x));
        }

        rows
    }

    fn detect_table_regions(&self, rows: &[Vec<(usize, Bounds)>]) -> Vec<Vec<usize>> {
        // Find contiguous groups of rows with 2+ items
        let mut regions: Vec<Vec<usize>> = vec![];
        let mut current: Vec<usize> = vec![];
        let mut last_y_bottom: Option<Num> = None;

        for (idx, row) in rows.iter().enumerate() {
            if row.len() < self.min_columns {
                if current.len() >= self.min_rows {
                    regions.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                last_y_bottom = None;
                continue;
            }

            let row_y = row.iter().map(|(_, b)| b.y).min().unwrap_or(Num::ZERO);
            let row_bottom = row.iter().map(|(_, b)| b.bottom()).max().unwrap_or(Num::ZERO);

            let is_contiguous = match last_y_bottom {
                Some(y) => (row_y - y) <= self.max_row_gap,
                None => true,
            };

            if is_contiguous {
                current.push(idx);
                last_y_bottom = Some(row_bottom);
            } else {
                if current.len() >= self.min_rows {
                    regions.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                current.push(idx);
                last_y_bottom = Some(row_bottom);
            }
        }

        if current.len() >= self.min_rows {
            regions.push(current);
        }

        regions
    }

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

        // Collect all children
        let mut children: Vec<usize> = vec![];
        let mut has_header = false;

        for (i, &row_idx) in region.iter().enumerate() {
            let row = &rows[row_idx];
            
            // Skip rows that don't have the right number of items
            if row.len() != num_cols {
                continue;
            }

            // Check if first row is header
            if i == 0 {
                has_header = row.iter().all(|(idx, _)| self.is_bold_text(doc, *idx));
            }

            for (idx, _) in row {
                children.push(*idx);
            }
        }

        if children.is_empty() || children.len() < num_cols * self.min_rows {
            return None;
        }

        Some((children, num_cols, has_header))
    }

    fn process_tables(&self, doc: &mut Document) {
        let candidates = self.find_candidate_blocks(doc);
        if candidates.len() < self.min_columns * self.min_rows {
            return;
        }

        let rows = self.cluster_rows(&candidates);
        let regions = self.detect_table_regions(&rows);

        let mut claimed: HashSet<usize> = HashSet::new();

        for region in regions {
            if let Some((children, num_cols, has_header)) = self.build_table(doc, &rows, &region) {
                if children.iter().any(|&idx| claimed.contains(&idx)) {
                    continue;
                }

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
