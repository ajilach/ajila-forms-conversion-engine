//! Caption field labeler module.
//!
//! Some XFA forms put a field's label in a *separate* non-interactive text field
//! (`access="nonInteractive"`) placed next to the input rather than in the
//! input's own `<caption>`. The canonical case is a date input with its label in
//! a read-only text field positioned directly below it:
//!
//! ```text
//!   [ date input            ]   [ date input            ]
//!   Date of the month-end ...   Expiry of the month-end ...
//! ```
//!
//! These caption fields are not picked up by [`LabelAttacher`](super::LabelAttacher):
//! they are field leaves, not text blocks, so they are invisible as label
//! candidates. The input would then grab an unrelated nearby paragraph (e.g. an
//! introductory disclaimer above the section) while the caption renders as stray
//! static text.
//!
//! This module runs *before* `LabelAttacher` and pairs each interactive field
//! with the nearest non-interactive caption field that lives in the **same
//! subform**. The same-subform constraint is the key disambiguator: it keeps a
//! caption bound to the input it actually belongs to even when the surrounding
//! geometry is ambiguous (a closer-looking paragraph in a sibling subform must
//! not win). Once paired into a `LabeledField`, the input is no longer a bare
//! root field, so `LabelAttacher` leaves it alone.

use super::AnalysisModule;
use crate::document::Document;
use crate::flattened::{Bounds, FlattenedNodeKind};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashMap;

/// Pairs interactive fields with adjacent non-interactive caption fields.
pub struct CaptionFieldLabeler {
    /// Maximum vertical gap for a caption above/below its field.
    pub vertical_threshold: Decimal,
    /// Maximum horizontal gap for a caption left/right of its field.
    pub horizontal_threshold: Decimal,
    /// Alignment tolerance (overlap / same-line).
    pub tolerance: Decimal,
}

impl Default for CaptionFieldLabeler {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptionFieldLabeler {
    pub fn new() -> Self {
        CaptionFieldLabeler {
            vertical_threshold: Decimal::from_str("20.0").unwrap(),
            horizontal_threshold: Decimal::from_str("150.0").unwrap(),
            tolerance: Decimal::from_str("8.0").unwrap(),
        }
    }

    /// SOM path of the parent subform of a group, used to require that a caption
    /// and its field are siblings. Returns `None` when no SOM path is available.
    fn parent_subform(doc: &Document, group_idx: usize) -> Option<String> {
        let path = doc.som_path(group_idx)?;
        path.parent().map(|p| p.into_string())
    }

    /// If `group_idx` is a non-interactive field leaf carrying static caption
    /// text (its value), return that text. Otherwise `None`.
    fn caption_text(doc: &Document, group_idx: usize) -> Option<String> {
        let node = doc
            .collect_nodes(group_idx)
            .into_iter()
            .find(|n| matches!(n.kind, FlattenedNodeKind::Field { .. }))?;
        if node.is_interactive() {
            return None;
        }
        if let FlattenedNodeKind::Field { value, .. } = &node.kind {
            if value.trim().is_empty() {
                None
            } else {
                Some(value.clone())
            }
        } else {
            None
        }
    }

    /// Gap between a caption and a field if the caption is positioned adjacent to
    /// the field (below, then left/right/above, in preference order). `None` when
    /// the caption is not adjacent within the configured thresholds.
    fn caption_gap(&self, caption: &Bounds, field: &Bounds) -> Option<Decimal> {
        // Captions for these stacked layouts sit directly below the input, so
        // prefer "below"; fall back to the other directions for robustness.
        caption
            .is_below_within(field, self.vertical_threshold, self.tolerance)
            .or_else(|| caption.is_right_within(field, self.horizontal_threshold, self.tolerance))
            .or_else(|| caption.is_left_within(field, self.horizontal_threshold, self.tolerance))
            .or_else(|| caption.is_above_within(field, self.vertical_threshold, self.tolerance))
    }
}

impl AnalysisModule for CaptionFieldLabeler {
    fn name(&self) -> &'static str {
        "CaptionFieldLabeler"
    }

    fn process(&self, doc: &mut Document) {
        let roots = doc.roots();

        // Interactive field groups, indexed by parent subform.
        let mut fields_by_subform: HashMap<String, Vec<usize>> = HashMap::new();
        for &idx in &roots {
            if doc.is_field(idx) {
                if let Some(parent) = Self::parent_subform(doc, idx) {
                    fields_by_subform.entry(parent).or_default().push(idx);
                }
            }
        }
        if fields_by_subform.is_empty() {
            return;
        }

        // Caption fields: unclaimed non-interactive field leaves with text.
        let mut captions: Vec<usize> = doc
            .unclaimed_field_leaves()
            .into_iter()
            .filter(|&idx| Self::caption_text(doc, idx).is_some())
            .collect();
        // Deterministic processing order (top-to-bottom, left-to-right).
        captions.sort_by(|&a, &b| match (doc.get_bounds(a), doc.get_bounds(b)) {
            (Some(a), Some(b)) => a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)),
            _ => std::cmp::Ordering::Equal,
        });

        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut used_fields: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for caption_idx in captions {
            let Some(parent) = Self::parent_subform(doc, caption_idx) else {
                continue;
            };
            let Some(candidates) = fields_by_subform.get(&parent) else {
                continue;
            };
            let Some(caption_bounds) = doc.get_bounds(caption_idx) else {
                continue;
            };

            // Closest unused sibling field that this caption sits adjacent to.
            let mut best: Option<(usize, Decimal)> = None;
            for &field_idx in candidates {
                if used_fields.contains(&field_idx) {
                    continue;
                }
                let Some(field_bounds) = doc.get_bounds(field_idx) else {
                    continue;
                };
                if let Some(gap) = self.caption_gap(&caption_bounds, &field_bounds) {
                    if best.map(|(_, b)| gap < b).unwrap_or(true) {
                        best = Some((field_idx, gap));
                    }
                }
            }

            if let Some((field_idx, _)) = best {
                used_fields.insert(field_idx);
                // LabeledField expects (label, field) order.
                pairs.push((caption_idx, field_idx));
            }
        }

        for (caption_idx, field_idx) in pairs {
            doc.create_labeled_field(caption_idx, field_idx, self.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::GroupKind;
    use crate::document::modules::{FieldGrouper, TextBlockGrouper};
    use crate::flattened::{FieldAccess, FlattenedNode, Hint, Page};
    use crate::xfa::num;
    use crate::xfa::scripting::SomPath;

    /// Build an interactive field with a SOM path.
    fn interactive_field(name: &str, som: &str, x: f64, y: f64, w: f64, h: f64) -> FlattenedNode {
        FlattenedNode::builder()
            .bounds(num(x), num(y), num(w), num(h))
            .field(name.to_string(), String::new(), String::new())
            .hint(Hint::SomPath(SomPath::new(som.to_string())))
            .build()
    }

    /// Build a non-interactive caption field carrying its label as a value.
    fn caption_field(value: &str, som: &str, x: f64, y: f64, w: f64, h: f64) -> FlattenedNode {
        FlattenedNode::builder()
            .bounds(num(x), num(y), num(w), num(h))
            .field(String::new(), value.to_string(), String::new())
            .hint(Hint::FieldBehavior {
                access: FieldAccess::NonInteractive,
                multiline: false,
                max_length: None,
                comb_cells: None,
            })
            .hint(Hint::SomPath(SomPath::new(som.to_string())))
            .build()
    }

    #[test]
    fn pairs_caption_below_field_in_same_subform() {
        // Date input with its label in a non-interactive caption directly below,
        // both children of the `Date_Expiry` subform.
        let flattened = crate::flattened::Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                interactive_field(
                    "Date_month_end",
                    "Root.Date_Expiry.Date_month_end",
                    0.0,
                    0.0,
                    85.0,
                    4.0,
                ),
                caption_field(
                    "Date of the month-end balance statement",
                    "Root.Date_Expiry.Date_month_statement",
                    0.0,
                    4.0,
                    85.0,
                    6.0,
                ),
            ],
        );
        let mut doc = crate::document::Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        CaptionFieldLabeler::new().process(&mut doc);

        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 1, "caption should be paired with the field");

        // The label child must be the caption field, whose text lives in its
        // value (Document::get_label_text only reads Text nodes; the converter
        // reads the field value — see structured_converter).
        let label_group = doc.get_label_group(labeled[0]).expect("label group");
        let label_value = doc
            .collect_nodes(label_group)
            .into_iter()
            .find_map(|n| match &n.kind {
                FlattenedNodeKind::Field { value, .. } => Some(value.clone()),
                _ => None,
            });
        assert_eq!(
            label_value.as_deref(),
            Some("Date of the month-end balance statement")
        );
    }

    #[test]
    fn does_not_pair_caption_in_different_subform() {
        // A non-interactive caption in a sibling subform must NOT be attached.
        let flattened = crate::flattened::Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                interactive_field(
                    "Date_month_end",
                    "Root.Date_Expiry.Date_month_end",
                    0.0,
                    0.0,
                    85.0,
                    4.0,
                ),
                caption_field(
                    "Unrelated note",
                    "Root.Other_Section.Note",
                    0.0,
                    4.0,
                    85.0,
                    6.0,
                ),
            ],
        );
        let mut doc = crate::document::Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        CaptionFieldLabeler::new().process(&mut doc);

        let labeled: Vec<_> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                doc.get_group(idx)
                    .map(|g| matches!(g.kind, GroupKind::LabeledField { .. }))
                    .unwrap_or(false)
            })
            .collect();
        assert!(labeled.is_empty(), "cross-subform caption must not pair");
    }
}
