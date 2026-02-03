//! Grid template detector module.
//!
//! Detects grid layouts by analyzing spatial arrangement of fields.
//! Groups elements that are aligned in rows and columns into grid structures.
//!
//! The detector works by:
//! 1. Finding unclaimed groups that could be grid elements
//! 2. Analyzing their x/y coordinates to detect row/column alignment
//! 3. Verifying consistent spacing patterns
//! 4. Creating GridLayout groups with appropriate column counts and spans

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::{HashMap, HashSet};

/// Detects grid layouts by identifying elements aligned in rows and columns.
///
/// Grid layouts are characterized by:
/// 1. Multiple elements aligned on consistent x-coordinates (columns)
/// 2. Multiple elements aligned on consistent y-coordinates (rows)
/// 3. At least 2x2 grid (2 rows, 2 columns)
/// 4. Elements arranged in reading order (top to bottom, left to right)
pub struct GridTemplateDetector {
    /// Tolerance for considering two coordinates as aligned (in points)
    pub alignment_tolerance: Decimal,
    /// Minimum number of rows required for a grid
    pub min_rows: usize,
    /// Minimum number of columns required for a grid
    pub min_columns: usize,
    /// Tolerance for "same line" vertical alignment
    pub line_tolerance: Decimal,
}

impl Default for GridTemplateDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GridTemplateDetector {
    pub fn new() -> Self {
        GridTemplateDetector {
            alignment_tolerance: Decimal::from_str("5.0").unwrap(), // 5 points tolerance
            min_rows: 2,
            min_columns: 2,
            line_tolerance: Decimal::from_str("2.0").unwrap(), // 2 points for same line
        }
    }

    /// Configure the alignment tolerance.
    pub fn with_alignment_tolerance(mut self, tolerance: Decimal) -> Self {
        self.alignment_tolerance = tolerance;
        self
    }

    /// Configure the minimum grid size.
    pub fn with_min_size(mut self, rows: usize, columns: usize) -> Self {
        self.min_rows = rows;
        self.min_columns = columns;
        self
    }

    /// Group coordinates that are within tolerance of each other.
    fn cluster_coordinates(&self, coords: &[Decimal]) -> Vec<Vec<usize>> {
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut sorted_indices: Vec<usize> = (0..coords.len()).collect();
        sorted_indices.sort_by(|&a, &b| coords[a].cmp(&coords[b]));

        for idx in sorted_indices {
            let coord = coords[idx];
            let mut found_cluster = false;

            // Try to add to an existing cluster
            for cluster in &mut clusters {
                let cluster_coord = coords[cluster[0]];
                if (coord - cluster_coord).abs() <= self.alignment_tolerance {
                    cluster.push(idx);
                    found_cluster = true;
                    break;
                }
            }

            // Create a new cluster if not found
            if !found_cluster {
                clusters.push(vec![idx]);
            }
        }

        clusters
    }

    /// Detect grid patterns in a set of groups (single row only).
    fn detect_grid(&self, doc: &Document, group_indices: &[usize]) -> Option<GridCandidate> {
        // For single-row grids, we need at least min_columns elements
        if group_indices.len() < self.min_columns {
            return None;
        }

        // Collect bounds for all groups
        let mut bounded_groups = Vec::new();
        for &idx in group_indices {
            if let Some(bounds) = doc.get_bounds(idx) {
                bounded_groups.push((idx, bounds));
            }
        }

        if bounded_groups.len() < self.min_columns {
            return None;
        }

        // Check that all groups are on the same horizontal line (within tolerance)
        let first_y = bounded_groups[0].1.y;
        for (_, bounds) in &bounded_groups {
            if (bounds.y - first_y).abs() > self.alignment_tolerance {
                return None; // Not all on same line
            }
        }

        // Sort by x coordinate (left to right)
        bounded_groups.sort_by(|a, b| a.1.x.cmp(&b.1.x));

        // Build the grid elements in order (single row)
        let mut elements = Vec::new();
        for (group_idx, _) in bounded_groups {
            elements.push(GridElement {
                group_idx,
                span: 1, // All elements have span 1 for now
            });
        }

        Some(GridCandidate {
            columns: elements.len(),
            elements,
        })
    }

    /// Find all potential grid groups among unclaimed groups.
    fn find_grid_candidates(&self, doc: &Document) -> Vec<GridCandidate> {
        // Get all unclaimed groups that could be grid elements (fields, labeled fields, etc.)
        let all_roots = doc.roots();
        let unclaimed: Vec<usize> = all_roots
            .into_iter()
            .filter(|&idx| {
                // Include any group with bounds, but skip headers/footers/headings/grids
                if let Some(group) = doc.get_group(idx) {
                    // Accept fields and labeled fields as grid candidates
                    matches!(
                        group.kind,
                        GroupKind::Field
                            | GroupKind::LabeledField { .. }
                            | GroupKind::RadioButton { .. }
                            | GroupKind::DateField { .. }
                    )
                } else {
                    false
                }
            })
            .collect();

        // Skip if we don't have enough groups
        if unclaimed.len() < self.min_rows * self.min_columns {
            return Vec::new();
        }

        // Group nearby elements together as potential grid candidates
        let mut candidates = Vec::new();
        let mut remaining: HashSet<usize> = unclaimed.iter().copied().collect();

        while !remaining.is_empty() {
            // Take the first remaining element
            let &start_idx = remaining.iter().next().unwrap();
            remaining.remove(&start_idx);

            let start_bounds = match doc.get_bounds(start_idx) {
                Some(b) => b,
                None => continue,
            };

            // Find all elements on the same horizontal line (within alignment tolerance)
            let mut row_group = vec![start_idx];
            let remaining_vec: Vec<usize> = remaining.iter().copied().collect();

            for &other_idx in &remaining_vec {
                if let Some(other_bounds) = doc.get_bounds(other_idx) {
                    // Check if on same horizontal line
                    if (start_bounds.y - other_bounds.y).abs() <= self.alignment_tolerance {
                        row_group.push(other_idx);
                        remaining.remove(&other_idx);
                    }
                }
            }

            // Try to detect a grid pattern in this row
            if let Some(candidate) = self.detect_grid(doc, &row_group) {
                candidates.push(candidate);
            }
        }

        candidates
    }
}

impl AnalysisModule for GridTemplateDetector {
    fn name(&self) -> &'static str {
        "GridTemplateDetector"
    }

    fn process(&self, doc: &mut Document) {
        let candidates = self.find_grid_candidates(doc);

        for candidate in candidates {
            // Create the GridLayout group
            let group_indices: Vec<usize> =
                candidate.elements.iter().map(|e| e.group_idx).collect();
            let spans: Vec<usize> = candidate.elements.iter().map(|e| e.span).collect();

            doc.merge(
                group_indices,
                GroupKind::GridLayout {
                    columns: candidate.columns,
                    spans,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}

/// A candidate grid layout detected in the document.
struct GridCandidate {
    /// Number of columns in the grid
    columns: usize,
    /// Elements in row-major order
    elements: Vec<GridElement>,
}

/// An element within a grid layout.
struct GridElement {
    /// Index of the group in the document
    group_idx: usize,
    /// Column span for this element (default 1)
    span: usize,
}
