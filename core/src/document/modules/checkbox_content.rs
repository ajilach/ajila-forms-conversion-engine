//! Detects inset content below checkboxes and wraps it in `CheckboxContent`
//! groups so the structured converter can render it as checked-only content.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::xfa::scripting::SomPath;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects inset content below checkboxes.
pub struct CheckboxContentDetector;

impl Default for CheckboxContentDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckboxContentDetector {
    pub fn new() -> Self {
        Self
    }

    fn get_field_bounds(
        &self,
        doc: &Document,
        checkbox_idx: usize,
    ) -> Option<crate::flattened::Bounds> {
        let group = doc.get_group(checkbox_idx)?;
        if let GroupKind::Checkbox { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            doc.get_bounds(field_group_idx)
        } else {
            None
        }
    }

    fn extract_checkbox_som_path(&self, doc: &Document, checkbox_idx: usize) -> Option<SomPath> {
        let group = doc.get_group(checkbox_idx)?;
        if let GroupKind::Checkbox { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            return doc.som_path(field_group_idx);
        }
        None
    }
}

impl AnalysisModule for CheckboxContentDetector {
    fn name(&self) -> &'static str {
        "CheckboxContentDetector"
    }

    fn process(&self, doc: &mut Document) {
        let inset_threshold = Decimal::from_str("10.0").unwrap();
        let alignment_tolerance = Decimal::from_str("10.0").unwrap();
        let max_block_height = Decimal::from_str("120.0").unwrap();

        let checkbox_indices = doc.find_groups(|k| matches!(k, GroupKind::Checkbox { .. }));

        let checkbox_bounds: Vec<(usize, crate::flattened::Bounds)> = checkbox_indices
            .iter()
            .filter_map(|&checkbox_idx| {
                self.get_field_bounds(doc, checkbox_idx)
                    .map(|bounds| (checkbox_idx, bounds))
            })
            .collect();

        for checkbox_idx in checkbox_indices {
            let Some(field_bounds) = self.get_field_bounds(doc, checkbox_idx) else {
                continue;
            };
            let Some(checkbox_som_path) = self.extract_checkbox_som_path(doc, checkbox_idx) else {
                continue;
            };

            let checkbox_top = field_bounds.top();
            let checkbox_left = field_bounds.left();
            let inset_left = checkbox_left + inset_threshold;

            let next_checkbox_top = checkbox_bounds
                .iter()
                .filter(|(other_idx, other_bounds)| {
                    *other_idx != checkbox_idx
                        && other_bounds.top() > checkbox_top
                        && field_bounds.is_vertically_aligned(other_bounds, alignment_tolerance)
                })
                .map(|(_, bounds)| bounds.top())
                .min();

            if let Some(y_end) = next_checkbox_top
                && y_end - checkbox_top > max_block_height
            {
                continue;
            }

            let collected: Vec<usize> = if let Some(y_end) = next_checkbox_top {
                let mut roots = doc
                    .roots()
                    .into_iter()
                    .filter(|&root_idx| root_idx != checkbox_idx)
                    .filter(|&root_idx| {
                        let Some(bounds) = doc.get_bounds(root_idx) else {
                            return false;
                        };
                        bounds.top() >= checkbox_top
                            && bounds.top() < y_end
                            && bounds.bottom() <= y_end
                            && bounds.left() >= checkbox_left
                    })
                    .collect::<Vec<_>>();
                roots.sort_by(|&a, &b| {
                    let ya = doc
                        .get_bounds(a)
                        .map(|bounds| bounds.top())
                        .unwrap_or_default();
                    let yb = doc
                        .get_bounds(b)
                        .map(|bounds| bounds.top())
                        .unwrap_or_default();
                    ya.partial_cmp(&yb).unwrap_or(std::cmp::Ordering::Equal)
                });
                roots
            } else {
                let mut candidates: Vec<(usize, Decimal)> = doc
                    .roots()
                    .into_iter()
                    .filter(|&root_idx| root_idx != checkbox_idx)
                    .filter_map(|root_idx| {
                        let bounds = doc.get_bounds(root_idx)?;
                        if bounds.top() > checkbox_top
                            && bounds.top() <= checkbox_top + max_block_height
                        {
                            Some((root_idx, bounds.top()))
                        } else {
                            None
                        }
                    })
                    .collect();

                candidates
                    .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

                let mut roots = Vec::new();
                for (root_idx, _) in candidates {
                    let bounds = doc.get_bounds(root_idx).unwrap();
                    if bounds.left() >= inset_left {
                        roots.push(root_idx);
                    } else {
                        break;
                    }
                }
                roots
            };

            // Guard against over-capturing general flow content: only keep a checkbox
            // content block if there is real inset content to the right of the checkbox.
            let has_inset_content = collected.iter().any(|&root_idx| {
                doc.get_bounds(root_idx)
                    .map(|bounds| bounds.left() >= inset_left)
                    .unwrap_or(false)
            });

            if !has_inset_content {
                continue;
            }

            // Be conservative: only wrap checkbox content blocks that include
            // nested radio-group controls. This covers known conditional
            // checkbox sections (e.g. AAGZ authorization details) without
            // swallowing larger non-conditional layout regions.
            let has_nested_radio_group = collected.iter().any(|&root_idx| {
                doc.get_group(root_idx)
                    .map(|group| matches!(group.kind, GroupKind::RadioButtonGroup))
                    .unwrap_or(false)
            });

            if !has_nested_radio_group {
                continue;
            }

            if collected.is_empty() {
                continue;
            }

            doc.merge(
                collected,
                GroupKind::CheckboxContent { checkbox_som_path },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}
