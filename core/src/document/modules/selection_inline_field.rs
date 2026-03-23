//! Detects radio buttons / checkboxes with an associated inline field on the
//! same line and wraps the field in a `SelectionInlineField` group.
//!
//! # Pattern
//!
//! ```text
//! [checkbox/radio] [label] [field]
//! ```
//!
//! The field is made conditional on the checkbox being checked or the radio
//! option being selected. The label from the checkbox/radio is shared with
//! the field.
//!
//! # Algorithm
//!
//! For every `Checkbox` or `RadioButton` group:
//!
//! 1. Get the label text and field bounds from the checkbox/radio.
//! 2. Find unclaimed root `Field` groups on the same line, to the right.
//! 3. Collect any unclaimed root text blocks between the control and the field
//!    (e.g. "Nr." in "Nur für Benutzer Nr. [field]") – these are appended to
//!    the label.
//! 4. Merge the text blocks and the field into a `SelectionInlineField` group
//!    carrying the condition SOM path, optional radio option name, and the
//!    combined label text.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Bounds, Hint};
use crate::xfa::scripting::SomPath;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects inline conditional fields next to radio buttons / checkboxes.
pub struct SelectionInlineFieldDetector;

impl Default for SelectionInlineFieldDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionInlineFieldDetector {
    pub fn new() -> Self {
        Self
    }

    /// Extract the SOM path from a Checkbox field.
    fn extract_checkbox_som_path(&self, doc: &Document, cb_idx: usize) -> Option<SomPath> {
        let group = doc.get_group(cb_idx)?;
        if let GroupKind::Checkbox { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            let nodes = doc.collect_nodes(field_group_idx);
            for node in &nodes {
                for hint in &node.hints {
                    if let Hint::SomPath(path) = hint {
                        return Some(path.clone());
                    }
                }
            }
        }
        None
    }

    /// Extract (ExclGroupSomPath, field_name) from a RadioButton field.
    fn extract_radio_option_info(
        &self,
        doc: &Document,
        rb_idx: usize,
    ) -> Option<(SomPath, String)> {
        let group = doc.get_group(rb_idx)?;
        if let GroupKind::RadioButton { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            let nodes = doc.collect_nodes(field_group_idx);
            let mut excl_path: Option<SomPath> = None;
            let mut field_name: Option<String> = None;
            for node in &nodes {
                if let crate::flattened::FlattenedNodeKind::Field { name, .. } = &node.kind {
                    if field_name.is_none() {
                        field_name = Some(name.clone());
                    }
                }
                for hint in &node.hints {
                    if let Hint::ExclGroupSomPath(path) = hint {
                        if excl_path.is_none() {
                            excl_path = Some(path.clone());
                        }
                    }
                }
            }
            excl_path.zip(field_name)
        } else {
            None
        }
    }

    /// Get the label text from a Checkbox or RadioButton group.
    fn get_label_text(&self, doc: &Document, group_idx: usize) -> Option<String> {
        let group = doc.get_group(group_idx)?;
        let label_child_idx = match &group.kind {
            GroupKind::Checkbox { label, .. } => *group.children.get(*label)?,
            GroupKind::RadioButton { label, .. } => *group.children.get(*label)?,
            _ => return None,
        };
        let text = doc.get_text_content(label_child_idx);
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Minimum width of the inline field to be matched. This filters out small
    /// square fields (checkbox/radio indicators, date field components) that happen
    /// to be on the same line. Genuine text input fields are typically 100+ pt wide.
    const MIN_FIELD_WIDTH: &str = "30.0";

    /// Find an unclaimed root Field on the same line to the right of the control,
    /// along with any text blocks between them. Returns `(extra_text_indices, field_idx)`.
    fn find_inline_field(
        &self,
        doc: &Document,
        control_bounds: &Bounds,
        control_idx: usize,
        line_tolerance: Decimal,
    ) -> Option<(Vec<usize>, usize)> {
        let min_width = Decimal::from_str(Self::MIN_FIELD_WIDTH).unwrap();
        let roots = doc.roots();

        // Find root Fields on the same line, to the right of the control
        let mut candidate_fields: Vec<(usize, Decimal)> = roots
            .iter()
            .filter(|&&root_idx| root_idx != control_idx)
            .filter(|&&root_idx| doc.is_field(root_idx))
            .filter_map(|&root_idx| {
                let bounds = doc.get_bounds(root_idx)?;
                if bounds.is_on_same_line(control_bounds, line_tolerance)
                    && bounds.left() > control_bounds.left()
                    && bounds.width >= min_width
                {
                    Some((root_idx, bounds.left()))
                } else {
                    None
                }
            })
            .collect();

        // Take the closest field to the right
        candidate_fields.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let (field_idx, field_left) = candidate_fields.first().copied()?;

        // Collect any text blocks between the control and the field
        let control_right = control_bounds.right();
        let mut extra_text: Vec<(usize, Decimal)> = roots
            .iter()
            .filter(|&&root_idx| root_idx != control_idx && root_idx != field_idx)
            .filter(|&&root_idx| doc.is_text_block(root_idx))
            .filter_map(|&root_idx| {
                let bounds = doc.get_bounds(root_idx)?;
                if bounds.is_on_same_line(control_bounds, line_tolerance)
                    && bounds.left() >= control_right
                    && bounds.left() < field_left
                {
                    Some((root_idx, bounds.left()))
                } else {
                    None
                }
            })
            .collect();

        extra_text.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let extra_text_indices: Vec<usize> = extra_text.into_iter().map(|(idx, _)| idx).collect();

        Some((extra_text_indices, field_idx))
    }
}

impl AnalysisModule for SelectionInlineFieldDetector {
    fn name(&self) -> &'static str {
        "SelectionInlineFieldDetector"
    }

    fn process(&self, doc: &mut Document) {
        let line_tolerance = Decimal::from_str("8.0").unwrap();

        // Collect all detections first, then apply them (to avoid borrow issues).
        struct Detection {
            children: Vec<usize>,
            condition_som_path: SomPath,
            option_field_name: Option<String>,
            label_text: String,
            field_child_index: usize,
        }

        let mut detections: Vec<Detection> = Vec::new();
        // Track already-claimed field indices to prevent double assignment.
        let mut claimed_fields: std::collections::HashSet<usize> = std::collections::HashSet::new();

        // --- Checkboxes ---
        let checkbox_indices = doc.find_groups(|k| matches!(k, GroupKind::Checkbox { .. }));
        for cb_idx in checkbox_indices {
            let Some(cb_bounds) = doc.get_bounds(cb_idx) else {
                continue;
            };
            let Some(som_path) = self.extract_checkbox_som_path(doc, cb_idx) else {
                continue;
            };
            let Some(base_label) = self.get_label_text(doc, cb_idx) else {
                continue;
            };

            let Some((extra_text_indices, field_idx)) =
                self.find_inline_field(doc, &cb_bounds, cb_idx, line_tolerance)
            else {
                continue;
            };

            if claimed_fields.contains(&field_idx) {
                continue;
            }
            claimed_fields.insert(field_idx);

            // Build combined label: base_label + extra text
            let mut label = base_label;
            for &text_idx in &extra_text_indices {
                let text = doc.get_text_content(text_idx);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    label.push(' ');
                    label.push_str(trimmed);
                }
            }

            let mut children = extra_text_indices;
            let field_child_index = children.len();
            children.push(field_idx);

            detections.push(Detection {
                children,
                condition_som_path: som_path,
                option_field_name: None,
                label_text: label,
                field_child_index,
            });
        }

        // --- Radio Buttons ---
        let radio_indices = doc.find_groups(|k| matches!(k, GroupKind::RadioButton { .. }));
        for rb_idx in radio_indices {
            let Some(rb_bounds) = doc.get_bounds(rb_idx) else {
                continue;
            };
            let Some((excl_group_path, field_name)) = self.extract_radio_option_info(doc, rb_idx)
            else {
                continue;
            };
            let Some(base_label) = self.get_label_text(doc, rb_idx) else {
                continue;
            };

            let Some((extra_text_indices, field_idx)) =
                self.find_inline_field(doc, &rb_bounds, rb_idx, line_tolerance)
            else {
                continue;
            };

            if claimed_fields.contains(&field_idx) {
                continue;
            }
            claimed_fields.insert(field_idx);

            // Build combined label
            let mut label = base_label;
            for &text_idx in &extra_text_indices {
                let text = doc.get_text_content(text_idx);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    label.push(' ');
                    label.push_str(trimmed);
                }
            }

            let mut children = extra_text_indices;
            let field_child_index = children.len();
            children.push(field_idx);

            detections.push(Detection {
                children,
                condition_som_path: excl_group_path,
                option_field_name: Some(field_name),
                label_text: label,
                field_child_index,
            });
        }

        // Apply detections
        for det in detections {
            doc.merge(
                det.children,
                GroupKind::SelectionInlineField {
                    condition_som_path: det.condition_som_path,
                    option_field_name: det.option_field_name,
                    label_text: det.label_text,
                    field: det.field_child_index,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}
