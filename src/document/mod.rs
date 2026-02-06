//! Document model for analyzing flattened PDF structure.
//!
//! The Document wraps a Flattened representation and builds up a hierarchy
//! of Groups through analysis modules. Every FlattenedNode starts as a Leaf
//! group, and modules merge groups into composite groups (TextBlock, LabeledField, etc.).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐         ┌──────────────────────────────────────────┐
//! │  Flattened  │◄────────│              Document                    │
//! │  (immutable)│         │  ┌────────────────────────────────────┐  │
//! │             │         │  │  groups: Vec<Group>                │  │
//! │  nodes[0]◄──┼─────────┼──│    [0] Leaf { node: 0 }            │  │
//! │  nodes[1]◄──┼─────────┼──│    [1] Leaf { node: 1 }            │  │
//! │  nodes[2]◄──┼─────────┼──│    [2] Leaf { node: 2 }            │  │
//! │             │         │  │    [3] TextBlock { children: [0,1] }│  │
//! │             │         │  │    [4] LabeledField { label:0, ... }│  │
//! └─────────────┘         │  └────────────────────────────────────┘  │
//!                         └──────────────────────────────────────────┘
//! ```
//!
//! # Claimed vs Unclaimed
//!
//! - **Unclaimed**: Leaf groups not referenced by any composite group
//! - **Claimed**: Leaf groups referenced (directly or transitively) by a composite group
//!
//! This is computed dynamically from the group structure - no separate tracking needed.
//!
//!
//!

pub mod modules;

use crate::flattened::{Bounds, Flattened, FlattenedNode, FlattenedNodeKind};
use crate::xfa::num;
use ab_glyph::PxScale;
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use rust_decimal::prelude::*;
use std::collections::HashSet;
use std::path::Path;

/// A Document wraps a Flattened representation and accumulates analysis results as Groups.
pub struct Document<'a> {
    /// Reference to the immutable flattened representation
    pub source: &'a Flattened,
    /// All groups - starts with one Leaf per node, grows as modules merge groups
    pub groups: Vec<Group>,
}

/// A Group represents either a single node (Leaf) or a composition of other groups.
#[derive(Debug, Clone)]
pub struct Group {
    /// What kind of group this is
    pub kind: GroupKind,
    /// Child group indices (empty for Leaf groups)
    pub children: Vec<usize>,
    /// Where this group came from
    pub source: GroupSource,
}

/// Where a group came from - either initially from flattening or inferred by a module.
#[derive(Debug, Clone)]
pub enum GroupSource {
    /// Group was created during initial flattening (Leaf groups)
    Initial,
    /// Group was directly from XFA structure
    Xfa,
    /// Group was inferred by an analysis module
    Inferred {
        /// Name of the module that created this group
        module: String,
    },
}

/// The kind of group - either a Leaf wrapping a node, or a composite kind.
#[derive(Debug, Clone)]
pub enum GroupKind {
    // ========================================================================
    // Leaf - wraps exactly one FlattenedNode
    // ========================================================================
    /// A leaf group wrapping a single FlattenedNode
    Leaf { node_index: usize },

    // ========================================================================
    // Composite kinds - children are other group indices
    // ========================================================================
    /// Unknown/unclassified composite group
    Unknown,

    /// Merged adjacent text nodes (children are Leaf or TextBlock groups)
    TextBlock,

    /// A field with its associated label
    LabeledField {
        /// Index into children vec for the label group
        label: usize,
        /// Index into children vec for the field group
        field: usize,
    },

    /// A radio button (square field) with its label on the right
    RadioButton {
        /// Index into children vec for the field group
        field: usize,
        /// Index into children vec for the label group
        label: usize,
    },

    /// A group of radio buttons on the same line (children are RadioButton groups)
    RadioButtonGroup,

    /// A date field composed of multiple input fields separated by delimiters
    DateField {
        /// Number of field components (e.g., 2 for month.year, 3 for day.month.year)
        num_fields: usize,
    },

    /// Exclusive group (radio buttons) - children are field Leaf groups
    ExclGroup {
        /// The currently selected value (if any)
        selected_value: Option<String>,
    },

    /// A heading with detected level
    Heading {
        /// Heading level (1 = h1, 2 = h2, etc.)
        level: u8,
    },

    /// A paragraph of text (children are text groups)
    Paragraph,

    /// A single field wrapped in its own group
    Field,

    /// A logical section of the document
    Section,

    /// Page header content
    Header,

    /// Page footer content
    Footer,

    /// A repeatable section (dynamic array/table) per XFA occur element
    RepeatableSection {
        /// Minimum occurrences required
        min_occurrences: u32,
        /// Maximum occurrences allowed (None = unlimited)
        max_occurrences: Option<u32>,
    },

    /// An inline field - a field with text directly before/after but no label above/below
    /// These are fields embedded in flowing text rather than traditional form layouts
    InlineField,

    /// Non-printable content (elements with relevant="-print")
    /// These are screen-only interactive elements like add/remove buttons
    /// that should not appear in print or structured output.
    NoPrint,

    /// A grid layout with elements arranged in rows and columns
    GridLayout {
        /// Number of columns in the grid
        columns: usize,
        /// Column span for each child element (in order)
        spans: Vec<usize>,
    },
}

impl<'a> Document<'a> {
    /// Create a new Document from a Flattened representation.
    ///
    /// Initializes one Leaf group per FlattenedNode.
    pub fn from_flattened(source: &'a Flattened) -> Self {
        // Collect all nodes from the recursive structure for index-based access
        let groups = source
            .iter_nodes()
            .enumerate()
            .map(|(i, _)| Group {
                kind: GroupKind::Leaf { node_index: i },
                children: vec![],
                source: GroupSource::Initial,
            })
            .collect();

        Document { source, groups }
    }

    // ========================================================================
    // Group creation
    // ========================================================================

    /// Merge multiple groups into a new composite group.
    ///
    /// Returns the index of the newly created group.
    pub fn merge(
        &mut self,
        child_indices: Vec<usize>,
        kind: GroupKind,
        source: GroupSource,
    ) -> usize {
        let new_index = self.groups.len();
        self.groups.push(Group {
            kind,
            children: child_indices,
            source,
        });
        new_index
    }

    /// Create a TextBlock group from multiple text groups.
    pub fn create_text_block(&mut self, child_indices: Vec<usize>, module: &str) -> usize {
        self.merge(
            child_indices,
            GroupKind::TextBlock,
            GroupSource::Inferred {
                module: module.to_string(),
            },
        )
    }

    /// Create a LabeledField group from a label group and a field group.
    pub fn create_labeled_field(
        &mut self,
        label_group: usize,
        field_group: usize,
        module: &str,
    ) -> usize {
        self.merge(
            vec![label_group, field_group],
            GroupKind::LabeledField { label: 0, field: 1 },
            GroupSource::Inferred {
                module: module.to_string(),
            },
        )
    }

    /// Create an ExclGroup from field groups.
    pub fn create_excl_group(
        &mut self,
        field_groups: Vec<usize>,
        selected_value: Option<String>,
        source: GroupSource,
    ) -> usize {
        self.merge(
            field_groups,
            GroupKind::ExclGroup { selected_value },
            source,
        )
    }

    /// Create a Heading group.
    pub fn create_heading(&mut self, content_group: usize, level: u8, module: &str) -> usize {
        self.merge(
            vec![content_group],
            GroupKind::Heading { level },
            GroupSource::Inferred {
                module: module.to_string(),
            },
        )
    }

    // ========================================================================
    // Querying groups
    // ========================================================================

    /// Get a group by index.
    pub fn get_group(&self, index: usize) -> Option<&Group> {
        self.groups.get(index)
    }

    /// Get a FlattenedNode by index.
    pub fn get_node(&self, index: usize) -> Option<&FlattenedNode> {
        self.source.iter_nodes().nth(index)
    }

    /// Get all leaf node indices under a group (recursively).
    pub fn collect_node_indices(&self, group_index: usize) -> Vec<usize> {
        let Some(group) = self.groups.get(group_index) else {
            return vec![];
        };

        match &group.kind {
            GroupKind::Leaf { node_index } => vec![*node_index],
            _ => group
                .children
                .iter()
                .flat_map(|&child| self.collect_node_indices(child))
                .collect(),
        }
    }

    /// Get all FlattenedNodes under a group (recursively).
    pub fn collect_nodes(&self, group_index: usize) -> Vec<&FlattenedNode> {
        self.collect_node_indices(group_index)
            .iter()
            .filter_map(|&i| self.source.iter_nodes().nth(i))
            .collect()
    }

    /// Get concatenated text content from a group.
    pub fn get_text_content(&self, group_index: usize) -> String {
        self.collect_nodes(group_index)
            .iter()
            .filter_map(|node| {
                if let FlattenedNodeKind::Text { content, .. } = &node.kind {
                    Some(content.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    // ========================================================================
    // Claimed / Unclaimed
    // ========================================================================

    /// Get set of all group indices that are referenced as children.
    fn referenced_groups(&self) -> HashSet<usize> {
        self.groups
            .iter()
            .flat_map(|g| g.children.iter().copied())
            .collect()
    }

    /// Get all root groups (groups not referenced by any other group).
    pub fn roots(&self) -> Vec<usize> {
        let referenced = self.referenced_groups();
        (0..self.groups.len())
            .filter(|i| !referenced.contains(i))
            .collect()
    }

    /// Get all root Field groups (Field or DateField kinds).
    ///
    /// This is a convenience method that eliminates the repeated pattern:
    /// `doc.roots().iter().filter(|&&idx| doc.is_field(idx)).copied().collect()`
    pub fn root_fields(&self) -> Vec<usize> {
        self.roots()
            .into_iter()
            .filter(|&idx| self.is_field(idx))
            .collect()
    }

    /// Get all root TextBlock groups.
    ///
    /// This is a convenience method that eliminates the repeated pattern:
    /// `doc.roots().iter().filter(|&&idx| doc.is_text_block(idx)).copied().collect()`
    pub fn root_text_blocks(&self) -> Vec<usize> {
        self.roots()
            .into_iter()
            .filter(|&idx| self.is_text_block(idx))
            .collect()
    }

    /// Get all root groups matching a predicate.
    ///
    /// This is a convenience method that eliminates the repeated pattern:
    /// `doc.roots().iter().filter(|&&idx| predicate(doc, idx)).copied().collect()`
    ///
    /// # Example
    /// ```ignore
    /// // Get all root text blocks that are not headings
    /// let text_groups = doc.root_groups_matching(|doc, idx| {
    ///     doc.is_text_block(idx) && !doc.is_heading(idx)
    /// });
    /// ```
    pub fn root_groups_matching<F>(&self, predicate: F) -> Vec<usize>
    where
        F: Fn(&Self, usize) -> bool,
    {
        self.roots()
            .into_iter()
            .filter(|&idx| predicate(self, idx))
            .collect()
    }

    /// Check if a group is claimed (referenced by some other group).
    pub fn is_claimed(&self, group_index: usize) -> bool {
        self.referenced_groups().contains(&group_index)
    }

    /// Get all unclaimed leaf groups (not referenced by any composite group).
    pub fn unclaimed_leaves(&self) -> Vec<usize> {
        let referenced = self.referenced_groups();
        (0..self.groups.len())
            .filter(|&i| {
                matches!(self.groups[i].kind, GroupKind::Leaf { .. }) && !referenced.contains(&i)
            })
            .collect()
    }

    /// Get all unclaimed leaf groups that contain text nodes.
    pub fn unclaimed_text_leaves(&self) -> Vec<usize> {
        self.unclaimed_leaves()
            .into_iter()
            .filter(|&i| {
                if let GroupKind::Leaf { node_index } = self.groups[i].kind {
                    matches!(
                        self.source.iter_nodes().nth(node_index).map(|n| &n.kind),
                        Some(FlattenedNodeKind::Text { .. })
                    )
                } else {
                    false
                }
            })
            .collect()
    }

    /// Get all unclaimed leaf groups that contain field nodes.
    pub fn unclaimed_field_leaves(&self) -> Vec<usize> {
        self.unclaimed_leaves()
            .into_iter()
            .filter(|&i| {
                if let GroupKind::Leaf { node_index } = self.groups[i].kind {
                    matches!(
                        self.source.iter_nodes().nth(node_index).map(|n| &n.kind),
                        Some(FlattenedNodeKind::Field { .. })
                    )
                } else {
                    false
                }
            })
            .collect()
    }

    // ========================================================================
    // Finding groups by kind
    // ========================================================================

    /// Find all groups of a specific kind.
    pub fn find_groups<F>(&self, predicate: F) -> Vec<usize>
    where
        F: Fn(&GroupKind) -> bool,
    {
        self.groups
            .iter()
            .enumerate()
            .filter(|(_, g)| predicate(&g.kind))
            .map(|(i, _)| i)
            .collect()
    }

    /// Find all ExclGroup groups.
    pub fn excl_groups(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::ExclGroup { .. }))
    }

    /// Find all LabeledField groups.
    pub fn labeled_fields(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::LabeledField { .. }))
    }

    /// Find all Heading groups.
    pub fn headings(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::Heading { .. }))
    }

    /// Find all RadioButton groups.
    pub fn radio_buttons(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::RadioButton { .. }))
    }

    /// Find all RadioButtonGroup groups.
    pub fn radio_button_groups(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::RadioButtonGroup))
    }

    /// Find all DateField groups.
    pub fn date_fields(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::DateField { .. }))
    }

    // ========================================================================
    // Group kind checking helpers
    // ========================================================================

    /// Check if a group is a specific kind.
    pub fn is_group_kind(&self, group_idx: usize, predicate: impl Fn(&GroupKind) -> bool) -> bool {
        self.get_group(group_idx)
            .map(|g| predicate(&g.kind))
            .unwrap_or(false)
    }

    /// Check if a group is a TextBlock.
    pub fn is_text_block(&self, group_idx: usize) -> bool {
        self.is_group_kind(group_idx, |k| matches!(k, GroupKind::TextBlock))
    }

    /// Check if a group is a Field or DateField.
    pub fn is_field(&self, group_idx: usize) -> bool {
        self.is_group_kind(group_idx, |k| {
            matches!(k, GroupKind::Field | GroupKind::DateField { .. })
        })
    }

    /// Check if a group is a Heading.
    pub fn is_heading(&self, group_idx: usize) -> bool {
        self.is_group_kind(group_idx, |k| matches!(k, GroupKind::Heading { .. }))
    }

    /// Check if a group (or any of its descendants) contains a field node.
    pub fn contains_field(&self, group_idx: usize) -> bool {
        let nodes = self.collect_nodes(group_idx);
        nodes
            .iter()
            .any(|node| matches!(node.kind, FlattenedNodeKind::Field { .. }))
    }

    /// Check if a group is an InlineField.
    pub fn is_inline_field(&self, group_idx: usize) -> bool {
        self.is_group_kind(group_idx, |k| matches!(k, GroupKind::InlineField))
    }

    /// Find all InlineField groups.
    pub fn inline_fields(&self) -> Vec<usize> {
        self.find_groups(|k| matches!(k, GroupKind::InlineField))
    }

    /// Mark a field group as an inline field by wrapping it in an InlineField group.
    pub fn add_inline_field_marker(&mut self, field_idx: usize) {
        self.merge(
            vec![field_idx],
            GroupKind::InlineField,
            GroupSource::Inferred {
                module: "InlineFieldDetector".to_string(),
            },
        );
    }

    // ========================================================================
    // LabeledField helpers
    // ========================================================================

    /// Get the label group index from a LabeledField group.
    pub fn get_label_group(&self, labeled_field_index: usize) -> Option<usize> {
        let group = self.groups.get(labeled_field_index)?;
        if let GroupKind::LabeledField { label, .. } = &group.kind {
            group.children.get(*label).copied()
        } else {
            None
        }
    }

    /// Get the field group index from a LabeledField group.
    pub fn get_field_group(&self, labeled_field_index: usize) -> Option<usize> {
        let group = self.groups.get(labeled_field_index)?;
        if let GroupKind::LabeledField { field, .. } = &group.kind {
            group.children.get(*field).copied()
        } else {
            None
        }
    }

    /// Get the label text from a LabeledField group.
    pub fn get_label_text(&self, labeled_field_index: usize) -> Option<String> {
        let label_group = self.get_label_group(labeled_field_index)?;
        Some(self.get_text_content(label_group))
    }

    /// Get the field name from a LabeledField group.
    pub fn get_field_name(&self, labeled_field_index: usize) -> Option<String> {
        let field_group = self.get_field_group(labeled_field_index)?;
        let nodes = self.collect_nodes(field_group);
        nodes.first().and_then(|n| {
            if let FlattenedNodeKind::Field { name, .. } = &n.kind {
                Some(name.clone())
            } else {
                None
            }
        })
    }

    // ========================================================================
    // Rendering
    // ========================================================================

    /// Render the document to an image file.
    ///
    /// This renders the underlying Flattened content first, then draws group
    /// overlays on top with blue borders and type annotations.
    ///
    /// Only non-Leaf groups are drawn (composite groups like TextBlock, LabeledField, etc.)
    pub fn render_to_image<P: AsRef<Path>>(
        &self,
        output_path: P,
        scale: f32,
    ) -> Result<(), String> {
        // First render the base Flattened content
        let mut img = self.source.render_to_image_buffer(scale)?;

        // Load fallback font for annotations
        let fallback_font = Flattened::load_fallback_font()?;

        // Colors for group overlays - pure blue (#0000ff) with light bg / dark text
        let group_border = Rgba([0u8, 0u8, 255u8, 255u8]); // Solid blue border
        let group_label_bg = Rgba([200u8, 200u8, 255u8, 255u8]); // Light blue fill for label background
        let group_label_text = Rgba([0u8, 0u8, 255u8, 255u8]); // Blue text

        let scale_dec = num(scale as f64);

        // Get referenced groups to identify outermost groups
        let referenced = self.referenced_groups();

        // Draw overlays only for outermost non-Leaf groups (not referenced by other groups)
        for (group_idx, group) in self.groups.iter().enumerate() {
            // Skip leaf groups - they just wrap single nodes
            if matches!(group.kind, GroupKind::Leaf { .. }) {
                continue;
            }

            // Skip groups that are children of other groups
            if referenced.contains(&group_idx) {
                continue;
            }

            // Calculate bounding box from children
            if let Some((min_x, min_y, max_x, max_y)) = self.compute_group_bounds(group_idx) {
                // Scale coordinates
                let x = (min_x * scale_dec).to_f32().unwrap_or(0.0) as i32;
                let y = (min_y * scale_dec).to_f32().unwrap_or(0.0) as i32;
                let w = ((max_x - min_x) * scale_dec).to_f32().unwrap_or(0.0) as i32;
                let h = ((max_y - min_y) * scale_dec).to_f32().unwrap_or(0.0) as i32;

                if w <= 0 || h <= 0 {
                    continue;
                }

                // Draw group border (2 pixels thick for visibility)
                Self::draw_thick_rect(&mut img, x, y, w, h, group_border, 2);

                // Get group type label
                let label = self.group_type_label(&group.kind);

                // Draw label background
                let label_height = (10.0 * scale) as i32;
                let label_width = (label.len() as f32 * 6.0 * scale) as i32 + 4;
                Flattened::fill_rect(
                    &mut img,
                    x,
                    y - label_height - 2,
                    label_width,
                    label_height + 2,
                    group_label_bg,
                );

                // Draw label text
                let font_size = (9.0 * scale).max(8.0);
                let text_scale = PxScale::from(font_size);
                draw_text_mut(
                    &mut img,
                    group_label_text,
                    x + 2,
                    y - label_height,
                    text_scale,
                    &fallback_font,
                    &label,
                );
            }
        }

        // Save the image
        img.save(output_path.as_ref())
            .map_err(|e| format!("Failed to save image: {}", e))?;

        Ok(())
    }

    /// Compute the bounding box for a group from its children.
    /// Returns (min_x, min_y, max_x, max_y) in document coordinates (not scaled).
    pub fn compute_group_bounds(
        &self,
        group_idx: usize,
    ) -> Option<(
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
    )> {
        let node_indices = self.collect_node_indices(group_idx);
        if node_indices.is_empty() {
            return None;
        }

        let mut min_x = rust_decimal::Decimal::MAX;
        let mut min_y = rust_decimal::Decimal::MAX;
        let mut max_x = rust_decimal::Decimal::MIN;
        let mut max_y = rust_decimal::Decimal::MIN;

        for node_idx in node_indices {
            if let Some(node) = self.source.iter_nodes().nth(node_idx) {
                min_x = min_x.min(node.x);
                min_y = min_y.min(node.y);
                max_x = max_x.max(node.x + node.width);
                max_y = max_y.max(node.y + node.height);
            }
        }

        if min_x == rust_decimal::Decimal::MAX {
            return None;
        }

        Some((min_x, min_y, max_x, max_y))
    }

    /// Get bounding box for a group as a Bounds struct.
    pub fn get_bounds(&self, group_idx: usize) -> Option<Bounds> {
        let (min_x, min_y, max_x, max_y) = self.compute_group_bounds(group_idx)?;
        Some(Bounds::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }

    /// Get a human-readable label for a group kind.
    fn group_type_label(&self, kind: &GroupKind) -> String {
        match kind {
            GroupKind::Leaf { .. } => "Leaf".to_string(),
            GroupKind::Unknown => "Unknown".to_string(),
            GroupKind::TextBlock => "TextBlock".to_string(),
            GroupKind::LabeledField { .. } => "LabeledField".to_string(),
            GroupKind::RadioButton { .. } => "RadioButton".to_string(),
            GroupKind::RadioButtonGroup => "RadioButtonGroup".to_string(),
            GroupKind::DateField { num_fields } => format!("DateField[{}]", num_fields),
            GroupKind::ExclGroup { selected_value } => {
                if let Some(val) = selected_value {
                    format!("ExclGroup[{}]", val)
                } else {
                    "ExclGroup".to_string()
                }
            }
            GroupKind::Heading { level } => format!("H{}", level),
            GroupKind::Paragraph => "Paragraph".to_string(),
            GroupKind::Field => "Field".to_string(),
            GroupKind::Section => "Section".to_string(),
            GroupKind::Header => "Header".to_string(),
            GroupKind::Footer => "Footer".to_string(),
            GroupKind::RepeatableSection {
                min_occurrences,
                max_occurrences,
            } => match max_occurrences {
                Some(max) => format!("RepeatableSection[{}-{}]", min_occurrences, max),
                None => format!("RepeatableSection[{}+]", min_occurrences),
            },
            GroupKind::InlineField => "InlineField".to_string(),
            GroupKind::NoPrint => "NoPrint".to_string(),
            GroupKind::GridLayout { columns, .. } => format!("GridLayout[{}cols]", columns),
        }
    }

    /// Draw a thick rectangular border by drawing multiple single-pixel rectangles.
    fn draw_thick_rect(
        img: &mut RgbaImage,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: Rgba<u8>,
        thickness: i32,
    ) {
        for i in 0..thickness {
            Flattened::draw_transparent_rect(img, x - i, y - i, w + 2 * i, h + 2 * i, color);
        }
    }
}

impl Group {
    /// Check if this is a leaf group.
    pub fn is_leaf(&self) -> bool {
        matches!(self.kind, GroupKind::Leaf { .. })
    }

    /// Get the node index if this is a leaf group.
    pub fn node_index(&self) -> Option<usize> {
        if let GroupKind::Leaf { node_index } = &self.kind {
            Some(*node_index)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    fn create_test_flattened() -> Flattened {
        Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "First".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(10.0),
                    num(30.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(42.0),
                    num(10.0),
                    num(35.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_FirstName".to_string(),
                    "".to_string(),
                    "First Name".to_string(),
                    num(80.0),
                    num(10.0),
                    num(150.0),
                    num(20.0),
                ),
                FlattenedNode::new_field(
                    "Radio_Yes".to_string(),
                    "1".to_string(),
                    "Yes".to_string(),
                    num(10.0),
                    num(50.0),
                    num(20.0),
                    num(20.0),
                ),
                FlattenedNode::new_field(
                    "Radio_No".to_string(),
                    "0".to_string(),
                    "No".to_string(),
                    num(40.0),
                    num(50.0),
                    num(20.0),
                    num(20.0),
                ),
            ],
        )
    }

    #[test]
    fn test_document_initialization() {
        let flattened = create_test_flattened();
        let doc = Document::from_flattened(&flattened);

        // Should have one leaf group per node
        assert_eq!(doc.groups.len(), 5);

        // All should be leaf groups
        for (i, group) in doc.groups.iter().enumerate() {
            assert!(group.is_leaf());
            assert_eq!(group.node_index(), Some(i));
        }

        // All leaves should be unclaimed initially
        assert_eq!(doc.unclaimed_leaves().len(), 5);

        // All should be roots initially
        assert_eq!(doc.roots().len(), 5);
    }

    #[test]
    fn test_merge_text_block() {
        let flattened = create_test_flattened();
        let mut doc = Document::from_flattened(&flattened);

        // Merge "First" and "Name:" into a TextBlock
        let text_block = doc.create_text_block(vec![0, 1], "TextMerger");

        assert_eq!(text_block, 5); // New group at index 5
        assert_eq!(doc.groups.len(), 6);

        // Text content should be concatenated
        assert_eq!(doc.get_text_content(text_block), "First Name:");

        // Leaves 0 and 1 are now claimed
        assert!(doc.is_claimed(0));
        assert!(doc.is_claimed(1));
        assert!(!doc.is_claimed(2));

        // Unclaimed leaves should be 3 (2, 3, 4)
        assert_eq!(doc.unclaimed_leaves().len(), 3);
    }

    #[test]
    fn test_create_labeled_field() {
        let flattened = create_test_flattened();
        let mut doc = Document::from_flattened(&flattened);

        // First merge text into TextBlock
        let text_block = doc.create_text_block(vec![0, 1], "TextMerger");

        // Then create LabeledField
        let labeled_field = doc.create_labeled_field(text_block, 2, "LabelAttacher");

        assert_eq!(labeled_field, 6);

        // Get label and field
        assert_eq!(doc.get_label_group(labeled_field), Some(text_block));
        assert_eq!(doc.get_field_group(labeled_field), Some(2));
        assert_eq!(
            doc.get_label_text(labeled_field),
            Some("First Name:".to_string())
        );
        assert_eq!(
            doc.get_field_name(labeled_field),
            Some("TF_FirstName".to_string())
        );

        // Now text_block is also claimed
        assert!(doc.is_claimed(text_block));

        // Root should be the LabeledField and the two radio buttons
        let roots = doc.roots();
        assert_eq!(roots.len(), 3);
        assert!(roots.contains(&labeled_field));
        assert!(roots.contains(&3)); // Radio_Yes
        assert!(roots.contains(&4)); // Radio_No
    }

    #[test]
    fn test_create_excl_group() {
        let flattened = create_test_flattened();
        let mut doc = Document::from_flattened(&flattened);

        // Create ExclGroup from radio buttons
        let excl_group = doc.create_excl_group(vec![3, 4], Some("1".to_string()), GroupSource::Xfa);

        assert_eq!(excl_group, 5);

        // Radio leaves are now claimed
        assert!(doc.is_claimed(3));
        assert!(doc.is_claimed(4));

        // Unclaimed should be text and field leaves
        let unclaimed = doc.unclaimed_leaves();
        assert_eq!(unclaimed.len(), 3);
        assert!(unclaimed.contains(&0));
        assert!(unclaimed.contains(&1));
        assert!(unclaimed.contains(&2));
    }

    #[test]
    fn test_collect_node_indices() {
        let flattened = create_test_flattened();
        let mut doc = Document::from_flattened(&flattened);

        // Build hierarchy: TextBlock -> LabeledField
        let text_block = doc.create_text_block(vec![0, 1], "TextMerger");
        let labeled_field = doc.create_labeled_field(text_block, 2, "LabelAttacher");

        // Collect nodes from LabeledField should get all 3 nodes
        let nodes = doc.collect_node_indices(labeled_field);
        assert_eq!(nodes.len(), 3);
        assert!(nodes.contains(&0));
        assert!(nodes.contains(&1));
        assert!(nodes.contains(&2));
    }

    #[test]
    fn test_find_groups_by_kind() {
        let flattened = create_test_flattened();
        let mut doc = Document::from_flattened(&flattened);

        // Create various groups
        let _text_block = doc.create_text_block(vec![0, 1], "TextMerger");
        let _excl_group = doc.create_excl_group(vec![3, 4], None, GroupSource::Xfa);

        // Find by kind
        assert_eq!(doc.excl_groups().len(), 1);
        assert_eq!(
            doc.find_groups(|k| matches!(k, GroupKind::TextBlock)).len(),
            1
        );
        assert_eq!(
            doc.find_groups(|k| matches!(k, GroupKind::Leaf { .. }))
                .len(),
            5
        );
    }

    #[test]
    fn test_compute_group_bounds() {
        let flattened = create_test_flattened();
        let mut doc = Document::from_flattened(&flattened);

        // Create a TextBlock from first two text nodes
        let text_block = doc.create_text_block(vec![0, 1], "TextMerger");

        // Get bounds
        let bounds = doc.compute_group_bounds(text_block);
        assert!(bounds.is_some());

        let (min_x, min_y, max_x, max_y) = bounds.unwrap();
        // First node: x=10, y=10, w=30, h=12
        // Second node: x=42, y=10, w=35, h=12
        // Expected bounds: min_x=10, min_y=10, max_x=77, max_y=22
        assert_eq!(min_x, num(10.0));
        assert_eq!(min_y, num(10.0));
        assert_eq!(max_x, num(77.0)); // 42 + 35
        assert_eq!(max_y, num(22.0)); // 10 + 12
    }

    #[test]
    fn test_group_type_labels() {
        let flattened = create_test_flattened();
        let doc = Document::from_flattened(&flattened);

        assert_eq!(doc.group_type_label(&GroupKind::TextBlock), "TextBlock");
        assert_eq!(
            doc.group_type_label(&GroupKind::LabeledField { label: 0, field: 1 }),
            "LabeledField"
        );
        assert_eq!(
            doc.group_type_label(&GroupKind::ExclGroup {
                selected_value: Some("Yes".to_string())
            }),
            "ExclGroup[Yes]"
        );
        assert_eq!(doc.group_type_label(&GroupKind::Heading { level: 2 }), "H2");
    }
}
