//! Footnote detector module.
//!
//! Detects footnote content on master pages. Footnotes are identified as
//! master-page Background text nodes with font size well below the body text
//! and starting with a digit (footnote marker).

use super::AnalysisModule;
use crate::document::{Document, Group, GroupKind};
use crate::flattened::FlattenedNodeKind;
use rust_decimal::prelude::ToPrimitive;

/// Detects footnote text on master pages.
///
/// Runs after `MasterPageDetector`. Scans for Background-classified leaf text
/// nodes with small font size that start with a digit. Creates a new parent
/// group with `GroupKind::Footnote` wrapping the detected leaf.
pub struct FootnoteDetector;

impl Default for FootnoteDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FootnoteDetector {
    pub fn new() -> Self {
        FootnoteDetector
    }

    /// Compute the body text font size (mode of all content-area text node sizes).
    fn compute_body_font_size(doc: &Document) -> Option<f32> {
        let mut sizes: Vec<f32> = Vec::new();

        for (idx, group) in doc.groups.iter().enumerate() {
            if let GroupKind::Leaf { node_index } = &group.kind {
                if doc.master_page_region(idx).is_some() {
                    continue;
                }
                if let Some(node) = doc.get_node(*node_index) {
                    if let FlattenedNodeKind::Text { font_size, .. } = &node.kind {
                        if let Some(size) = font_size.to_f32() {
                            sizes.push((size * 2.0).round() / 2.0);
                        }
                    }
                }
            }
        }

        if sizes.is_empty() {
            return None;
        }

        // Find mode (most common font size)
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut best_size = sizes[0];
        let mut best_count = 1usize;
        let mut current_size = sizes[0];
        let mut current_count = 1usize;

        for &size in &sizes[1..] {
            if (size - current_size).abs() < 0.01 {
                current_count += 1;
            } else {
                if current_count > best_count {
                    best_count = current_count;
                    best_size = current_size;
                }
                current_size = size;
                current_count = 1;
            }
        }
        if current_count > best_count {
            best_size = current_size;
        }

        Some(best_size)
    }
}

impl AnalysisModule for FootnoteDetector {
    fn process(&self, doc: &mut Document) {
        let body_size = match Self::compute_body_font_size(doc) {
            Some(s) => s,
            None => return,
        };

        let mut footnote_leaves = Vec::new();
        let mut seen_texts = std::collections::HashSet::new();

        // Find master-page Background leaf text nodes that look like footnotes:
        // - Font size well below body text (< 75% of body size)
        // - Text starts with a digit (footnote marker like "1 ...")
        //
        // We skip Footer-classified groups because those often contain
        // operational/instructional text that is small but not footnotes.
        //
        // The same footnote text may appear in multiple pageArea definitions,
        // so we deduplicate by text content to avoid repeated footnotes.
        for (idx, group) in doc.groups.iter().enumerate() {
            if let GroupKind::Leaf { node_index } = &group.kind {
                if let Some(region) = doc.master_page_region(idx) {
                    if region == crate::flattened::MasterPageRegion::Background {
                        if let Some(node) = doc.get_node(*node_index) {
                            if let FlattenedNodeKind::Text {
                                font_size, content, ..
                            } = &node.kind
                            {
                                if let Some(size) = font_size.to_f32() {
                                    let trimmed = content.trim_start();
                                    if size < body_size * 0.75
                                        && trimmed.starts_with(|c: char| c.is_ascii_digit())
                                        && seen_texts.insert(content.clone())
                                    {
                                        footnote_leaves.push(idx);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Wrap each detected leaf in a Footnote parent group.
        for idx in footnote_leaves {
            let new_parent = Group {
                kind: GroupKind::Footnote,
                children: vec![idx],
                source: crate::document::GroupSource::Inferred {
                    module: "FootnoteDetector".to_string(),
                },
            };
            doc.groups.push(new_parent);
        }
    }

    fn name(&self) -> &'static str {
        "FootnoteDetector"
    }
}
