use crate::xfa::{XfaNode, XfaNodeKind, Border, Font, Para, HAlign, VAlign, StrokeStyle, Num, num};
use crate::scripting::{XfaScriptEngine, parse_events_from_node, ScriptContentType, EventActivity, EventRef, Presence};
use crate::font_manager::get_font_manager;
use std::path::Path;
use std::collections::HashMap;
use image::{RgbaImage, Rgba, ImageBuffer};
use imageproc::drawing::draw_text_mut;
use ab_glyph::{FontRef, PxScale, Font as AbGlyphFont, ScaleFont};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::str::FromStr;

pub struct Flattened {
    pub page: Page,
    pub nodes: Vec<FlattenedNode>,
}

pub struct Page {
    pub width: Num,
    pub height: Num,
}

/// Rendering style information
#[derive(Debug, Clone, Default)]
pub struct RenderStyle {
    /// Border properties
    pub border: Option<Border>,
    /// Font properties
    pub font: Option<Font>,
    /// Paragraph properties  
    pub para: Option<Para>,
}

/// Main flattened node structure containing position and rendering information
#[derive(Debug, Clone)]
pub struct FlattenedNode {
    /// Node-specific information
    pub kind: FlattenedNodeKind,
    
    /// Position and dimensions
    pub x: Num,
    pub y: Num,
    pub width: Num,
    pub height: Num,
    
    /// Rotation in degrees (counter-clockwise, multiples of 90)
    pub rotate: i32,
    
    /// Rendering style
    pub style: RenderStyle,
}

/// Enum representing the specific kind of flattened node
#[derive(Debug, Clone)]
pub enum FlattenedNodeKind {
    /// Text/draw element
    Text {
        content: String,
        font_size: Num,
        font_name: String,
        /// Name of the source XFA node (for Draw elements with scripts)
        source_name: Option<String>,
        /// Optional rich text structure for HTML content (exData with contentType="text/html")
        /// When present, this should be used for rendering instead of `content` to preserve
        /// paragraph structure, text-indent, and xfa-spacerun spacing.
        rich_text: Option<RichText>,
    },
    
    /// Input field
    Field {
        name: String,
        value: String,
        label: String,
    },
}

// ============================================================================
// XFA-Compliant Rich Text Model
// ============================================================================

/// A rich text document consisting of multiple paragraphs.
/// Per XFA spec, rich text in exData contentType="text/html" is structured as
/// XHTML paragraphs with inline styling.
#[derive(Debug, Clone, Default)]
pub struct RichText {
    /// Paragraphs in the document
    pub paragraphs: Vec<RichParagraph>,
}

/// A single paragraph with optional styling and text runs.
/// Per XFA spec (Chapter 27): paragraphs can have text-indent, margins, alignment.
#[derive(Debug, Clone, Default)]
pub struct RichParagraph {
    /// Text runs within the paragraph
    pub runs: Vec<RichRun>,
    /// First-line text indent (from CSS text-indent style)
    pub text_indent: Option<f32>,
    /// Horizontal alignment
    pub h_align: HAlign,
    /// Space above paragraph
    pub space_above: Option<f32>,
    /// Space below paragraph
    pub space_below: Option<f32>,
    /// Whether this is an empty paragraph (just whitespace/separator)
    pub is_empty: bool,
}

/// A run of text with uniform styling.
/// Per XFA spec: spans can have xfa-spacerun:yes to preserve consecutive spaces.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct RichRun {
    /// The text content
    pub text: String,
    /// Whether consecutive spaces should be preserved (xfa-spacerun:yes)
    pub preserve_spaces: bool,
    /// Font weight (bold)
    pub bold: bool,
    /// Font style (italic)
    pub italic: bool,
    /// Underline
    pub underline: bool,
}


/// A positioned word/token ready for rendering.
/// Used for glyph-by-glyph rendering with proper justify support.
#[derive(Debug, Clone)]
pub struct RenderedWord {
    /// The text of this word
    pub text: String,
    /// X position in pixels
    pub x: f32,
    /// Whether spaces should be preserved (from xfa-spacerun)
    pub preserve_spaces: bool,
    /// Whether this word should be rendered bold
    pub bold: bool,
    /// Whether this word should be rendered italic
    pub italic: bool,
}

/// A line of text ready for rendering, with positioning info.
#[derive(Debug, Clone)]
pub struct RenderedLine {
    /// Words/tokens in this line
    pub words: Vec<RenderedWord>,
    /// Y position of the line baseline
    pub y: f32,
    /// Whether this is the first line of a paragraph (for text-indent)
    pub is_first_line: bool,
    /// Whether this is the last line of a paragraph (for justify - don't stretch)
    pub is_last_line: bool,
    /// The paragraph's text indent (only applied if is_first_line)
    pub text_indent: f32,
    /// Horizontal alignment for this line
    pub h_align: HAlign,
    /// Total width of all content on this line
    pub content_width: f32,
}

/// A token for text layout - a word or preserved space run.
/// Used internally during text layout.
#[derive(Debug, Clone)]
pub struct LayoutToken {
    pub text: String,
    pub width: f32,
    pub preserve_spaces: bool,
    /// Whether this token should be rendered bold
    pub bold: bool,
    /// Whether this token should be rendered italic
    pub italic: bool,
}

impl FlattenedNode {
    /// Create a new text node
    pub fn new_text(content: String, font_size: Num, font_name: String, x: Num, y: Num, width: Num, height: Num) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name: None, rich_text: None },
            x,
            y,
            width,
            height,
            rotate: 0,
            style: RenderStyle::default(),
        }
    }
    
    /// Create a new text node with style
    pub fn new_text_styled(content: String, font_size: Num, font_name: String, x: Num, y: Num, width: Num, height: Num, style: RenderStyle) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name: None, rich_text: None },
            x,
            y,
            width,
            height,
            rotate: 0,
            style,
        }
    }
    
    /// Create a new field node
    pub fn new_field(name: String, value: String, label: String, x: Num, y: Num, width: Num, height: Num) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Field { name, value, label },
            x,
            y,
            width,
            height,
            rotate: 0,
            style: RenderStyle::default(),
        }
    }
    
    /// Create a new field node with style
    pub fn new_field_styled(name: String, value: String, label: String, x: Num, y: Num, width: Num, height: Num, style: RenderStyle) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Field { name, value, label },
            x,
            y,
            width,
            height,
            rotate: 0,
            style,
        }
    }
    
    /// Create a new field node with style and rotation
    pub fn new_field_styled_rotated(name: String, value: String, label: String, x: Num, y: Num, width: Num, height: Num, style: RenderStyle, rotate: i32) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Field { name, value, label },
            x,
            y,
            width,
            height,
            rotate,
            style,
        }
    }
    
    /// Create a new text node with style and rotation
    pub fn new_text_styled_rotated(content: String, font_size: Num, font_name: String, x: Num, y: Num, width: Num, height: Num, style: RenderStyle, rotate: i32) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name: None, rich_text: None },
            x,
            y,
            width,
            height,
            rotate,
            style,
        }
    }
    
    /// Create a new text node with style, rotation, and source name (for Draw elements with scripts)
    pub fn new_text_styled_rotated_named(content: String, font_size: Num, font_name: String, x: Num, y: Num, width: Num, height: Num, style: RenderStyle, rotate: i32, source_name: Option<String>) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name, rich_text: None },
            x,
            y,
            width,
            height,
            rotate,
            style,
        }
    }
    
    /// Create a new text node with rich text content (for HTML exData)
    pub fn new_text_with_rich_text(content: String, font_size: Num, font_name: String, x: Num, y: Num, width: Num, height: Num, style: RenderStyle, rotate: i32, source_name: Option<String>, rich_text: Option<RichText>) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name, rich_text },
            x,
            y,
            width,
            height,
            rotate,
            style,
        }
    }
    
    /// Get the bounds of this node.
    pub fn bounds(&self) -> Bounds {
        Bounds::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Position {
    pub x: Num,
    pub y: Num,
    pub width: Num,
    pub height: Num,
}

impl Position {
    pub fn new(x: Num, y: Num, width: Num, height: Num) -> Self {
        Position { x, y, width, height }
    }
    
    pub fn zero() -> Self {
        Position { x: Decimal::ZERO, y: Decimal::ZERO, width: Decimal::ZERO, height: Decimal::ZERO }
    }
}

/// Bounding box with geometry helper methods.
/// 
/// Provides convenient methods for spatial relationship calculations
/// commonly used in document analysis modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x: Num,
    pub y: Num,
    pub width: Num,
    pub height: Num,
}

impl Bounds {
    /// Create a new Bounds from position and dimensions.
    pub fn new(x: Num, y: Num, width: Num, height: Num) -> Self {
        Bounds { x, y, width, height }
    }
    
    /// Create bounds from a tuple (x, y, width, height).
    pub fn from_tuple(tuple: (Num, Num, Num, Num)) -> Self {
        Bounds { x: tuple.0, y: tuple.1, width: tuple.2, height: tuple.3 }
    }
    
    /// Convert to tuple (x, y, width, height).
    pub fn to_tuple(self) -> (Num, Num, Num, Num) {
        (self.x, self.y, self.width, self.height)
    }
    
    // ========================================================================
    // Edge accessors
    // ========================================================================
    
    /// Right edge (x + width).
    #[inline]
    pub fn right(&self) -> Num {
        self.x + self.width
    }
    
    /// Bottom edge (y + height).
    #[inline]
    pub fn bottom(&self) -> Num {
        self.y + self.height
    }
    
    /// Left edge (alias for x).
    #[inline]
    pub fn left(&self) -> Num {
        self.x
    }
    
    /// Top edge (alias for y).
    #[inline]
    pub fn top(&self) -> Num {
        self.y
    }
    
    // ========================================================================
    // Center calculations
    // ========================================================================
    
    /// Horizontal center (x + width / 2).
    #[inline]
    pub fn center_x(&self) -> Num {
        self.x + self.width / Decimal::TWO
    }
    
    /// Vertical center (y + height / 2).
    #[inline]
    pub fn center_y(&self) -> Num {
        self.y + self.height / Decimal::TWO
    }
    
    // ========================================================================
    // Distance calculations
    // ========================================================================
    
    /// Horizontal gap from this bounds' right edge to another bounds' left edge.
    /// Returns None if other is not to the right (overlapping or reversed).
    pub fn horizontal_gap_to(&self, other: &Bounds) -> Option<Num> {
        if other.x >= self.right() {
            Some(other.x - self.right())
        } else {
            None
        }
    }
    
    /// Vertical gap from this bounds' bottom edge to another bounds' top edge.
    /// Returns None if other is not below (overlapping or reversed).
    pub fn vertical_gap_to(&self, other: &Bounds) -> Option<Num> {
        if other.y >= self.bottom() {
            Some(other.y - self.bottom())
        } else {
            None
        }
    }
    
    /// Absolute vertical distance between center points.
    pub fn vertical_center_distance(&self, other: &Bounds) -> Num {
        (self.center_y() - other.center_y()).abs()
    }
    
    /// Absolute horizontal distance between center points.
    pub fn horizontal_center_distance(&self, other: &Bounds) -> Num {
        (self.center_x() - other.center_x()).abs()
    }
    
    // ========================================================================
    // Alignment checks
    // ========================================================================
    
    /// Check if horizontally aligned (centers within tolerance).
    pub fn is_horizontally_aligned(&self, other: &Bounds, tolerance: Num) -> bool {
        self.vertical_center_distance(other) <= tolerance
    }
    
    /// Check if vertically aligned (centers within tolerance).
    pub fn is_vertically_aligned(&self, other: &Bounds, tolerance: Num) -> bool {
        self.horizontal_center_distance(other) <= tolerance
    }
    
    /// Check if on the same line (vertical centers within tolerance based on max height).
    pub fn is_on_same_line(&self, other: &Bounds, tolerance: Num) -> bool {
        let max_half_height = self.height.max(other.height) / Decimal::TWO;
        self.vertical_center_distance(other) <= max_half_height + tolerance
    }
    
    // ========================================================================
    // Overlap checks
    // ========================================================================
    
    /// Check if this bounds overlaps horizontally with another (within tolerance).
    pub fn overlaps_horizontally(&self, other: &Bounds, tolerance: Num) -> bool {
        !(self.right() < other.x - tolerance || self.x > other.right() + tolerance)
    }
    
    /// Check if this bounds overlaps vertically with another (within tolerance).
    pub fn overlaps_vertically(&self, other: &Bounds, tolerance: Num) -> bool {
        !(self.bottom() < other.y - tolerance || self.y > other.bottom() + tolerance)
    }
    
    /// Check if this bounds overlaps with another in both dimensions.
    pub fn overlaps(&self, other: &Bounds) -> bool {
        self.overlaps_horizontally(other, Decimal::ZERO) && 
        self.overlaps_vertically(other, Decimal::ZERO)
    }
    
    // ========================================================================
    // Relative position checks
    // ========================================================================
    
    /// Check if other is above this bounds (other's bottom <= this top).
    pub fn is_above(&self, other: &Bounds) -> bool {
        other.bottom() <= self.y
    }
    
    /// Check if other is below this bounds (other's top >= this bottom).
    pub fn is_below(&self, other: &Bounds) -> bool {
        other.y >= self.bottom()
    }
    
    /// Check if other is to the left of this bounds (other's right <= this left).
    pub fn is_left_of(&self, other: &Bounds) -> bool {
        other.right() <= self.x
    }
    
    /// Check if other is to the right of this bounds (other's left >= this right).
    pub fn is_right_of(&self, other: &Bounds) -> bool {
        other.x >= self.right()
    }
    
    // ========================================================================
    // Bounding box operations
    // ========================================================================
    
    /// Compute union of this bounds with another.
    pub fn union(&self, other: &Bounds) -> Bounds {
        let min_x = self.x.min(other.x);
        let min_y = self.y.min(other.y);
        let max_x = self.right().max(other.right());
        let max_y = self.bottom().max(other.bottom());
        Bounds::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layout {
    TopToBottom,            // tb
    LeftToRightTopToBottom, // lr-tb
    RightToLeftTopToBottom, // rl-tb
    LeftToRight,            // lr (alias for lr-tb in many cases)
    Row,                    // row
    RightToLeftRow,         // rl-row
    Position,               // position (absolute positioning)
    Table,                  // table
}

/// Context for flattening XFA nodes into absolute positions.
/// 
/// Bundles all state needed during the recursive flattening process:
/// - Embed resolution data (computed_values, id_to_field) for xfa:embed references
/// - Inherited presence from parent containers (inherited_presence)
/// 
/// Per XFA 3.3 spec (page 221, "Rich Text That Contains External Objects"):
/// External references via xfa:embed are resolved during the layout process.
/// 
/// Per XFA 3.3 spec (section 2, "Explicitly Concealing Containers"):
/// Children inherit presence from their parent container - if a parent is hidden,
/// all its children are also hidden regardless of their individual presence values.
pub struct FlattenContext<'a> {
    /// Map of field name/ID -> computed value from scripts
    pub computed_values: &'a HashMap<String, String>,
    /// Map of element ID -> field name for resolving embed URI references
    pub id_to_field: &'a HashMap<String, String>,
    /// Inherited presence from parent - if Hidden or Inactive, children are also hidden
    pub inherited_presence: Option<Presence>,
}

impl<'a> FlattenContext<'a> {
    /// Create a new flatten context with the given embed resolution data
    pub fn new(
        computed_values: &'a HashMap<String, String>, 
        id_to_field: &'a HashMap<String, String>,
    ) -> Self {
        FlattenContext { 
            computed_values, 
            id_to_field, 
            inherited_presence: None,
        }
    }
    
    /// Create an empty context (no embed resolution)
    pub fn empty() -> FlattenContext<'static> {
        static EMPTY_STR: std::sync::LazyLock<HashMap<String, String>> = std::sync::LazyLock::new(HashMap::new);
        FlattenContext {
            computed_values: &EMPTY_STR,
            id_to_field: &EMPTY_STR,
            inherited_presence: None,
        }
    }
    
    /// Create a child context with inherited presence
    /// Used when recursing into subforms that may have presence set
    pub fn with_inherited_presence(&self, presence: Presence) -> FlattenContext<'a> {
        FlattenContext {
            computed_values: self.computed_values,
            id_to_field: self.id_to_field,
            inherited_presence: Some(presence),
        }
    }
    
    /// Get the effective presence for a node, considering:
    /// 1. Inherited presence from parent (takes precedence if hidden/inactive)
    /// 2. Presence stored directly on the XfaNode (set by scripts or from attributes)
    pub fn get_effective_presence(&self, node: &XfaNode) -> Presence {
        // If parent is hidden/inactive, children inherit that
        if let Some(inherited) = self.inherited_presence
            && inherited.should_skip_layout() {
                return inherited;
            }
        
        // Read presence directly from the XFA node (scripts modify this directly)
        node.get_presence()
    }
    
    /// Extract text content from node children, resolving any xfa:embed references
    pub fn extract_text(&self, children: &[XfaNode]) -> Option<String> {
        Flattened::extract_text_content_with_embed(children, self.computed_values, self.id_to_field)
    }
}

impl Layout {
    /// Parse layout attribute string to Layout enum
    /// Per XFA spec: if subform has no layout attribute, it defaults to "position"
    pub fn from_str(s: &str) -> Self {
        match s {
            "tb" => Layout::TopToBottom,
            "lr-tb" => Layout::LeftToRightTopToBottom,
            "rl-tb" => Layout::RightToLeftTopToBottom,
            "lr" => Layout::LeftToRight,
            "row" => Layout::Row,
            "rl-row" => Layout::RightToLeftRow,
            "position" => Layout::Position,
            "table" => Layout::Table,
            // Per XFA spec section 8 (page 280): "If a subform element does not have
            // a layout attribute it defaults to positioned layout."
            _ => Layout::Position,
        }
    }
    
    /// Returns true if this layout mode is a flowing layout (ignores x/y coordinates)
    /// Per XFA spec: "In flowing layout the contained object's x and y properties,
    /// as well as its anchor point, are ignored."
    pub fn is_flowing(&self) -> bool {
        match self {
            Layout::Position => false,
            _ => true,
        }
    }
}

impl Flattened {
    /// Create a flattened representation from XFA nodes with computed absolute positions.
    /// 
    /// This implements the XFA Layout process per the spec (section 3, "Template DOM, Form DOM, and Layout DOM"):
    /// 
    /// 1. **Template DOM** supplies:
    ///    - Page structure: pageSet → pageArea → contentArea
    ///    - Page background: direct children of pageArea (excluding contentArea/medium)
    /// 
    /// 2. **Form DOM** (derived from Template DOM):
    ///    - A duplicate of the subtree under the root subform (NOT including pageSet)
    ///    - Contains the actual form content (fields, draws, subforms with data)
    /// 
    /// 3. **Layout DOM** (what we're building here):
    ///    - Page structure from Template DOM
    ///    - Page background from Template DOM (rendered per page instance)
    ///    - Form content placed INTO the contentArea
    /// 
    /// Structure example:
    /// ```text
    /// template
    ///   subform 'UBSForms' (root container)
    ///     pageSet 'MPs'           <-- stays in Template DOM
    ///       pageArea 'Page1'
    ///         draw (header)       <-- page background, rendered at page origin
    ///         contentArea 'Body'  <-- defines where form content goes
    ///     subform 'Page'          <-- Form DOM root, content goes INTO contentArea
    /// ```
    pub fn from_xfa(xfa_nodes: &[XfaNode]) -> Result<Self, String> {
        // Use the version without scripts (empty computed values, no embed context)
        Self::from_xfa_with_computed_values(xfa_nodes, &HashMap::new(), &HashMap::new())
    }
    
    /// Create a flattened representation from XFA nodes with script execution.
    /// 
    /// This method:
    /// 1. Extracts translation objects from <variables> sections
    /// 2. Executes all form-ready scripts to compute field values and presence
    /// 3. Builds an ID-to-field-name map for resolving xfa:embed references
    /// 4. Uses those computed values during flattening
    /// 
    /// Parameters:
    /// - `xfa_nodes`: The parsed XFA template nodes (mutable - scripts modify presence)
    /// - `language`: The language code (e.g., "DE", "EN", "SP") for translations
    /// - `form_id`: The form ID (e.g., "AAAB_019_DE") used by some scripts
    pub fn from_xfa_with_scripts(xfa_nodes: &mut [XfaNode], language: &str, form_id: &str) -> Result<Self, String> {
        // Execute scripts - modifies presence directly on XFA nodes, returns computed values
        let computed_values = Self::execute_form_ready_scripts(xfa_nodes, language, form_id)?;
        
        // Build ID-to-field-name map for xfa:embed resolution
        let id_to_field = Self::build_id_to_field_map(xfa_nodes);
        
        // Flatten with computed values and ID map (presence is now on nodes)
        Self::from_xfa_with_computed_values(xfa_nodes, &computed_values, &id_to_field)
    }
    
    /// Build a map from element ID to field name (for resolving xfa:embed references)
    fn build_id_to_field_map(xfa_nodes: &[XfaNode]) -> HashMap<String, String> {
        let mut id_map = HashMap::new();
        Self::collect_ids_recursive(xfa_nodes, &mut id_map);
        id_map
    }
    
    /// Recursively collect ID attributes and map them to field names
    fn collect_ids_recursive(nodes: &[XfaNode], id_map: &mut HashMap<String, String>) {
        for node in nodes {
            // Check if this node has both an id and a name
            if let (Some(id), Some(name)) = (node.attributes.get("id"), &node.name) {
                id_map.insert(id.clone(), name.clone());
            }
            // Recurse into children
            Self::collect_ids_recursive(&node.children, id_map);
        }
    }
    
    /// Build a map from node name to its child field names
    /// Used for setting up `this.childField` access in scripts
    fn build_parent_child_map(xfa_nodes: &[XfaNode]) -> HashMap<String, Vec<String>> {
        let mut parent_child_map: HashMap<String, Vec<String>> = HashMap::new();
        
        fn collect_children(nodes: &[XfaNode], parent_name: Option<&str>, map: &mut HashMap<String, Vec<String>>) {
            for node in nodes {
                let node_name = node.name.as_deref();
                
                // If this is a field and we have a parent, add to parent's children
                if let Some(parent) = parent_name {
                    let is_field = matches!(node.kind, XfaNodeKind::Field) ||
                        matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                    
                    if is_field
                        && let Some(name) = node_name {
                            map.entry(parent.to_string())
                                .or_default()
                                .push(name.to_string());
                        }
                }
                
                // For subforms, recurse with this node as the parent
                let is_subform = matches!(node.kind, XfaNodeKind::Subform) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                
                if is_subform {
                    collect_children(&node.children, node_name, map);
                } else {
                    // Non-subform containers pass through the current parent
                    collect_children(&node.children, parent_name, map);
                }
            }
        }
        
        collect_children(xfa_nodes, None, &mut parent_child_map);
        parent_child_map
    }
    
    /// Build a parent-child map that tracks both child names AND their unique IDs.
    /// Key: unique path like "Signature[0]" or "Signature[1]" for multiple instances
    /// Value: Vec of (child_name, child_id) pairs
    /// This is needed because multiple subforms can have same-named children with different IDs.
    fn build_parent_child_map_with_ids(xfa_nodes: &[XfaNode]) -> HashMap<String, Vec<(String, String)>> {
        let mut parent_child_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        // Track how many times we've seen each subform name to create unique keys
        let mut subform_counters: HashMap<String, usize> = HashMap::new();
        
        fn collect_children_with_ids(
            nodes: &[XfaNode], 
            parent_key: Option<&str>, 
            map: &mut HashMap<String, Vec<(String, String)>>,
            counters: &mut HashMap<String, usize>
        ) {
            for node in nodes {
                let node_name = node.name.clone().unwrap_or_default();
                let node_id = node.attributes.get("id").cloned().unwrap_or_default();
                
                // If this is a field and we have a parent, add to parent's children
                if let Some(parent) = parent_key {
                    let is_field = matches!(node.kind, XfaNodeKind::Field) ||
                        matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                    
                    if is_field && !node_name.is_empty() {
                        map.entry(parent.to_string())
                            .or_default()
                            .push((node_name.clone(), node_id.clone()));
                    }
                }
                
                // For subforms and exclGroups, recurse with this node as the parent
                let is_subform = matches!(node.kind, XfaNodeKind::Subform) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");
                
                if (is_subform || is_exclgroup) && !node_name.is_empty() {
                    // Create a key that uniquely identifies this subform/exclGroup instance
                    // Use ID if available, otherwise use instance counter
                    // For exclGroups, we want to track their field children (RB_1, RB_2, etc.)
                    let key = if !node_id.is_empty() { 
                        format!("{}#{}", node_name, node_id) 
                    } else {
                        // Use instance counter for subforms/exclGroups without IDs
                        let count = counters.entry(node_name.clone()).or_insert(0);
                        let key = format!("{}[{}]", node_name, *count);
                        *count += 1;
                        key
                    };
                    collect_children_with_ids(&node.children, Some(&key), map, counters);
                } else if !is_subform && !is_exclgroup {
                    // Non-subform/non-exclgroup containers pass through the current parent
                    collect_children_with_ids(&node.children, parent_key, map, counters);
                }
            }
        }
        
        collect_children_with_ids(xfa_nodes, None, &mut parent_child_map, &mut subform_counters);
        parent_child_map
    }

    /// Build and register the XFA SOM hierarchy in the scripting engine.
    /// 
    /// Per XFA 3.3 spec Chapter 3 ("Scripting Object Model"):
    /// - Subforms and fields form a hierarchy accessible via dot notation
    /// - Top-level subforms are accessible as global variables (e.g., "Page")
    /// - Child elements are accessible as properties (e.g., "Page.FormTitle.Field.rawValue")
    /// 
    /// This enables scripts to use references like:
    /// `if(Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.rawValue == 3) { ... }`
    fn build_and_register_xfa_som_hierarchy(xfa_nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        /// Recursively register all subforms and fields
        fn register_nodes_recursive(
            nodes: &[XfaNode], 
            parent_path: Option<&str>,
            engine: &mut XfaScriptEngine
        ) {
            for node in nodes {
                let name = match &node.name {
                    Some(n) if !n.is_empty() => n.clone(),
                    _ => continue, // Skip unnamed nodes
                };
                
                // Build the SOM path for this node
                let path = match parent_path {
                    Some(parent) => format!("{}.{}", parent, name),
                    None => name.clone(),
                };
                
                // Determine node type and value
                let is_field = matches!(node.kind, XfaNodeKind::Field) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                let is_subform = matches!(node.kind, XfaNodeKind::Subform) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");
                
                if is_field {
                    // Register field
                    let value = extract_field_value_helper(&node.children);
                    engine.register_xfa_node(&name, &path, parent_path, true, &value);
                } else if is_exclgroup {
                    // Per XFA spec (section 2 "Exclusion Group"):
                    // An exclGroup is a container that can have a rawValue (the selected field's value)
                    // AND it contains field children that should be accessible via SOM paths.
                    // Register the exclGroup as a field-like node with its value
                    let value = extract_field_value_helper(&node.children);
                    engine.register_xfa_node(&name, &path, parent_path, true, &value);
                    // Also recurse into children to register the fields (RB_1, RB_2, etc.)
                    // This enables scripts like: Page.FormTitle.RB_Group_Neuanlage.RB_1.rawValue = 1
                    register_nodes_recursive(&node.children, Some(&path), engine);
                } else if is_subform {
                    // Register subform and recurse
                    engine.register_xfa_node(&name, &path, parent_path, false, "");
                    register_nodes_recursive(&node.children, Some(&path), engine);
                } else {
                    // Other container types (area, etc.) - just recurse through with same parent
                    register_nodes_recursive(&node.children, parent_path, engine);
                }
            }
        }
        
        /// Helper function to extract field value (to avoid Self:: in nested fn)
        fn extract_field_value_helper(children: &[XfaNode]) -> String {
            Flattened::extract_field_value(children)
        }
        
        // Start from the template root
        // Skip pageSet and go directly to content subforms
        // IMPORTANT: We must register content subforms FIRST, then floating fields AFTER
        // because floating fields need to be added as properties on all existing subforms
        fn find_and_register_from_template(nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
            // First pass: collect pageSet nodes for later
            let mut page_set_nodes: Vec<&XfaNode> = Vec::new();
            
            fn find_template_and_collect<'a>(
                nodes: &'a [XfaNode], 
                engine: &mut XfaScriptEngine,
                page_set_nodes: &mut Vec<&'a XfaNode>,
            ) {
                for node in nodes {
                    if matches!(node.kind, XfaNodeKind::Template) ||
                       matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "template")
                    {
                        // Found template, look for root subform
                        for child in &node.children {
                            if matches!(child.kind, XfaNodeKind::Subform) ||
                               matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform")
                            {
                                // This is the root container (like "UBSForms")
                                // Register its children (pageSet and content subforms)
                                for grandchild in &child.children {
                                    // Skip proto and variables elements
                                    if matches!(&grandchild.kind, XfaNodeKind::Element { tag_name, .. } 
                                        if tag_name == "variables" || tag_name == "proto")
                                    {
                                        continue;
                                    }
                                    
                                    // Save pageSet for later - we'll register floating fields AFTER subforms
                                    if matches!(grandchild.kind, XfaNodeKind::PageSet) ||
                                       matches!(&grandchild.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "pageSet")
                                    {
                                        page_set_nodes.push(grandchild);
                                        continue;
                                    }
                                    
                                    // Register content subforms and their children FIRST
                                    if let Some(name) = &grandchild.name
                                        && !name.is_empty() {
                                            let is_subform = matches!(grandchild.kind, XfaNodeKind::Subform) ||
                                                matches!(&grandchild.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                                            if is_subform {
                                                engine.register_xfa_node(name, name, None, false, "");
                                                register_nodes_recursive(&grandchild.children, Some(name), engine);
                                            }
                                        }
                                }
                            }
                        }
                        return;
                    }
                    // Recurse to find template
                    find_template_and_collect(&node.children, engine, page_set_nodes);
                }
            }
            
            // First: register all subforms
            find_template_and_collect(nodes, engine, &mut page_set_nodes);
            
            // Second: NOW register floating fields (after all subforms exist)
            // This allows floating fields to be added as properties on all subforms
            for page_set in page_set_nodes {
                register_floating_fields(&page_set.children, engine);
            }
        }
        
        /// Register floating fields from pageSet
        /// These are fields that can be embedded anywhere in the form via xfa:embed
        /// They need to be registered so xfa.resolveNode() can find them
        fn register_floating_fields(nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
            for node in nodes {
                // Check if this is a field
                let is_field = matches!(node.kind, XfaNodeKind::Field) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                
                if is_field
                    && let Some(name) = &node.name
                        && !name.is_empty() {
                            // Register floating field with just its name (no parent path)
                            let value = extract_field_value_helper(&node.children);
                            engine.register_xfa_node(name, name, None, true, &value);
                        }
                
                // Recurse into children (e.g., pageArea contains the floating fields)
                register_floating_fields(&node.children, engine);
            }
        }
        
        find_and_register_from_template(xfa_nodes, engine);
        
        // After registering the basic hierarchy, scan for xfa:embed references
        // and register floating fields at their embed locations
        // Per XFA spec, embedded fields appear in the SOM at their embed location
        Self::register_embedded_fields_at_locations(xfa_nodes, engine);
    }
    
    /// Scan for xfa:embed references and register floating fields at their embed locations.
    /// 
    /// Per XFA 3.3 spec: When a field is embedded via xfa:embed, it becomes part of the
    /// form DOM at the location where it's embedded, making it accessible via SOM paths
    /// like `Page.SectionTitle.STP_SectionTitle.ffrb1`.
    fn register_embedded_fields_at_locations(xfa_nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        // Build map of floating field ID -> field name
        let id_to_field = Self::build_id_to_field_map(xfa_nodes);
        
        /// Recursively scan for xfa:embed and register fields at their locations
        fn scan_for_embeds(
            nodes: &[XfaNode], 
            parent_path: Option<&str>,
            engine: &mut XfaScriptEngine,
            id_to_field: &HashMap<String, String>,
        ) {
            for node in nodes {
                let node_name = node.name.clone().unwrap_or_default();
                
                // Build current path
                let current_path = if !node_name.is_empty() {
                    match parent_path {
                        Some(p) => format!("{}.{}", p, node_name),
                        None => node_name.clone(),
                    }
                } else {
                    parent_path.map(|s| s.to_string()).unwrap_or_default()
                };
                
                // Check for xfa:embed attribute (references like "#floatingField010747")
                if let Some(embed_ref) = node.attributes.get("xfa:embed") {
                    // Parse the embed reference to get the field ID
                    let field_id = embed_ref.trim_start_matches('#');
                    
                    // Look up the field name from the ID
                    if let Some(field_name) = id_to_field.get(field_id) {
                        // Register this field as a child of the current container
                        if let Some(parent) = parent_path {
                            let embed_path = format!("{}.{}", parent, field_name);
                            // Register at the embed location (this makes Page.Section.field work)
                            engine.register_xfa_node(field_name, &embed_path, Some(parent), true, "");
                        }
                    }
                }
                
                // Determine if this node is a container that forms a new path segment
                let is_subform = matches!(node.kind, XfaNodeKind::Subform) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");
                
                // Recurse into children
                let child_parent = if (is_subform || is_exclgroup) && !node_name.is_empty() {
                    Some(current_path.as_str())
                } else {
                    parent_path
                };
                
                scan_for_embeds(&node.children, child_parent, engine, id_to_field);
            }
        }
        
        // Start scanning from the root content subform
        if let Some(root) = Self::find_root_subform(xfa_nodes) {
            let root_name = root.name.clone().unwrap_or_default();
            if !root_name.is_empty() {
                scan_for_embeds(&root.children, Some(&root_name), engine, &id_to_field);
            }
        }
    }

    /// Execute all form-ready scripts and return computed values.
    /// Presence changes are applied directly to the XFA nodes.
    fn execute_form_ready_scripts(
        xfa_nodes: &mut [XfaNode], 
        language: &str, 
        form_id: &str
    ) -> Result<HashMap<String, String>, String> {
        let mut computed_values = HashMap::new();
        let mut presence_changes: Vec<(String, Option<String>, Presence)> = Vec::new(); // (name, id, presence)
        let mut engine = XfaScriptEngine::new();
        
        // Register control fields used by scripts
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", language);
        engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", form_id);
        
        // Extract and register translation objects from the XFA
        Self::extract_and_register_translations(xfa_nodes, &mut engine);
        
        // Build the XFA SOM hierarchy for unqualified references
        // Per XFA 3.3 spec Chapter 3: unqualified references like "Page.FormTitle.Field"
        // must be resolvable by searching up the hierarchy from the current container.
        Self::build_and_register_xfa_som_hierarchy(xfa_nodes, &mut engine);
        
        // Build parent-child map for setting up `this.childField` access
        // This maps subform name -> list of (child_name, child_id) pairs
        let parent_child_map = Self::build_parent_child_map_with_ids(xfa_nodes);
        
        // Find all events recursively, tracking the node's child fields with their IDs
        // Returns: (parent_name, vec of (child_name, child_id), script)
        // Uses instance counters to match subforms/exclGroups without IDs to their children
        fn find_all_events_with_child_ids(
            nodes: &[XfaNode], 
            events: &mut Vec<(String, Vec<(String, String)>, crate::scripting::XfaScript)>,
            parent_child_map: &HashMap<String, Vec<(String, String)>>,
            subform_counters: &mut HashMap<String, usize>
        ) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                
                // Get this node's ID for looking up its children
                let node_id = node.attributes.get("id").cloned().unwrap_or_default();
                
                // Check if this is a subform or exclGroup (both need counter-based keys for children)
                let is_subform = matches!(node.kind, XfaNodeKind::Subform) ||
                    matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");
                
                // Build the key the same way as build_parent_child_map_with_ids
                // Both subforms and exclGroups use counter-based keys when they don't have IDs
                let key = if !node_id.is_empty() { 
                    format!("{}#{}", name, node_id) 
                } else if (is_subform || is_exclgroup) && !name.is_empty() {
                    // Use instance counter for subforms/exclGroups without IDs
                    let count = subform_counters.entry(name.clone()).or_insert(0);
                    let key = format!("{}[{}]", name, *count);
                    *count += 1;
                    key
                } else {
                    name.clone()
                };
                
                // Look up children using the computed key
                let children = parent_child_map.get(&key).cloned().unwrap_or_default();
                
                // Look for event children
                let node_events = parse_events_from_node(&node.children);
                for event in node_events {
                    events.push((name.clone(), children.clone(), event));
                }
                
                // Recurse into children
                find_all_events_with_child_ids(&node.children, events, parent_child_map, subform_counters);
            }
        }
        
        let mut subform_counters: HashMap<String, usize> = HashMap::new();
        let mut all_events = Vec::new();
        find_all_events_with_child_ids(xfa_nodes, &mut all_events, &parent_child_map, &mut subform_counters);
        
        // Execute events in proper XFA lifecycle order:
        // 1. Initialize events (activity="initialize") - these call setupVariables() etc.
        // 2. Ready events (activity="ready") - form-ready scripts that compute field values
        
        // Phase 1: Execute initialize events
        // These are typically scripts like: soCommonLabelDefinition.setupVariables()
        // IMPORTANT: Initialize scripts can set presence values on containers (e.g., parent.presence = "hidden")
        for (field_name, child_fields, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript 
                && script.activity == EventActivity::Initialize
            {
                // Set up field context with child fields (initialize scripts may reference children)
                engine.set_current_field_with_children(field_name, field_name, "", child_fields);
                
                // Execute the initialize script (maintains global context)
                let _ = engine.execute_script(script);
                
                // Collect presence values set on the current field
                if let Some(presence) = engine.get_current_field_presence() {
                    presence_changes.push((field_name.clone(), None, presence));
                }
                
                // Collect presence values set on child fields
                for (child_name, child_id) in child_fields {
                    if let Some((id, presence)) = engine.get_child_field_presence(child_name) {
                        let storage_id = if !id.is_empty() { Some(id) } else if !child_id.is_empty() { Some(child_id.clone()) } else { None };
                        presence_changes.push((child_name.clone(), storage_id, presence));
                    }
                }
                
                // Initialize scripts (especially those calling change()) can set values on fields
                // via xfa.resolveNode(). Collect these values from the field registry.
                let init_som_values = engine.get_all_som_field_values();
                for (init_field_name, init_value) in init_som_values {
                    if !init_value.is_empty() {
                        computed_values.insert(init_field_name, init_value);
                    }
                }
            }
        }
        
        // Phase 2: Execute form-ready JavaScript events
        // These compute field values after initialization is complete
        for (field_name, child_fields, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript 
                && script.activity == EventActivity::Ready 
                && script.event_ref == EventRef::Form 
                && !field_name.is_empty() 
            {
                // Set up field context with child fields as properties of `this`
                // This enables scripts like: this.ffDesSignature.rawValue = mySignatureClient
                // child_fields is Vec<(child_name, child_id)>
                engine.set_current_field_with_children(field_name, field_name, "", child_fields);
                
                // Execute the script
                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(field_name.clone(), value);
                }
                
                // Collect values set on child fields
                // Per XFA spec, scripts can set rawValue on child fields via this.childName
                // Store by UNIQUE ID to avoid collisions when multiple subforms have same-named children
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name)
                        && !child_value.is_empty() {
                            // Use the ID if available, otherwise fall back to the id from the pair
                            let storage_key = if !id.is_empty() { id } else { child_id.clone() };
                            
                            // Store by ID for unique identification
                            if !storage_key.is_empty() {
                                computed_values.insert(storage_key.clone(), child_value.clone());
                            }
                            // Also store by name for fallback lookups (will be overwritten by later instances)
                            computed_values.insert(child_name.clone(), child_value);
                        }
                }
            }
        }
        
        // Phase 3: Execute layout-ready JavaScript events
        // Per XFA 3.3 spec (page 388): "In the case of the Layout DOM ($layout), the ready event 
        // fires when the layout is complete but rendering has not yet begun. Thus a script can 
        // modify the layout before it is rendered."
        // For static flattening, we execute these after form:ready to ensure values are computed.
        for (field_name, child_fields, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript 
                && script.activity == EventActivity::Ready 
                && script.event_ref == EventRef::Layout 
                && !field_name.is_empty() 
            {
                // Set up field context with child fields as properties of `this`
                engine.set_current_field_with_children(field_name, field_name, "", child_fields);
                
                // Execute the script
                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(field_name.clone(), value);
                }
                
                // Collect values set on child fields (same logic as form:ready)
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name)
                        && !child_value.is_empty() {
                            let storage_key = if !id.is_empty() { id } else { child_id.clone() };
                            
                            if !storage_key.is_empty() {
                                computed_values.insert(storage_key.clone(), child_value.clone());
                            }
                            computed_values.insert(child_name.clone(), child_value);
                        }
                }
            }
        }
        
        // Phase 4: Collect all values from SOM hierarchy
        // This captures values set via SOM path references like:
        // `Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.RB_1.rawValue = 1`
        // These may not be captured by the this.childName collection above.
        let som_values = engine.get_all_som_field_values();
        for (field_name, value) in som_values {
            if !value.is_empty() {
                // Only insert if not already present (don't overwrite more specific values)
                computed_values.entry(field_name.clone()).or_insert(value);
            }
        }
        
        // Apply presence changes directly to the XFA tree
        Self::apply_presence_changes(xfa_nodes, &presence_changes);
        
        Ok(computed_values)
    }
    
    /// Apply presence changes collected from script execution directly to XFA nodes
    fn apply_presence_changes(nodes: &mut [XfaNode], changes: &[(String, Option<String>, Presence)]) {
        for (name, id, presence) in changes {
            // Try to find by ID first (more specific)
            if let Some(id_val) = id {
                if Self::apply_presence_by_id(nodes, id_val, *presence) {
                    continue;
                }
            }
            // Fall back to finding by name
            Self::apply_presence_by_name(nodes, name, *presence);
        }
    }
    
    /// Recursively find a node by ID and set its presence
    fn apply_presence_by_id(nodes: &mut [XfaNode], id: &str, presence: Presence) -> bool {
        for node in nodes {
            if node.attributes.get("id").map(|s| s.as_str()) == Some(id) {
                node.set_presence(presence);
                return true;
            }
            if Self::apply_presence_by_id(&mut node.children, id, presence) {
                return true;
            }
        }
        false
    }
    
    /// Recursively find a node by name and set its presence
    fn apply_presence_by_name(nodes: &mut [XfaNode], name: &str, presence: Presence) -> bool {
        for node in nodes {
            if node.name.as_deref() == Some(name) {
                node.set_presence(presence);
                return true;
            }
            if Self::apply_presence_by_name(&mut node.children, name, presence) {
                return true;
            }
        }
        false
    }
    
    /// Execute variable scripts from the XFA template.
    /// 
    /// According to XFA 3.3 spec (page 376-377), scripts in <variables> elements
    /// are compiled into script objects when the subform is instantiated during data binding.
    /// The script object is registered with the subform and can be referenced by name.
    /// 
    /// Per spec, variable scripts can:
    /// - Define functions (setupVariables, change, etc.)
    /// - Declare global variables when executed
    /// - Be accessed as named script objects (e.g., soLocalLabelDefinition.change())
    fn extract_and_register_translations(xfa_nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        // Collect all script contents from <variables> elements
        let mut variable_scripts: Vec<(String, String)> = Vec::new();
        Self::collect_variable_scripts(xfa_nodes, &mut variable_scripts);
        
        // Execute each variable script to create a named script object
        // Per XFA spec, variable scripts run in a context where they can define global variables
        // and functions. The script object is then accessible by its name.
        for (name, content) in &variable_scripts {
            // The script content typically defines functions and may declare globals.
            // We wrap it in an IIFE that:
            // 1. Executes the script content (which may set globals)
            // 2. Captures any defined functions and exposes them on a named object
            // 3. Wraps change() to sync exclGroup values first (per XFA spec)
            //
            // This follows XFA spec: "The script object is registered with the subform 
            // and can be referenced by name."
            let wrapped = format!(
                r#"
                var {name} = (function() {{
                    // Execute the script content in this scope
                    // Any assignments to undeclared variables become globals
                    // Any function declarations become local to this IIFE
                    {content}
                    
                    // Return an object exposing any functions defined in the script
                    var _obj = {{}};
                    if (typeof setupVariables === 'function') {{
                        _obj.setupVariables = function() {{ setupVariables(); }};
                    }}
                    if (typeof change === 'function') {{
                        // Wrap change() to sync exclGroup values first
                        // Per XFA spec, when a radio button's rawValue is set, the parent
                        // exclGroup's rawValue should also update before change() reads it
                        _obj.change = function() {{ 
                            if (typeof _xfa_sync_exclgroups_ === 'function') {{
                                _xfa_sync_exclgroups_();
                            }}
                            change(); 
                        }};
                    }}
                    // Expose any other common XFA script functions
                    if (typeof calculate === 'function') {{
                        _obj.calculate = function() {{ calculate(); }};
                    }}
                    if (typeof validate === 'function') {{
                        _obj.validate = function() {{ return validate(); }};
                    }}
                    return _obj;
                }})();
                "#,
                name = name,
                content = content
            );
            
            let _ = engine.execute_variable_script(&wrapped);
        }
    }
    
    /// Recursively collect script content from <variables> elements
    fn collect_variable_scripts(nodes: &[XfaNode], scripts: &mut Vec<(String, String)>) {
        for node in nodes {
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind
                && tag_name == "variables" {
                    // Look for script children
                    for child in &node.children {
                        if let XfaNodeKind::Element { tag_name: child_tag, text_content, .. } = &child.kind
                            && child_tag == "script"
                                && let Some(name) = child.name.as_ref().or_else(|| child.attributes.get("name"))
                                    && let Some(content) = text_content {
                                        scripts.push((name.clone(), content.clone()));
                                    }
                    }
                }
            
            // Recurse
            Self::collect_variable_scripts(&node.children, scripts);
        }
    }
    
    /// Create a flattened representation with pre-computed field values
    /// 
    /// Parameters:
    /// - `xfa_nodes`: The parsed XFA template nodes (presence already set by scripts)
    /// - `computed_values`: Map of field name -> computed value from scripts
    /// - `id_to_field`: Map of element ID -> field name for resolving xfa:embed references
    fn from_xfa_with_computed_values(
        xfa_nodes: &[XfaNode], 
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>,
    ) -> Result<Self, String> {
        let mut flattened_nodes = Vec::new();
        
        // Default to A4 size (210mm x 297mm in points)
        let mut page = Page { 
            width: Self::parse_dimension("210mm").unwrap_or_else(|_| num(595.27)), 
            height: Self::parse_dimension("297mm").unwrap_or_else(|_| num(841.89))
        };
        
        // Find page dimensions and contentArea offset from pageArea
        let mut content_offset_x = Decimal::ZERO;
        let mut content_offset_y = Decimal::ZERO;
        let mut content_width = page.width;
        let mut content_height = page.height;
        
        if let Some((page_area, content_area)) = Self::find_page_and_content_area(xfa_nodes) {
            // Get pageArea dimensions (defines the page size)
            if let Some(w) = page_area.w {
                page.width = w;
            }
            if let Some(h) = page_area.h {
                page.height = h;
            }
            
            // Get contentArea offset and dimensions (defines the usable area for form content)
            content_offset_x = content_area.x.unwrap_or(Decimal::ZERO);
            content_offset_y = content_area.y.unwrap_or(Decimal::ZERO);
            content_width = content_area.w.unwrap_or(page.width);
            content_height = content_area.h.unwrap_or(page.height);
            
            // ============================================================
            // STEP 1: Render PAGE BACKGROUND (from Template DOM's pageArea)
            // ============================================================
            // Per XFA spec (section 7, "Page Background"):
            // "A pageArea may contain content such as subforms. Such content, which is placed
            // directly in a pageArea element, represents page background."
            // 
            // Page background elements are positioned relative to the page origin (0,0),
            // NOT the contentArea. They use positioned layout (absolute coordinates).
            let page_position = Position::new(Decimal::ZERO, Decimal::ZERO, page.width, page.height);
            
            // Create context for page background
            let page_ctx = FlattenContext::new(computed_values, id_to_field);
            
            for child in &page_area.children {
                // Skip contentArea and medium - these define page structure, not content
                if matches!(child.kind, XfaNodeKind::ContentArea) {
                    continue;
                }
                if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                    && (tag_name == "contentArea" || tag_name == "medium") {
                        continue;
                    }
                
                // Render page background element with positioned layout relative to page origin
                Self::flatten_single_node(child, page_position, Layout::Position, &mut flattened_nodes, &page_ctx)?;
            }
        }
        
        // ============================================================
        // STEP 2: Render FORM CONTENT (from Form DOM) INTO contentArea
        // ============================================================
        // Per XFA spec: "The Form DOM is the place where the data from the XFA Data DOM
        // is bound to logical structure from the Template DOM."
        // 
        // The root content subform (Form DOM root) is rendered into the contentArea.
        // Its position is offset by the contentArea's x,y coordinates.
        let root_position = Position::new(
            content_offset_x, 
            content_offset_y, 
            content_width, 
            content_height
        );
        
        // Create flatten context for resolving xfa:embed references during text extraction
        let ctx = FlattenContext::new(computed_values, id_to_field);
        
        // Find and flatten the root content subform (the Form DOM)
        // This is the sibling to pageSet, NOT inside pageArea
        if let Some(root_subform) = Self::find_root_subform(xfa_nodes) {
            // Get the layout from the root subform (often "tb" for top-to-bottom)
            let layout = root_subform.layout.as_ref()
                .map(|l| Layout::from_str(l))
                .unwrap_or(Layout::Position);
            
            Self::flatten_nodes(&root_subform.children, root_position, layout, &mut flattened_nodes, &ctx)?;
        } else {
            // Fallback: flatten all nodes (old behavior for simple forms without proper structure)
            Self::flatten_nodes(xfa_nodes, root_position, Layout::Position, &mut flattened_nodes, &ctx)?;
        };
        
        // Apply computed values from scripts to nodes
        for node in &mut flattened_nodes {
            match &mut node.kind {
                FlattenedNodeKind::Field { name, value, .. } => {
                    // If we have a computed value for this field and it currently has no value,
                    // use the computed value
                    if value.is_empty()
                        && let Some(computed) = computed_values.get(name) {
                            *value = computed.clone();
                        }
                }
                FlattenedNodeKind::Text { content, source_name, .. } => {
                    // For Draw elements with a source name, check if we have a computed value
                    if let Some(name) = source_name
                        && content.is_empty()
                            && let Some(computed) = computed_values.get(name) {
                                *content = computed.clone();
                            }
                }
            }
        }
        
        Ok(Flattened {
            page,
            nodes: flattened_nodes,
        })
    }
    
    /// Find the content subform (the Form DOM root)
    /// 
    /// Per XFA spec:
    /// - Template DOM contains: pageSet (page structure) + root subform (form content)
    /// - Form DOM is a duplicate of the subtree under the root subform (NOT including pageSet)
    /// - The root content subform is typically a sibling to pageSet, not inside it
    /// - xfa:datasets contains data and should NOT be used for layout
    /// 
    /// Structure example:
    ///   template
    ///     subform 'UBSForms' (root container)
    ///       pageSet 'MPs'        <-- page structure (stays in Template DOM)
    ///       subform 'Page'       <-- root content subform (becomes Form DOM)
    fn find_root_subform(nodes: &[XfaNode]) -> Option<&XfaNode> {
        /// Helper to check if a node is a pageSet or similar page structure element
        fn is_page_structure(node: &XfaNode) -> bool {
            matches!(node.kind, XfaNodeKind::PageSet | XfaNodeKind::PageArea | XfaNodeKind::ContentArea) ||
            matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } 
                if tag_name == "pageSet" || tag_name == "pageArea" || tag_name == "contentArea")
        }
        
        /// Helper to check if a node is a non-content element (variables, proto, desc, event, etc.)
        fn is_non_content_element(node: &XfaNode) -> bool {
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                matches!(tag_name.as_str(), "variables" | "proto" | "desc" | "event" | "extras" | "occur")
            } else {
                false
            }
        }
        
        /// Helper to check if a node is a data-only element that should be skipped for layout
        fn is_data_element(node: &XfaNode) -> bool {
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                // Skip xfa:datasets, xfa:data, form (Form DOM), and similar data containers
                // The "form" element is the Form DOM root - it's a duplicate of Template content
                tag_name.starts_with("xfa:") || 
                tag_name.starts_with("dd:") ||  // data description
                tag_name == "datasets" || 
                tag_name == "data" ||
                tag_name == "form"  // Form DOM - duplicates Template content
            } else {
                false
            }
        }
        
        /// Find content subform inside a container subform (sibling to pageSet)
        fn find_content_subform_in_container(container: &XfaNode) -> Option<&XfaNode> {
            // Look for a subform that is NOT a pageSet and NOT a non-content element
            // This is the actual content subform that goes into the Form DOM
            for child in &container.children {
                if is_page_structure(child) || is_non_content_element(child) {
                    continue;
                }
                
                // Found a content subform
                if matches!(child.kind, XfaNodeKind::Subform) {
                    return Some(child);
                }
                if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                    && tag_name == "subform" {
                        return Some(child);
                    }
            }
            None
        }
        
        fn search_recursive(nodes: &[XfaNode]) -> Option<&XfaNode> {
            for node in nodes {
                // Skip data elements - we only want Template DOM content
                if is_data_element(node) {
                    continue;
                }
                
                // Look in template
                if matches!(node.kind, XfaNodeKind::Template) {
                    // Template's direct child subform is the root container
                    for child in &node.children {
                        if matches!(child.kind, XfaNodeKind::Subform) {
                            // This is the root container subform (e.g., 'UBSForms')
                            // Look for the content subform inside it (sibling to pageSet)
                            if let Some(content_subform) = find_content_subform_in_container(child) {
                                return Some(content_subform);
                            }
                            // If no content subform found, the container itself might be the content
                            // (for simpler forms without separate pageSet)
                            let has_page_set = child.children.iter().any(is_page_structure);
                            if !has_page_set {
                                return Some(child);
                            }
                        }
                    }
                }
                
                // Check Element nodes for template
                if let XfaNodeKind::Element { tag_name, .. } = &node.kind
                    && tag_name == "template" {
                        for child in &node.children {
                            let is_subform = matches!(child.kind, XfaNodeKind::Subform) ||
                                matches!(&child.kind, XfaNodeKind::Element { tag_name: ct, .. } if ct == "subform");
                            
                            if is_subform {
                                // This is the root container subform
                                if let Some(content_subform) = find_content_subform_in_container(child) {
                                    return Some(content_subform);
                                }
                                // Fallback: use the container if no pageSet
                                let has_page_set = child.children.iter().any(is_page_structure);
                                if !has_page_set {
                                    return Some(child);
                                }
                            }
                        }
                    }
                
                // Only recurse into Template or container nodes, skip data elements
                if !is_data_element(node)
                    && let Some(result) = search_recursive(&node.children) {
                        return Some(result);
                    }
            }
            None
        }
        search_recursive(nodes)
    }
    
    /// Flatten a single node (used for pageArea children)
    fn flatten_single_node(
        node: &XfaNode,
        parent_position: Position,
        _parent_layout: Layout,
        flattened_nodes: &mut Vec<FlattenedNode>,
        ctx: &FlattenContext,
    ) -> Result<(), String> {
        // Check presence - per XFA spec, hidden/inactive nodes should not be rendered
        // This is critical for fields whose values are set by scripts but should remain hidden
        let presence = ctx.get_effective_presence(node);
        if presence.should_skip_layout() {
            // Hidden/Inactive: skip entirely - don't render, don't consume layout space
            return Ok(());
        }
        
        // For positioned layout, use node's x,y directly
        let x = node.x.unwrap_or(Decimal::ZERO);
        let y = node.y.unwrap_or(Decimal::ZERO);
        
        // Per XFA spec: if w is not specified, the element is horizontally growable.
        // Use minW as the width, or calculate natural width for Draw elements.
        let width = node.w.unwrap_or_else(|| {
            // For Draw elements without explicit width, use minW or natural text width
            if let XfaNodeKind::Draw = &node.kind {
                let text = ctx.extract_text(&node.children).unwrap_or_default();
                let natural_width = Self::calculate_natural_text_width(&text, &node.font);
                let min_w = node.min_w.unwrap_or(Decimal::ZERO);
                natural_width.max(min_w)
            } else {
                // For other elements, use minW or fall back to parent width
                node.min_w.unwrap_or(parent_position.width)
            }
        });
        let height = node.h.unwrap_or_else(|| num(20.0));
        
        let pos = Position::new(
            parent_position.x + x,
            parent_position.y + y,
            width,
            height,
        );
        
        match &node.kind {
            XfaNodeKind::Draw => {
                // Extract text content, or use empty string if none (scripts may fill it later)
                // Use context to resolve xfa:embed references
                let text_content = ctx.extract_text(&node.children).unwrap_or_default();
                let font_size = Self::extract_font_size(node);
                let font_name = Self::extract_font_name(node);
                let style = Self::extract_style(node);
                
                // Get default h_align from XFA para element
                let default_h_align = node.para.as_ref().map(|p| p.h_align).unwrap_or(HAlign::Left);
                
                // Extract rich text if this is HTML content (exData with contentType="text/html")
                let rich_text = Self::extract_rich_text_from_node(&node.children, default_h_align);
                
                flattened_nodes.push(FlattenedNode::new_text_with_rich_text(
                    text_content,
                    font_size,
                    font_name,
                    pos.x,
                    pos.y,
                    pos.width,
                    pos.height,
                    style,
                    node.rotate,
                    node.name.clone(),
                    rich_text,
                ));
            }
            XfaNodeKind::Field => {
                let field_name = node.name.clone().unwrap_or_else(|| "unnamed".to_string());
                let field_value = Self::extract_field_value(&node.children);
                let style = Self::extract_style(node);
                
                flattened_nodes.push(FlattenedNode::new_field_styled_rotated(
                    field_name.clone(),
                    field_value,
                    field_name,
                    pos.x,
                    pos.y,
                    pos.width,
                    pos.height,
                    style,
                    node.rotate,
                ));
            }
            XfaNodeKind::Subform | XfaNodeKind::Element { .. } => {
                // Recurse into subform children with positioned layout
                for child in &node.children {
                    Self::flatten_single_node(child, pos, Layout::Position, flattened_nodes, ctx)?;
                }
            }
            _ => {}
        }
        
        Ok(())
    }
    
    fn find_page_and_content_area(nodes: &[XfaNode]) -> Option<(&XfaNode, &XfaNode)> {
        fn search_recursive(nodes: &[XfaNode]) -> Option<(&XfaNode, &XfaNode)> {
            for node in nodes {
                // Check for PageArea node type
                if matches!(node.kind, XfaNodeKind::PageArea) {
                    // Found pageArea, now look for contentArea within it
                    for child in &node.children {
                        if matches!(child.kind, XfaNodeKind::ContentArea) {
                            return Some((node, child));
                        }
                        // Also check Element nodes that might be contentArea
                        if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                            && tag_name == "contentArea" {
                                return Some((node, child));
                            }
                    }
                    // If no contentArea found, return pageArea twice (use page dimensions)
                    return Some((node, node));
                }
                
                // Check for pageArea as Element
                if let XfaNodeKind::Element { tag_name, .. } = &node.kind
                    && tag_name == "pageArea" {
                        // Found pageArea as Element, look for contentArea
                        for child in &node.children {
                            if matches!(child.kind, XfaNodeKind::ContentArea) {
                                return Some((node, child));
                            }
                            if let XfaNodeKind::Element { tag_name: ca_tag, .. } = &child.kind
                                && ca_tag == "contentArea" {
                                    return Some((node, child));
                                }
                        }
                        return Some((node, node));
                    }
                
                // Recurse into all container-like nodes to find pageArea
                let should_recurse = matches!(node.kind, 
                    XfaNodeKind::Template | XfaNodeKind::PageSet | XfaNodeKind::Subform)
                    || matches!(&node.kind, XfaNodeKind::Element { .. });
                    
                if should_recurse
                    && let Some(result) = search_recursive(&node.children) {
                        return Some(result);
                    }
            }
            None
        }
        search_recursive(nodes)
    }
    
    /// Extract style information from an XFA node
    fn extract_style(node: &XfaNode) -> RenderStyle {
        RenderStyle {
            border: node.border.clone(),
            font: node.font.clone(),
            para: node.para.clone(),
        }
    }
    
    /// Extract font size from node, with default fallback
    fn extract_font_size(node: &XfaNode) -> Num {
        node.font.as_ref().map(|f| f.size).unwrap_or_else(|| num(10.0))
    }
    
    /// Extract font name from node, with default fallback
    fn extract_font_name(node: &XfaNode) -> String {
        node.font.as_ref().map(|f| f.typeface.clone()).unwrap_or_else(|| "Helvetica".to_string())
    }
    
    /// Count the number of paragraphs in HTML content.
    /// Used for height calculation to account for paragraph breaks.
    fn count_html_paragraphs(children: &[XfaNode]) -> usize {
        let mut count = 0;
        Self::count_paragraphs_recursive(children, &mut count);
        count.max(1) // At least 1 paragraph
    }
    
    fn count_paragraphs_recursive(children: &[XfaNode], count: &mut usize) {
        for child in children {
            match &child.kind {
                XfaNodeKind::Element { tag_name, .. } => {
                    let tag_lower = tag_name.to_lowercase();
                    if tag_lower == "p" {
                        *count += 1;
                    }
                    // Recurse into children
                    Self::count_paragraphs_recursive(&child.children, count);
                }
                XfaNodeKind::Value => {
                    Self::count_paragraphs_recursive(&child.children, count);
                }
                _ => {
                    Self::count_paragraphs_recursive(&child.children, count);
                }
            }
        }
    }
    
    /// Check if a node contains HTML exData content
    fn has_html_exdata(children: &[XfaNode]) -> bool {
        for child in children {
            if matches!(child.kind, XfaNodeKind::Value) {
                for value_child in &child.children {
                    if let XfaNodeKind::Element { tag_name, .. } = &value_child.kind
                        && tag_name == "exData" {
                            for ex_child in &value_child.children {
                                if let XfaNodeKind::Element { tag_name: inner_tag, .. } = &ex_child.kind
                                    && inner_tag == "body" {
                                        return true;
                                    }
                            }
                        }
                }
            }
        }
        false
    }

    /// Extract rich text from a node's value children.
    /// Handles both:
    /// - HTML exData content (with paragraph styling, text-indent, xfa-spacerun)
    /// - Plain <text> elements containing U+2029 paragraph separators
    fn extract_rich_text_from_node(children: &[XfaNode], default_h_align: HAlign) -> Option<RichText> {
        for child in children {
            // Check for XfaNodeKind::Value
            if matches!(child.kind, XfaNodeKind::Value) {
                for value_child in &child.children {
                    if let XfaNodeKind::Element { tag_name, text_content } = &value_child.kind {
                        // Check for <text> element with U+2029 paragraph separators
                        if tag_name == "text"
                            && let Some(text) = text_content
                                && text.contains('\u{2029}') {
                                    // Create rich text from plain text with paragraph separators
                                    return Some(Self::create_rich_text_from_plain_with_separators(text, default_h_align));
                                }
                        
                        if tag_name == "exData" {
                            // Check if it has HTML body content
                            for ex_child in &value_child.children {
                                if let XfaNodeKind::Element { tag_name: inner_tag, .. } = &ex_child.kind
                                    && inner_tag == "body" {
                                        // Found HTML body - parse it into RichText
                                        return Some(Self::parse_rich_text_from_html(&value_child.children, default_h_align));
                                    }
                            }
                        }
                    }
                }
            }
            // Also check for Element with tag_name "value"
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                && tag_name == "value" {
                    for value_child in &child.children {
                        if let XfaNodeKind::Element { tag_name: inner_tag, text_content } = &value_child.kind {
                            // Check for <text> element with U+2029 paragraph separators
                            if inner_tag == "text"
                                && let Some(text) = text_content
                                    && text.contains('\u{2029}') {
                                        return Some(Self::create_rich_text_from_plain_with_separators(text, default_h_align));
                                    }
                            
                            if inner_tag == "exData" {
                                for ex_child in &value_child.children {
                                    if let XfaNodeKind::Element { tag_name: body_tag, .. } = &ex_child.kind
                                        && body_tag == "body" {
                                            return Some(Self::parse_rich_text_from_html(&value_child.children, default_h_align));
                                        }
                                }
                            }
                        }
                    }
                }
        }
        None
    }
    
    /// Create rich text structure from plain text containing U+2029 paragraph separators.
    /// Each segment separated by U+2029 becomes a separate paragraph.
    fn create_rich_text_from_plain_with_separators(text: &str, default_h_align: HAlign) -> RichText {
        let segments: Vec<&str> = text.split('\u{2029}').collect();
        let mut paragraphs = Vec::new();
        
        for segment in segments {
            // Normalize whitespace in each segment
            let normalized = Self::normalize_whitespace(segment);
            
            let mut para = RichParagraph {
                h_align: default_h_align,
                ..RichParagraph::default()
            };
            
            if !normalized.is_empty() {
                para.runs.push(RichRun {
                    text: normalized,
                    preserve_spaces: false,
                    bold: false,
                    italic: false,
                    underline: false,
                });
            } else {
                para.is_empty = true;
            }
            
            paragraphs.push(para);
        }
        
        // If no paragraphs were created, add an empty one
        if paragraphs.is_empty() {
            paragraphs.push(RichParagraph {
                h_align: default_h_align,
                is_empty: true,
                ..RichParagraph::default()
            });
        }
        
        RichText { paragraphs }
    }

    fn flatten_nodes(
        nodes: &[XfaNode],
        parent_position: Position,
        parent_layout: Layout,
        flattened_nodes: &mut Vec<FlattenedNode>,
        ctx: &FlattenContext,
    ) -> Result<Num, String> {
        // Returns the total height consumed by these nodes
        // Initialize flow position based on layout direction
        // For right-to-left layouts, start from the right edge
        let mut current_x = match parent_layout {
            Layout::RightToLeftTopToBottom | Layout::RightToLeftRow => {
                parent_position.x + parent_position.width
            }
            _ => parent_position.x,
        };
        let mut current_y = parent_position.y;
        let mut max_height_in_row = Decimal::ZERO;
        let start_y = parent_position.y;
        
        // For positioned layout, track the maximum extent (bottom-most point of all children)
        // This is needed when this container has no explicit height - we compute it from children
        let mut max_extent_y = Decimal::ZERO;
        
        for node in nodes {
            // Check presence attribute using the context
            // This considers: inherited presence > script-set presence > static attribute
            // Per XFA spec (section 2, "Explicitly Concealing Containers"):
            // - Visible - element is rendered and participates in layout (normal behavior)
            // - Invisible - element takes up space but is NOT rendered (participates in layout)
            // - Hidden - element does NOT take up space and is NOT rendered (no layout)
            // - Inactive - element does NOT take up space and is NOT rendered (no layout, no automation)
            let presence = ctx.get_effective_presence(node);
            let skip_render = presence.should_skip_render();
            
            if presence.should_skip_layout() {
                // Hidden/Inactive: skip entirely - don't render, don't consume layout space
                continue;
            }
            
            // Create child context - if this node's presence is hidden/inactive, 
            // children inherit that presence
            let child_ctx = if presence.should_skip_layout() {
                ctx.with_inherited_presence(presence)
            } else {
                // Pass through existing inherited presence or none
                if let Some(inherited) = ctx.inherited_presence {
                    ctx.with_inherited_presence(inherited)
                } else {
                    // No inheritance needed, use visible as default
                    ctx.with_inherited_presence(Presence::Visible)
                }
            };
            
            match &node.kind {
                XfaNodeKind::Subform => {
                    let (outer_pos, content_pos, layout, consumed_height) = Self::compute_position_for_node_with_children(
                        node,
                        parent_position,
                        parent_layout,
                        &mut current_x,
                        &mut current_y,
                        &mut max_height_in_row,
                        flattened_nodes,
                        &child_ctx,
                    )?;
                    
                    // For positioned layout, track the max extent (relative to parent_position)
                    if parent_layout == Layout::Position {
                        let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                        max_extent_y = max_extent_y.max(node_bottom);
                    }
                    
                    // Recurse into subform children with the content position (inside margins)
                    // The subform's layout applies to its children
                    // Pass child_ctx to propagate inherited presence to children
                    let children_height = Self::flatten_nodes(&node.children, content_pos, layout, flattened_nodes, &child_ctx)?;;
                    
                    // For tb layout, update current_y based on actual content height if no explicit height
                    if parent_layout == Layout::TopToBottom && node.h.is_none() {
                        // The subform grew based on its children - update flow position
                        let actual_height = children_height + node.margin_top.unwrap_or(Decimal::ZERO) + node.margin_bottom.unwrap_or(Decimal::ZERO);
                        let min_h = node.min_h.unwrap_or(Decimal::ZERO);
                        let effective_height = actual_height.max(min_h).max(consumed_height);
                        
                        // Adjust current_y if children consumed more height than the default
                        if effective_height > consumed_height {
                            current_y = outer_pos.y + effective_height;
                        }
                    }
                }
                XfaNodeKind::Field => {
                    let (outer_pos, content_pos, _layout, _) = Self::compute_position_for_node_with_children(
                        node,
                        parent_position,
                        parent_layout,
                        &mut current_x,
                        &mut current_y,
                        &mut max_height_in_row,
                        flattened_nodes,
                        &child_ctx,
                    )?;
                    
                    // For positioned layout, track the max extent (relative to parent_position)
                    if parent_layout == Layout::Position {
                        let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                        max_extent_y = max_extent_y.max(node_bottom);
                    }
                    
                    // Only add to output if not hidden
                    // For fields, use outer_pos for rendering (field box includes margins)
                    if !skip_render {
                        let field_name = node.name.clone().unwrap_or_else(|| "unnamed".to_string());
                        let field_value = Self::extract_field_value(&node.children);
                        let style = Self::extract_style(node);
                        
                        flattened_nodes.push(FlattenedNode::new_field_styled_rotated(
                            field_name.clone(),
                            field_value,
                            field_name,
                            content_pos.x,
                            content_pos.y,
                            content_pos.width,
                            content_pos.height,
                            style,
                            node.rotate,
                        ));
                    }
                    
                    // Don't recurse into field children for positioning
                }
                XfaNodeKind::Draw => {
                    let (outer_pos, content_pos, _layout, _) = Self::compute_position_for_node_with_children(
                        node,
                        parent_position,
                        parent_layout,
                        &mut current_x,
                        &mut current_y,
                        &mut max_height_in_row,
                        flattened_nodes,
                        ctx,
                    )?;
                    
                    // For positioned layout, track the max extent (relative to parent_position)
                    if parent_layout == Layout::Position {
                        let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                        max_extent_y = max_extent_y.max(node_bottom);
                    }
                    
                    // Only add to output if not hidden
                    if !skip_render {
                        // Extract text content from draw node, or use empty (scripts may fill it)
                        // Use context to resolve xfa:embed references
                        let text_content = child_ctx.extract_text(&node.children).unwrap_or_default();
                        let font_size = Self::extract_font_size(node);
                        let font_name = Self::extract_font_name(node);
                        let style = Self::extract_style(node);
                        
                        // Get default h_align from XFA para element
                        let default_h_align = node.para.as_ref().map(|p| p.h_align).unwrap_or(HAlign::Left);
                        
                        // Extract rich text if this is HTML content (exData with contentType="text/html")
                        // This preserves paragraph structure, text-indent, and xfa-spacerun spacing
                        let rich_text = Self::extract_rich_text_from_node(&node.children, default_h_align);
                        
                        flattened_nodes.push(FlattenedNode::new_text_with_rich_text(
                            text_content,
                            font_size,
                            font_name,
                            content_pos.x,
                            content_pos.y,
                            content_pos.width,
                            content_pos.height,
                            style,
                            node.rotate,
                            node.name.clone(),
                            rich_text,
                        ));
                    }
                    
                    // Don't recurse into draw children for positioning
                }
                XfaNodeKind::Element { tag_name, .. } => {
                    // Handle generic elements that might be containers
                    match tag_name.as_str() {
                        "subform" => {
                            let (outer_pos, content_pos, layout, consumed_height) = Self::compute_position_for_node_with_children(
                                node,
                                parent_position,
                                parent_layout,
                                &mut current_x,
                                &mut current_y,
                                &mut max_height_in_row,
                                flattened_nodes,
                                &child_ctx,
                            )?;
                            
                            // For positioned layout, track the max extent (relative to parent_position)
                            if parent_layout == Layout::Position {
                                let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                                max_extent_y = max_extent_y.max(node_bottom);
                            }
                            
                            let children_height = Self::flatten_nodes(&node.children, content_pos, layout, flattened_nodes, &child_ctx)?;
                            
                            // For tb layout, update current_y based on actual content height
                            if parent_layout == Layout::TopToBottom && node.h.is_none() {
                                let actual_height = children_height + node.margin_top.unwrap_or(Decimal::ZERO) + node.margin_bottom.unwrap_or(Decimal::ZERO);
                                let min_h = node.min_h.unwrap_or(Decimal::ZERO);
                                let effective_height = actual_height.max(min_h).max(consumed_height);
                                
                                if effective_height > consumed_height {
                                    current_y = outer_pos.y + effective_height;
                                }
                            }
                        }
                        "field" => {
                            let (outer_pos, content_pos, _layout, _) = Self::compute_position_for_node_with_children(
                                node,
                                parent_position,
                                parent_layout,
                                &mut current_x,
                                &mut current_y,
                                &mut max_height_in_row,
                                flattened_nodes,
                                &child_ctx,
                            )?;
                            
                            // For positioned layout, track the max extent (relative to parent_position)
                            if parent_layout == Layout::Position {
                                let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                                max_extent_y = max_extent_y.max(node_bottom);
                            }
                            
                            // Only add to output if not hidden
                            if !skip_render {
                                let field_name = node.name.clone().unwrap_or_else(|| "unnamed".to_string());
                                let field_value = Self::extract_field_value(&node.children);
                                let style = Self::extract_style(node);
                                
                                flattened_nodes.push(FlattenedNode::new_field_styled_rotated(
                                    field_name,
                                    field_value.clone(),
                                    field_value,
                                    content_pos.x,
                                    content_pos.y,
                                    content_pos.width,
                                    content_pos.height,
                                    style,
                                    node.rotate,
                                ));
                            }
                        }
                        "draw" => {
                            let (outer_pos, content_pos, _layout, _) = Self::compute_position_for_node_with_children(
                                node,
                                parent_position,
                                parent_layout,
                                &mut current_x,
                                &mut current_y,
                                &mut max_height_in_row,
                                flattened_nodes,
                                &child_ctx,
                            )?;
                            
                            // For positioned layout, track the max extent (relative to parent_position)
                            if parent_layout == Layout::Position {
                                let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                                max_extent_y = max_extent_y.max(node_bottom);
                            }
                            
                            // Only add to output if not hidden
                            if !skip_render {
                                // Draw nodes render text or images - use empty string if no content (scripts may fill it)
                                // Use context to resolve xfa:embed references
                                let text_content = child_ctx.extract_text(&node.children).unwrap_or_default();
                                let font_size = Self::extract_font_size(node);
                                let font_name = Self::extract_font_name(node);
                                let style = Self::extract_style(node);
                                
                                // Get default h_align from XFA para element
                                let default_h_align = node.para.as_ref().map(|p| p.h_align).unwrap_or(HAlign::Left);
                                
                                // Extract rich text if this is HTML content (exData with contentType="text/html")
                                let rich_text = Self::extract_rich_text_from_node(&node.children, default_h_align);
                                
                                flattened_nodes.push(FlattenedNode::new_text_with_rich_text(
                                    text_content,
                                    font_size,
                                    font_name,
                                    content_pos.x,
                                    content_pos.y,
                                    content_pos.width,
                                    content_pos.height,
                                    style,
                                    node.rotate,
                                    node.name.clone(),
                                    rich_text,
                                ));
                            }
                            
                            // Don't recurse into draw children for positioning
                        }
                        "template" | "pageSet" | "pageArea" | "contentArea" => {
                            // NOTE: These should NOT normally be encountered when processing Form DOM content,
                            // since find_root_subform returns the content subform (sibling to pageSet).
                            // This fallback handles edge cases or malformed documents.
                            // Per XFA spec: pageSet/pageArea define page structure and are NOT part of Form DOM.
                            let child_layout = if tag_name == "pageArea" {
                                Layout::Position
                            } else {
                                parent_layout
                            };
                            
                            Self::flatten_nodes(&node.children, parent_position, child_layout, flattened_nodes, &child_ctx)?;
                        }
                        "exclGroup" => {
                            // Per XFA spec (section 17 "The exclGroup element"):
                            // exclGroup is a container element with x, y, w, h, layout, and other positioning attributes.
                            // It should be treated like a subform for layout purposes - compute its position
                            // and use that as the parent position for its children (the radio button fields).
                            let (outer_pos, content_pos, layout, consumed_height) = Self::compute_position_for_node_with_children(
                                node,
                                parent_position,
                                parent_layout,
                                &mut current_x,
                                &mut current_y,
                                &mut max_height_in_row,
                                flattened_nodes,
                                &child_ctx,
                            )?;
                            
                            // For positioned layout, track the max extent (relative to parent_position)
                            if parent_layout == Layout::Position {
                                let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                                max_extent_y = max_extent_y.max(node_bottom);
                            }
                            
                            // Recurse into exclGroup children with the computed content position
                            // The exclGroup's layout applies to its children (the fields)
                            let children_height = Self::flatten_nodes(&node.children, content_pos, layout, flattened_nodes, &child_ctx)?;
                            
                            // For tb layout, update current_y based on actual content height if no explicit height
                            if parent_layout == Layout::TopToBottom && node.h.is_none() {
                                let actual_height = children_height + node.margin_top.unwrap_or(Decimal::ZERO) + node.margin_bottom.unwrap_or(Decimal::ZERO);
                                let min_h = node.min_h.unwrap_or(Decimal::ZERO);
                                let effective_height = actual_height.max(min_h).max(consumed_height);
                                
                                if effective_height > consumed_height {
                                    current_y = outer_pos.y + effective_height;
                                }
                            }
                        }
                        // Skip data-only elements - these are Form DOM data, not layout
                        _ if tag_name.starts_with("xfa:") || 
                             tag_name.starts_with("dd:") || 
                             tag_name == "datasets" || 
                             tag_name == "data" ||
                             tag_name == "form" => {
                            // Skip xfa:datasets, xfa:data, form (Form DOM), etc. - they contain duplicate data
                        }
                        _ => {
                            // Other elements, recurse with current position
                            Self::flatten_nodes(&node.children, parent_position, parent_layout, flattened_nodes, &child_ctx)?;
                        }
                    }
                }
                XfaNodeKind::Template | XfaNodeKind::ContentArea | XfaNodeKind::PageSet => {
                    // NOTE: These should NOT normally be encountered when processing Form DOM content.
                    // This handles fallback cases. Pass through with same parent position and layout.
                    Self::flatten_nodes(&node.children, parent_position, parent_layout, flattened_nodes, &child_ctx)?;
                }
                XfaNodeKind::PageArea => {
                    // NOTE: PageArea should NOT normally be encountered when processing Form DOM content.
                    // Page background (pageArea children) are handled separately in from_xfa().
                    // This fallback handles edge cases - pass through with positioned layout.
                    Self::flatten_nodes(&node.children, parent_position, Layout::Position, flattened_nodes, &child_ctx)?;
                }
                _ => {}
            }
        }
        
        // Return the total height consumed
        // For positioned layout, use max_extent_y (the bottom-most point of all children)
        // For flowing layouts, use current_y - start_y
        match parent_layout {
            Layout::Position => Ok(max_extent_y),
            _ => Ok(current_y - start_y + max_height_in_row),
        }
    }
    
    /// Compute position for a node, considering its children for height calculation
    /// Returns (outer_position, content_position, layout, height_consumed)
    fn compute_position_for_node_with_children(
        node: &XfaNode,
        parent_position: Position,
        parent_layout: Layout,
        current_x: &mut Num,
        current_y: &mut Num,
        max_height_in_row: &mut Num,
        _flattened_nodes: &mut Vec<FlattenedNode>,
        ctx: &FlattenContext,
    ) -> Result<(Position, Position, Layout, Num), String> {
        // Check if explicit coordinates are provided
        let has_explicit_x = node.x.is_some();
        let has_explicit_y = node.y.is_some();
        
        // Get dimensions from node's parsed layout attributes
        let x = node.x.unwrap_or(Decimal::ZERO);
        let y = node.y.unwrap_or(Decimal::ZERO);
        
        // Per XFA spec: if w is not specified, the element is horizontally growable.
        // - For Draw elements: use natural text width (constrained by minW/maxW)
        // - For other elements: use minW if available, otherwise parent width
        let width = node.w.unwrap_or_else(|| {
            match &node.kind {
                XfaNodeKind::Draw => {
                    // Calculate natural width from text content
                    let text = ctx.extract_text(&node.children).unwrap_or_default();
                    let natural_width = Self::calculate_natural_text_width(&text, &node.font);
                    let min_w = node.min_w.unwrap_or(Decimal::ZERO);
                    let max_w = node.max_w;
                    
                    // Constrain by minW and maxW
                    let width = natural_width.max(min_w);
                    if let Some(max) = max_w {
                        width.min(max)
                    } else {
                        width
                    }
                }
                XfaNodeKind::Element { tag_name, .. } if tag_name == "draw" => {
                    // Same logic for generic draw elements
                    let text = ctx.extract_text(&node.children).unwrap_or_default();
                    let natural_width = Self::calculate_natural_text_width(&text, &node.font);
                    let min_w = node.min_w.unwrap_or(Decimal::ZERO);
                    let max_w = node.max_w;
                    
                    let width = natural_width.max(min_w);
                    if let Some(max) = max_w {
                        width.min(max)
                    } else {
                        width
                    }
                }
                _ => {
                    // For subforms, fields, etc: use minW if available, else parent width
                    node.min_w.unwrap_or(parent_position.width)
                }
            }
        });
        
        // Get margins (these define spacing between the element's edges and its content)
        // NOTE: Must be extracted before height calculation since natural height includes margins
        let margin_top = node.margin_top.unwrap_or(Decimal::ZERO);
        let margin_bottom = node.margin_bottom.unwrap_or(Decimal::ZERO);
        let margin_left = node.margin_left.unwrap_or(Decimal::ZERO);
        let margin_right = node.margin_right.unwrap_or(Decimal::ZERO);
        
        // Height: use explicit h, or calculate natural height for leaf nodes
        let explicit_height = node.h;
        let min_height = node.min_h.unwrap_or(Decimal::ZERO);
        
        // For containers without explicit height, calculate natural height
        // NOTE: For draw/field elements, natural height is content + margins
        let height = explicit_height.unwrap_or_else(|| {
            // For leaf nodes (field/draw), calculate natural height based on content
            // The natural height must include space for margins + content
            match &node.kind {
                XfaNodeKind::Draw => {
                    // Calculate natural height for draw element based on text content
                    // Per XFA AXTE spec
                    // Use context to resolve xfa:embed references for accurate height
                    let natural_content_height = if let Some(text) = ctx.extract_text(&node.children) {
                        // Check if this is HTML content with multiple paragraphs
                        let paragraph_count = if Self::has_html_exdata(&node.children) {
                            Self::count_html_paragraphs(&node.children)
                        } else {
                            0
                        };
                        
                        Self::calculate_natural_text_height_with_paragraphs(
                            &text, 
                            &node.font, 
                            &node.para, 
                            width,
                            paragraph_count
                        )
                    } else {
                        // No text content, use default line height
                        num(12.0)
                    };
                    // Total height = content + margins
                    let total_height = natural_content_height + margin_top + margin_bottom;
                    total_height.max(min_height)
                }
                XfaNodeKind::Field => {
                    // For fields, calculate based on font size + margins
                    // Per XFA spec: natural height of text widget is height of text block
                    let font_size = node.font.as_ref()
                        .map(|f| f.size)
                        .unwrap_or_else(|| num(10.0));
                    // Line gap of 20% plus some padding
                    let content_height = font_size * num(1.4); // Font size + 20% line gap + padding
                    let total_height = content_height + margin_top + margin_bottom;
                    total_height.max(min_height)
                }
                XfaNodeKind::Element { tag_name, .. } => {
                    match tag_name.as_str() {
                        "draw" => {
                            // Calculate natural height for draw element
                            // Use context to resolve xfa:embed references for accurate height
                            let natural_content_height = if let Some(text) = ctx.extract_text(&node.children) {
                                // Check if this is HTML content with multiple paragraphs
                                let paragraph_count = if Self::has_html_exdata(&node.children) {
                                    Self::count_html_paragraphs(&node.children)
                                } else {
                                    0
                                };
                                
                                Self::calculate_natural_text_height_with_paragraphs(
                                    &text, 
                                    &node.font, 
                                    &node.para, 
                                    width,
                                    paragraph_count
                                )
                            } else {
                                num(12.0)
                            };
                            // Total height = content + margins
                            let total_height = natural_content_height + margin_top + margin_bottom;
                            total_height.max(min_height)
                        }
                        "field" => {
                            let font_size = node.font.as_ref()
                                .map(|f| f.size)
                                .unwrap_or_else(|| num(10.0));
                            let content_height = font_size * num(1.4);
                            let total_height = content_height + margin_top + margin_bottom;
                            total_height.max(min_height)
                        }
                        _ => {
                            // Containers: if min_height is set, use it; else 0 (children determine)
                            if min_height > Decimal::ZERO { min_height } else { Decimal::ZERO }
                        }
                    }
                }
                _ => {
                    // Other containers: if min_height is set, use it; else 0
                    if min_height > Decimal::ZERO { min_height } else { Decimal::ZERO }
                }
            }
        });
        
        // Get layout from node's layout attribute
        // Per XFA spec: if subform has no layout attribute, it defaults to "position"
        let layout = node.layout.as_ref()
            .map(|l| Layout::from_str(l))
            .unwrap_or(Layout::Position);
        
        // Get anchor type for positioning (default is topLeft)
        let anchor_type = node.attributes.get("anchorType")
            .map(|s| s.as_str())
            .unwrap_or("topLeft");
        
        // Compute absolute position based on parent layout strategy
        let outer_pos = match parent_layout {
            Layout::Position => {
                // Position layout: children specify their own positions relative to parent
                let (adj_x, adj_y) = Self::apply_anchor_type(x, y, width, height, anchor_type);
                Position::new(
                    parent_position.x + adj_x,
                    parent_position.y + adj_y,
                    width,
                    height.max(min_height),
                )
            }
            Layout::TopToBottom => {
                // TopToBottom (tb): Stack vertically, left-aligned
                // Per XFA spec (section 8): "In this type of layout the contained object's 
                // x and y properties, as well as its anchor point, are ignored."
                //
                // Elements are placed at top-left of container, then immediately below
                // the nominal extent of the previous object, aligned with left edge.
                
                let abs_x = parent_position.x;
                let abs_y = *current_y;
                
                let effective_height = height.max(min_height);
                let pos = Position::new(abs_x, abs_y, width, effective_height);
                
                // Advance flow position in tb layout
                *current_y = abs_y + effective_height;
                
                pos
            }
            Layout::LeftToRightTopToBottom | Layout::LeftToRight => {
                // Left-to-right top-to-bottom tiled layout (lr-tb)
                // Per XFA spec (section 8): "In this type of layout the contained object's 
                // x and y properties, as well as its anchor point, are ignored."
                //
                // Elements flow horizontally from left to right. When an element doesn't fit
                // in the remaining width, it wraps to the next line.
                
                // Check if we need to wrap to next line
                if *current_x + width > parent_position.x + parent_position.width && 
                   *current_x > parent_position.x {
                    // Wrap to next line
                    *current_x = parent_position.x;
                    *current_y += *max_height_in_row;
                    *max_height_in_row = Decimal::ZERO;
                }
                
                // Position at current flow position (x and y properties are ignored)
                let pos = Position::new(*current_x, *current_y, width, height);
                
                // Advance flow position
                *current_x += width;
                *max_height_in_row = (*max_height_in_row).max(height);
                
                pos
            }
            Layout::RightToLeftTopToBottom => {
                // Right-to-left top-to-bottom tiled layout (rl-tb)
                // Per XFA spec (section 8): "In this type of layout the contained object's 
                // x and y properties, as well as its anchor point, are ignored."
                //
                // Like lr-tb but objects flow from right to left instead of left to right.
                
                // Calculate position from right edge
                let right_edge = parent_position.x + parent_position.width;
                
                // Check if we need to wrap to next line
                if *current_x - width < parent_position.x && *current_x < right_edge {
                    // Wrap to next line
                    *current_x = right_edge;
                    *current_y += *max_height_in_row;
                    *max_height_in_row = Decimal::ZERO;
                }
                
                // Position from right, moving left
                let pos_x = *current_x - width;
                let pos = Position::new(pos_x, *current_y, width, height);
                *current_x = pos_x;
                *max_height_in_row = (*max_height_in_row).max(height);
                pos
            }
            Layout::Row => {
                // Row layout: similar to lr-tb but typically within a table row
                // Honor explicit coordinates if provided
                if has_explicit_x || has_explicit_y {
                    Position::new(
                        parent_position.x + x,
                        parent_position.y + y,
                        width,
                        height,
                    )
                } else {
                    let pos = Position::new(*current_x, *current_y, width, height);
                    *current_x += width;
                    *max_height_in_row = (*max_height_in_row).max(height);
                    pos
                }
            }
            Layout::RightToLeftRow => {
                // Right-to-left row layout
                // Honor explicit coordinates if provided
                if has_explicit_x || has_explicit_y {
                    Position::new(
                        parent_position.x + x,
                        parent_position.y + y,
                        width,
                        height,
                    )
                } else {
                    let pos_x = *current_x - width;
                    let pos = Position::new(pos_x.max(parent_position.x), *current_y, width, height);
                    *current_x = pos_x;
                    *max_height_in_row = (*max_height_in_row).max(height);
                    pos
                }
            }
            Layout::Table => {
                // Table layout: handled specially, for now treat as tb
                let pos = Position::new(parent_position.x, *current_y, width, height);
                *current_y += height;
                pos
            }
        };
        
        // Calculate content position (inset by margins)
        // This is where children will be placed
        let content_pos = Position::new(
            outer_pos.x + margin_left,
            outer_pos.y + margin_top,
            (outer_pos.width - margin_left - margin_right).max(Decimal::ZERO),
            (outer_pos.height - margin_top - margin_bottom).max(Decimal::ZERO),
        );
        
        // Return the height consumed by this node
        let consumed_height = outer_pos.height;
        Ok((outer_pos, content_pos, layout, consumed_height))
    }
    
    /// Apply anchor type adjustment to coordinates
    /// Per XFA spec: anchor point determines which point of the object's nominal extent
    /// is placed at the (x, y) coordinate
    fn apply_anchor_type(x: Num, y: Num, width: Num, height: Num, anchor_type: &str) -> (Num, Num) {
        let two = num(2.0);
        match anchor_type {
            "topLeft" => (x, y),
            "topCenter" => (x - width / two, y),
            "topRight" => (x - width, y),
            "middleLeft" => (x, y - height / two),
            "middleCenter" => (x - width / two, y - height / two),
            "middleRight" => (x - width, y - height / two),
            "bottomLeft" => (x, y - height),
            "bottomCenter" => (x - width / two, y - height),
            "bottomRight" => (x - width, y - height),
            _ => (x, y), // Default to topLeft
        }
    }
    
    /// Calculate the natural width for a text/draw element.
    /// Per XFA spec: when w is not specified, the element is horizontally growable
    /// and its width is determined by the content (natural width).
    /// The width is constrained by minW (minimum) and maxW (maximum) if specified.
    fn calculate_natural_text_width(text: &str, font: &Option<Font>) -> Num {
        // Get font size from style or use default
        let font_size = font.as_ref()
            .map(|f| f.size)
            .unwrap_or_else(|| num(10.0));
        
        let font_size_f32 = font_size.to_f32().unwrap_or(10.0);
        
        // Approximate character width as 60% of font size (rough estimate)
        // This is a simplified calculation; for accurate width we'd need actual font metrics
        let char_width = font_size_f32 * 0.6;
        
        // Calculate width of the text
        let text_width = text.chars().count() as f32 * char_width;
        
        // Add some padding for margins
        let padded_width = text_width + font_size_f32 * 0.5;
        
        Decimal::from_f32(padded_width).unwrap_or_else(|| num(100.0))
    }
    
    /// Calculate the natural height for a text/draw element based on AXTE rules.
    /// Per XFA spec (AXTE appendix):
    /// - Line gap is 20% of font size
    /// - Text height = ascent + descent (padded to at least font_size)
    /// - Full height = margin_top + derived_spacing + margin_bottom (with LG removed on last line)
    /// 
    /// This is used when no explicit height is specified for a draw element.
    fn calculate_natural_text_height(text: &str, font: &Option<Font>, para: &Option<Para>, max_width: Num) -> Num {
        // Get font size from style or use default
        let font_size = font.as_ref()
            .map(|f| f.size)
            .unwrap_or_else(|| num(10.0));
        
        let font_size_f32 = font_size.to_f32().unwrap_or(10.0);
        
        // Get line height from para, or calculate default (font_size + 20% line gap)
        let line_height = para.as_ref()
            .and_then(|p| p.line_height)
            .unwrap_or(font_size * num(1.2));
        let _line_height_f32 = line_height.to_f32().unwrap_or(font_size_f32 * 1.2);
        
        // Use a more accurate character width estimate based on typical font metrics
        // Average character width is typically 40-50% of font size for proportional fonts
        let char_width = font_size_f32 * 0.45;
        let max_width_f32 = max_width.to_f32().unwrap_or(1000.0);
        let chars_per_line = (max_width_f32 / char_width).max(1.0) as usize;
        
        // Count words and estimate lines more accurately
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut num_lines: usize = 1;
        let mut current_line_chars: usize = 0;
        
        for word in words {
            let word_chars = word.chars().count();
            if current_line_chars == 0 {
                current_line_chars = word_chars;
            } else if current_line_chars + 1 + word_chars <= chars_per_line {
                current_line_chars += 1 + word_chars;
            } else {
                num_lines += 1;
                current_line_chars = word_chars;
            }
        }
        
        if text.is_empty() {
            num_lines = 1;
        }
        
        // Add extra lines for paragraph breaks (empty lines in rich text)
        // Count paragraph separators that would add extra lines
        let paragraph_breaks = text.matches('\n').count() + text.matches('\u{2029}').count();
        num_lines += paragraph_breaks;
        
        // Paragraph margins
        let margin_top = para.as_ref()
            .and_then(|p| p.space_above)
            .unwrap_or(Decimal::ZERO);
        let margin_bottom = para.as_ref()
            .and_then(|p| p.space_below)
            .unwrap_or(Decimal::ZERO);
        
        // Calculate total height using line_height for all lines
        // Per AXTE: FH = MT + (num_lines * line_height) + MB
        // But last line doesn't need trailing gap, so subtract one line gap
        let line_gap = line_height - font_size;
        
        
        if num_lines == 0 {
            margin_top + font_size + margin_bottom
        } else if num_lines == 1 {
            // Single line: MT + line_height + MB - line_gap (no trailing gap)
            margin_top + font_size + margin_bottom
        } else {
            // Multiple lines: all lines use line_height, but last line has no trailing gap
            let lines_height = num(num_lines as f64) * line_height - line_gap;
            margin_top + lines_height + margin_bottom
        }
    }
    
    /// Calculate the natural height for a text/draw element with paragraph count.
    /// This version accounts for multiple HTML paragraphs that each add a line break.
    fn calculate_natural_text_height_with_paragraphs(
        text: &str, 
        font: &Option<Font>, 
        para: &Option<Para>, 
        max_width: Num,
        paragraph_count: usize
    ) -> Num {
        // Get font size from style or use default
        let font_size = font.as_ref()
            .map(|f| f.size)
            .unwrap_or_else(|| num(10.0));
        
        let font_size_f32 = font_size.to_f32().unwrap_or(10.0);
        
        // Get line height from para, or calculate default (font_size + 20% line gap)
        let line_height = para.as_ref()
            .and_then(|p| p.line_height)
            .unwrap_or(font_size * num(1.2));
        let _line_height_f32 = line_height.to_f32().unwrap_or(font_size_f32 * 1.2);
        
        // Use a more accurate character width estimate based on typical font metrics
        // Average character width is typically 40-50% of font size for proportional fonts
        let char_width = font_size_f32 * 0.45;
        let max_width_f32 = max_width.to_f32().unwrap_or(1000.0);
        let chars_per_line = (max_width_f32 / char_width).max(1.0) as usize;
        
        // Count words and estimate lines more accurately
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut num_lines: usize = 1;
        let mut current_line_chars: usize = 0;
        
        for word in words {
            let word_chars = word.chars().count();
            if current_line_chars == 0 {
                current_line_chars = word_chars;
            } else if current_line_chars + 1 + word_chars <= chars_per_line {
                current_line_chars += 1 + word_chars;
            } else {
                num_lines += 1;
                current_line_chars = word_chars;
            }
        }
        
        if text.is_empty() {
            num_lines = 1;
        }
        
        // Add extra lines for paragraph breaks from HTML <p> elements
        // Each paragraph after the first adds a line break
        if paragraph_count > 1 {
            num_lines += paragraph_count - 1;
        }
        
        // Also count inline paragraph breaks
        let inline_breaks = text.matches('\n').count() + text.matches('\u{2029}').count();
        num_lines += inline_breaks;
        
        // Paragraph margins
        let margin_top = para.as_ref()
            .and_then(|p| p.space_above)
            .unwrap_or(Decimal::ZERO);
        let margin_bottom = para.as_ref()
            .and_then(|p| p.space_below)
            .unwrap_or(Decimal::ZERO);
        
        // Calculate total height using line_height for all lines
        let line_gap = line_height - font_size;
        
        
        if num_lines == 0 {
            margin_top + font_size + margin_bottom
        } else if num_lines == 1 {
            margin_top + font_size + margin_bottom
        } else {
            // Multiple lines: all lines use line_height, but last line has no trailing gap
            let lines_height = num(num_lines as f64) * line_height - line_gap;
            margin_top + lines_height + margin_bottom
        }
    }
    
    fn parse_dimension(s: &str) -> Result<Num, String> {
        // Parse dimensions that might have units like "100pt", "2in", "50mm"
        let s = s.trim();
        
        // Conversion constants with full precision
        let pts_per_inch = Decimal::from_str("72").unwrap();
        let pts_per_mm = Decimal::from_str("2.834645669291339").unwrap();
        let pts_per_cm = Decimal::from_str("28.34645669291339").unwrap();
        
        if s.ends_with("pt") {
            Decimal::from_str(s[..s.len()-2].trim())
                .map_err(|e| format!("Failed to parse dimension: {}", e))
        } else if s.ends_with("in") {
            Decimal::from_str(s[..s.len()-2].trim())
                .map(|v| v * pts_per_inch)
                .map_err(|e| format!("Failed to parse dimension: {}", e))
        } else if s.ends_with("mm") {
            Decimal::from_str(s[..s.len()-2].trim())
                .map(|v| v * pts_per_mm)
                .map_err(|e| format!("Failed to parse dimension: {}", e))
        } else if s.ends_with("cm") {
            Decimal::from_str(s[..s.len()-2].trim())
                .map(|v| v * pts_per_cm)
                .map_err(|e| format!("Failed to parse dimension: {}", e))
        } else {
            // No unit, assume points or just a number
            Decimal::from_str(s)
                .map_err(|e| format!("Failed to parse dimension: {}", e))
        }
    }
    
    fn extract_field_value(children: &[XfaNode]) -> String {
        for child in children {
            if matches!(child.kind, XfaNodeKind::Value) {
                // Look for text content in value node's children
                for value_child in &child.children {
                    if let XfaNodeKind::Text { content } = &value_child.kind {
                        return content.clone();
                    }
                    if let XfaNodeKind::Element { text_content, .. } = &value_child.kind
                        && let Some(text) = text_content {
                            return text.clone();
                        }
                }
            }
        }
        String::new()
    }
    
    fn extract_text_content(children: &[XfaNode]) -> Option<String> {
        // Use empty context for backward compatibility
        Self::extract_text_content_with_embed(children, &HashMap::new(), &HashMap::new())
    }
    
    /// Extract text content with xfa:embed resolution support
    /// 
    /// Parameters:
    /// - `children`: The node's children to extract text from
    /// - `computed_values`: Map of field name -> computed value
    /// - `id_to_field`: Map of element ID -> field name for resolving embed URIs
    fn extract_text_content_with_embed(
        children: &[XfaNode],
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
    ) -> Option<String> {
        for child in children {
            // Check for XfaNodeKind::Value
            if matches!(child.kind, XfaNodeKind::Value)
                && let Some(text) = Self::extract_value_text_with_embed(&child.children, computed_values, id_to_field) {
                    return Some(text);
                }
            // Also check for Element with tag_name "value" (when parsed via parse_element_content)
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                && tag_name == "value"
                    && let Some(text) = Self::extract_value_text_with_embed(&child.children, computed_values, id_to_field) {
                        return Some(text);
                    }
            if let XfaNodeKind::Text { content } = &child.kind {
                return Some(content.clone());
            }
        }
        None
    }
    
    /// Extract text from value node's children (handles both text and exData with HTML)
    fn extract_value_text(children: &[XfaNode]) -> Option<String> {
        Self::extract_value_text_with_embed(children, &HashMap::new(), &HashMap::new())
    }
    
    /// Extract text from value node's children with xfa:embed resolution
    fn extract_value_text_with_embed(
        children: &[XfaNode],
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
    ) -> Option<String> {
        for value_child in children {
            if let XfaNodeKind::Text { content } = &value_child.kind {
                return Some(content.clone());
            }
            if let XfaNodeKind::Element { tag_name, text_content } = &value_child.kind {
                if tag_name == "text"
                    && let Some(text) = text_content {
                        return Some(text.clone());
                    }
                // Handle exData with HTML content - extract plain text from it
                if tag_name == "exData" {
                    // Try to extract text from HTML body with embed resolution
                    if let Some(plain_text) = Self::extract_text_from_exdata_with_embed(
                        &value_child.children, 
                        computed_values, 
                        id_to_field
                    ) {
                        return Some(plain_text);
                    }
                    // Fallback to text_content if available
                    if let Some(text) = text_content {
                        return Some(text.clone());
                    }
                }
            }
        }
        None
    }
    
    /// Extract plain text from exData HTML content
    fn extract_text_from_exdata(children: &[XfaNode]) -> Option<String> {
        Self::extract_text_from_exdata_with_embed(children, &HashMap::new(), &HashMap::new())
    }
    
    /// Extract plain text from exData HTML content with xfa:embed resolution
    fn extract_text_from_exdata_with_embed(
        children: &[XfaNode],
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
    ) -> Option<String> {
        let mut text_parts = Vec::new();
        Self::collect_text_recursive_with_embed(children, &mut text_parts, computed_values, id_to_field);
        if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        }
    }
    
    /// Recursively collect text content from nested elements
    fn collect_text_recursive(children: &[XfaNode], text_parts: &mut Vec<String>) {
        Self::collect_text_recursive_with_embed(children, text_parts, &HashMap::new(), &HashMap::new());
    }
    
    /// Recursively collect text content from nested elements with xfa:embed resolution
    fn collect_text_recursive_with_embed(
        children: &[XfaNode], 
        text_parts: &mut Vec<String>,
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
    ) {
        for child in children {
            match &child.kind {
                XfaNodeKind::Text { content } => {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        text_parts.push(trimmed.to_string());
                    }
                }
                XfaNodeKind::Element { tag_name, text_content } => {
                    // Check for xfa:embed attribute (span elements with embedded references)
                    if let Some(embed_ref) = child.attributes.get("xfa:embed") {
                        // Resolve the embedded reference
                        if let Some(resolved_text) = Self::resolve_embed_reference(embed_ref, computed_values, id_to_field) {
                            text_parts.push(resolved_text);
                            continue; // Don't recurse into embed spans - they're empty
                        }
                    }
                    
                    // Add text content if present
                    if let Some(text) = text_content {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            text_parts.push(trimmed.to_string());
                        }
                    }
                    // Add space/newline for paragraph breaks
                    if (tag_name == "p" || tag_name == "br")
                        && !text_parts.is_empty() {
                            text_parts.push(" ".to_string());
                        }
                    // Recurse into children
                    Self::collect_text_recursive_with_embed(&child.children, text_parts, computed_values, id_to_field);
                }
                _ => {
                    // Recurse into other node types
                    Self::collect_text_recursive_with_embed(&child.children, text_parts, computed_values, id_to_field);
                }
            }
        }
    }
    
    /// Resolve an xfa:embed reference to the actual field value
    /// 
    /// The embed reference can be:
    /// - A URI fragment like "#uuid:field_id" (xfa:embedType="uri")
    /// - A SOM expression like "FIELD_NAME" (xfa:embedType="som")
    /// 
    /// Values in computed_values are now stored BOTH by field ID (primary) and by name (fallback).
    /// This handles the case where multiple subforms have same-named children with different IDs.
    fn resolve_embed_reference(
        embed_ref: &str,
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
    ) -> Option<String> {
        // Handle URI reference (starts with #)
        if let Some(id) = embed_ref.strip_prefix('#') {
            // Remove the # prefix
            
            // FIRST: Try to look up the value directly by ID (preferred - handles multiple same-named fields)
            if let Some(value) = computed_values.get(id) {
                return Some(value.clone());
            }
            
            // SECOND: Look up the field name from the ID, then look up by name (fallback)
            if let Some(field_name) = id_to_field.get(id) {
                return computed_values.get(field_name).cloned();
            }
            
            return None;
        }
        
        // Handle SOM expression (no # prefix) - direct field name reference
        computed_values.get(embed_ref).cloned()
    }
    
    /// Render the flattened layout to an image file
    /// Pass 1: Draw actual content (text in black, field boxes)
    /// Pass 2: Overlay debug info in transparent red (names, outlines)
    /// 
    /// Per XFA spec, font rendering respects:
    /// - typeface: Font family name (default: Courier)
    /// - size: Font size in points (default: 10pt)
    /// - weight: normal or bold (default: normal)
    /// - posture: normal or italic (default: normal)
    pub fn render_to_image<P: AsRef<Path>>(&self, output_path: P, scale: f32) -> Result<(), String> {
        let img = self.render_to_image_buffer(scale)?;
        img.save(output_path.as_ref())
            .map_err(|e| format!("Failed to save image: {}", e))?;
        Ok(())
    }
    
    /// Render the flattened layout to an image buffer (for compositing)
    /// 
    /// Returns the rendered image without saving to disk. This is useful for
    /// compositing additional overlays (e.g., group annotations in Document).
    /// This includes both the actual content and red debug annotations.
    pub fn render_to_image_buffer(&self, scale: f32) -> Result<RgbaImage, String> {
        // Start with the plain rendering (PASS 1)
        let mut img = self.render_to_image_buffer_plain(scale)?;
        
        // Get the scale and dimensions for PASS 2
        let scale_dec = num(scale as f64);
        
        // Get a default fallback font for debug text
        let fallback_font = Self::load_fallback_font()?;
        
        // Colors for debug overlay
        let debug_red = Rgba([255u8, 0u8, 0u8, 180u8]); // More visible red for field names
        let debug_red_outline = Rgba([255u8, 0u8, 0u8, 20u8]);
        
        // ============================================
        // PASS 2: Draw debug overlay in red
        // ============================================
        for node in &self.nodes {
            // Handle rotation: for 90/270 degrees, we swap width/height and adjust position
            let (x, y, w, h) = Self::apply_rotation_to_bounds(
                node.x, node.y, node.width, node.height, 
                node.rotate, scale_dec
            );
            
            if x < 0 || y < 0 || w <= 0 || h <= 0 {
                continue;
            }
            
            // Draw debug outline with transparency (blend with existing pixels)
            Self::draw_transparent_rect(&mut img, x, y, w, h, debug_red_outline);
            
            // Draw debug name label
            let debug_name = match &node.kind {
                FlattenedNodeKind::Field { name, .. } => name.clone(),
                FlattenedNodeKind::Text { .. } => "Text".to_string(),
            };
            
            let debug_font_size = (8.0 * scale).max(6.0);
            let debug_scale = PxScale::from(debug_font_size);
            
            // Draw name in transparent red at top-left of box (using fallback font)
            draw_text_mut(
                &mut img,
                debug_red,
                x + 1,
                y + 1,
                debug_scale,
                &fallback_font,
                &debug_name,
            );
        }
        
        Ok(img)
    }
    
    /// Render the flattened layout to an image buffer without debug annotations (plain mode)
    /// 
    /// Returns the rendered image without red debug overlays, only showing the actual content.
    pub fn render_to_image_buffer_plain(&self, scale: f32) -> Result<RgbaImage, String> {
        // Scale dimensions for better resolution (e.g., scale=2.0 for 2x)
        let scale_dec = num(scale as f64);
        
        // Width is fixed to page width
        let img_width = (self.page.width * scale_dec).to_f32().unwrap_or(0.0) as u32;
        
        // Height adapts to actual content bounds (maximum y + height of all nodes)
        let actual_content_height = self.nodes.iter()
            .map(|node| node.y + node.height)
            .max()
            .unwrap_or(self.page.height);
        let img_height = (actual_content_height * scale_dec).to_f32().unwrap_or(0.0) as u32;
        
        // Create a white background image (RGBA for transparency support)
        let mut img: RgbaImage = ImageBuffer::from_pixel(img_width, img_height, Rgba([255u8, 255u8, 255u8, 255u8]));
        
        // Get the font manager for font resolution
        let font_manager = get_font_manager();
        
        // Get a default fallback font
        let fallback_font = Self::load_fallback_font()?;
        
        // Colors (RGBA - last value is alpha: 255=opaque, 0=transparent)
        let black = Rgba([0u8, 0u8, 0u8, 255u8]);
        let dark_gray = Rgba([80u8, 80u8, 80u8, 255u8]);
        let light_blue_fill = Rgba([200u8, 220u8, 255u8, 255u8]); // Light blue for field backgrounds
        
        // ============================================
        // Draw actual content (as in PDF) - no debug overlay
        // ============================================
        for node in &self.nodes {
            // Handle rotation: for 90/270 degrees, we swap width/height and adjust position
            // Per XFA spec: rotation is counter-clockwise about anchor point
            let (x, y, w, h) = Self::apply_rotation_to_bounds(
                node.x, node.y, node.width, node.height, 
                node.rotate, scale_dec
            );
            
            // Skip nodes outside the visible area or with invalid dimensions
            if x < 0 || y < 0 || w <= 0 || h <= 0 {
                continue;
            }
            
            // Draw fill background if present
            if let Some(border) = &node.style.border
                && let Some(fill) = &border.fill
                    && fill.presence != "hidden" && fill.presence != "inactive"
                        && let Some((r, g, b)) = fill.color {
                            Self::fill_rect(&mut img, x, y, w, h, Rgba([r, g, b, 255u8]));
                        }
            
            // Draw border if present and visible
            if let Some(border) = &node.style.border
                && border.is_visible() {
                    Self::draw_border(&mut img, x, y, w, h, border, scale);
                }
            
            match &node.kind {
                FlattenedNodeKind::Field { value, .. } => {
                    // Draw light blue fill for field background (no border)
                    Self::fill_rect(&mut img, x, y, w, h, light_blue_fill);
                    
                    // Only draw field VALUE (not name) in black if present
                    if !value.is_empty() {
                        // Get font style from node, or use XFA defaults
                        let xfa_font = node.style.font.clone().unwrap_or_default();
                        let font_size = xfa_font.size.to_f32().unwrap_or(10.0);
                        let scaled_font_size = (font_size * scale).max(8.0);
                        let text_scale = PxScale::from(scaled_font_size);
                        
                        // Get the appropriate font for this style (with fallback)
                        let render_font = {
                            let mut mgr = font_manager.lock().map_err(|e| format!("Lock error: {}", e))?;
                            mgr.get_font(&xfa_font).unwrap_or_else(|_| fallback_font.clone())
                        };
                        
                        // Get text color from style or use black
                        let text_color = node.style.font.as_ref()
                            .and_then(|f| f.color)
                            .map(|(r, g, b)| Rgba([r, g, b, 255u8]))
                            .unwrap_or(black);
                        
                        // Calculate content area inside border margins
                        let (content_x, content_y, content_w, content_h) = if let Some(border) = &node.style.border {
                            let ml = (border.margin_left.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32;
                            let mt = (border.margin_top.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32;
                            let mr = (border.margin_right.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32;
                            let mb = (border.margin_bottom.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32;
                            (x + ml, y + mt, (w - ml - mr).max(0), (h - mt - mb).max(0))
                        } else {
                            (x, y, w, h)
                        };
                        
                        // Apply text alignment from para using font metrics (within content area)
                        let text_x = Self::calculate_text_x(content_x, content_w, value, scaled_font_size, &node.style.para, &render_font);
                        let text_y = Self::calculate_text_y(content_y, content_h, scaled_font_size, &node.style.para, &render_font, 0, 1, scale);
                        
                        draw_text_mut(
                            &mut img,
                            text_color,
                            text_x,
                            text_y,
                            text_scale,
                            &render_font,
                            value,
                        );
                    }
                }
                FlattenedNodeKind::Text { content, font_size, rich_text, source_name: _, .. } => {
                    // Draw text content (draw elements/labels)
                    // Get font style from node, or use XFA defaults
                    let xfa_font = node.style.font.clone().unwrap_or_default();
                    // Use style font size if available, otherwise use the passed value
                    let effective_font_size = if node.style.font.is_some() {
                        xfa_font.size.to_f32().unwrap_or(10.0)
                    } else {
                        font_size.to_f32().unwrap_or(10.0)
                    };
                    let scaled_font_size = (effective_font_size * scale).max(8.0);
                    let text_scale = PxScale::from(scaled_font_size);
                    
                    // Get the appropriate font for this style (with fallback)
                    // Also get bold and italic variants for rich text rendering
                    let (render_font, normal_font, bold_font, italic_font, bold_italic_font) = {
                        let mut mgr = font_manager.lock().map_err(|e| format!("Lock error: {}", e))?;
                        
                        // Get font as specified in XFA (may be bold/italic)
                        let base = mgr.get_font(&xfa_font).unwrap_or_else(|_| fallback_font.clone());
                        
                        // Get normal weight variant (for rich text base)
                        let mut normal_xfa_font = xfa_font.clone();
                        normal_xfa_font.weight = crate::xfa::FontWeight::Normal;
                        normal_xfa_font.posture = crate::xfa::FontPosture::Normal;
                        let normal = mgr.get_font(&normal_xfa_font).ok();
                        
                        // Get bold variant
                        let mut bold_xfa_font = xfa_font.clone();
                        bold_xfa_font.weight = crate::xfa::FontWeight::Bold;
                        bold_xfa_font.posture = crate::xfa::FontPosture::Normal;
                        let bold = mgr.get_font(&bold_xfa_font).ok();
                        
                        // Get italic variant
                        let mut italic_xfa_font = xfa_font.clone();
                        italic_xfa_font.weight = crate::xfa::FontWeight::Normal;
                        italic_xfa_font.posture = crate::xfa::FontPosture::Italic;
                        let italic = mgr.get_font(&italic_xfa_font).ok();
                        
                        // Get bold italic variant
                        let mut bold_italic_xfa_font = xfa_font.clone();
                        bold_italic_xfa_font.weight = crate::xfa::FontWeight::Bold;
                        bold_italic_xfa_font.posture = crate::xfa::FontPosture::Italic;
                        let bold_italic = mgr.get_font(&bold_italic_xfa_font).ok();
                        
                        (base, normal, bold, italic, bold_italic)
                    };
                    
                    // Get text color from style or use dark gray
                    let text_color = node.style.font.as_ref()
                        .and_then(|f| f.color)
                        .map(|(r, g, b)| Rgba([r, g, b, 255u8]))
                        .unwrap_or(dark_gray);
                    
                    // Calculate content area inside border margins
                    let (content_x, content_y, content_w, content_h) = {
                        // Get border margins if present
                        let (ml, mt, mr, mb) = if let Some(border) = &node.style.border {
                            (
                                (border.margin_left.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32,
                                (border.margin_top.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32,
                                (border.margin_right.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32,
                                (border.margin_bottom.unwrap_or(Decimal::ZERO).to_f32().unwrap_or(0.0) * scale) as i32,
                            )
                        } else {
                            (0, 0, 0, 0)
                        };
                        
                        (x + ml, y + mt, (w - ml - mr).max(0), (h - mt - mb).max(scaled_font_size as i32))
                    };
                    
                    // Check if we have rich text (HTML content with paragraph structure)
                    let has_rich_content = rich_text.as_ref().is_some_and(|rt| {
                        rt.paragraphs.iter().any(|p| !p.is_empty && p.runs.iter().any(|r| !r.text.is_empty()))
                    });
                    
                    // Get letter spacing from XFA font (scaled to pixels)
                    let letter_spacing = xfa_font.letter_spacing
                        .map(|ls| ls.to_f32().unwrap_or(0.0) * scale)
                        .unwrap_or(0.0);
                    
                    if has_rich_content {
                        let rt = rich_text.as_ref().unwrap();
                        // For rich text, use normal weight font as base
                        let base_font = normal_font.as_ref().unwrap_or(&render_font);
                        
                        // Use XFA-compliant rich text rendering with glyph-by-glyph positioning
                        let rendered_lines = Self::layout_rich_text(
                            rt,
                            content_w as f32,
                            scaled_font_size,
                            base_font,
                            scale,
                            letter_spacing,
                        );
                        
                        Self::render_text_glyph_by_glyph(
                            &mut img,
                            &rendered_lines,
                            content_x,
                            content_y,
                            content_w,
                            content_h,
                            scaled_font_size,
                            base_font,
                            bold_font.as_ref(),
                            italic_font.as_ref(),
                            bold_italic_font.as_ref(),
                            text_color,
                            &node.style.para,
                            scale,
                            letter_spacing,
                        );
                    } else if !content.is_empty() {
                        // Fallback to simple text rendering for plain text content
                        let lines = Self::wrap_text_with_font(content, content_w as f32, scaled_font_size, &render_font);
                        let total_lines = lines.len();
                        
                        for (i, line) in lines.iter().enumerate() {
                            // Calculate x position based on alignment (within content area)
                            let line_x = Self::calculate_text_x(content_x, content_w, line, scaled_font_size, &node.style.para, &render_font);
                            
                            // Calculate y position using AXTE-compliant method (within content area)
                            let line_y = Self::calculate_text_y(content_y, content_h, scaled_font_size, &node.style.para, &render_font, i, total_lines, scale);
                            
                            if line_y >= 0 && line_y < img_height as i32 - scaled_font_size as i32 {
                                draw_text_mut(
                                    &mut img,
                                    text_color,
                                    line_x,
                                    line_y,
                                    text_scale,
                                    &render_font,
                                    line,
                                );
                            }
                        }
                    }
                }
            }
        }
        
        Ok(img)
    }
    
    /// Draw border with proper edge styling
    fn draw_border(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, border: &Border, scale: f32) {
        let img_width = img.width() as i32;
        let img_height = img.height() as i32;
        
        // Get edges (0=top, 1=right, 2=bottom, 3=left)
        // Per XFA spec: if fewer than 4 edges, reuse the last one
        for edge_idx in 0..4 {
            if let Some(edge) = border.get_edge(edge_idx) {
                // Skip hidden edges
                if edge.presence == "hidden" || edge.presence == "inactive" {
                    continue;
                }
                
                // Get thickness in pixels (scaled)
                let thickness = edge.thickness.map(|t| t.to_f32().unwrap_or(1.0)).unwrap_or(1.0) * scale;
                let thickness_px = (thickness as i32).max(1);
                
                // Get color (default black)
                let color = edge.color
                    .map(|(r, g, b)| Rgba([r, g, b, 255u8]))
                    .unwrap_or(Rgba([0u8, 0u8, 0u8, 255u8]));
                
                // Draw based on stroke style
                match edge.stroke {
                    StrokeStyle::Solid => {
                        Self::draw_edge_solid(img, x, y, w, h, edge_idx, thickness_px, color, img_width, img_height);
                    }
                    StrokeStyle::Dashed => {
                        Self::draw_edge_dashed(img, x, y, w, h, edge_idx, thickness_px, color, img_width, img_height, 6);
                    }
                    StrokeStyle::Dotted => {
                        Self::draw_edge_dashed(img, x, y, w, h, edge_idx, thickness_px, color, img_width, img_height, 2);
                    }
                    StrokeStyle::Lowered | StrokeStyle::Raised | StrokeStyle::Etched | StrokeStyle::Embossed => {
                        // 3D effects - draw with two colors for highlight/shadow
                        let (light, dark) = if matches!(edge.stroke, StrokeStyle::Raised | StrokeStyle::Embossed) {
                            (Rgba([255u8, 255u8, 255u8, 255u8]), Rgba([128u8, 128u8, 128u8, 255u8]))
                        } else {
                            (Rgba([128u8, 128u8, 128u8, 255u8]), Rgba([255u8, 255u8, 255u8, 255u8]))
                        };
                        // Top and left get one color, bottom and right get the other
                        let edge_color = if edge_idx == 0 || edge_idx == 3 { light } else { dark };
                        Self::draw_edge_solid(img, x, y, w, h, edge_idx, thickness_px, edge_color, img_width, img_height);
                    }
                    _ => {
                        Self::draw_edge_solid(img, x, y, w, h, edge_idx, thickness_px, color, img_width, img_height);
                    }
                }
            }
        }
    }
    
    /// Draw a solid edge
    fn draw_edge_solid(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, edge_idx: usize, thickness: i32, color: Rgba<u8>, img_width: i32, img_height: i32) {
        match edge_idx {
            0 => { // Top edge
                for t in 0..thickness {
                    for dx in 0..w {
                        let px = x + dx;
                        let py = y + t;
                        if px >= 0 && px < img_width && py >= 0 && py < img_height {
                            img.put_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            }
            1 => { // Right edge
                for t in 0..thickness {
                    for dy in 0..h {
                        let px = x + w - 1 - t;
                        let py = y + dy;
                        if px >= 0 && px < img_width && py >= 0 && py < img_height {
                            img.put_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            }
            2 => { // Bottom edge
                for t in 0..thickness {
                    for dx in 0..w {
                        let px = x + dx;
                        let py = y + h - 1 - t;
                        if px >= 0 && px < img_width && py >= 0 && py < img_height {
                            img.put_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            }
            3 => { // Left edge
                for t in 0..thickness {
                    for dy in 0..h {
                        let px = x + t;
                        let py = y + dy;
                        if px >= 0 && px < img_width && py >= 0 && py < img_height {
                            img.put_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    
    /// Draw a dashed edge
    fn draw_edge_dashed(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, edge_idx: usize, thickness: i32, color: Rgba<u8>, img_width: i32, img_height: i32, dash_len: i32) {
        match edge_idx {
            0 => { // Top edge
                for t in 0..thickness {
                    for dx in 0..w {
                        if (dx / dash_len) % 2 == 0 {
                            let px = x + dx;
                            let py = y + t;
                            if px >= 0 && px < img_width && py >= 0 && py < img_height {
                                img.put_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                }
            }
            1 => { // Right edge
                for t in 0..thickness {
                    for dy in 0..h {
                        if (dy / dash_len) % 2 == 0 {
                            let px = x + w - 1 - t;
                            let py = y + dy;
                            if px >= 0 && px < img_width && py >= 0 && py < img_height {
                                img.put_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                }
            }
            2 => { // Bottom edge
                for t in 0..thickness {
                    for dx in 0..w {
                        if (dx / dash_len) % 2 == 0 {
                            let px = x + dx;
                            let py = y + h - 1 - t;
                            if px >= 0 && px < img_width && py >= 0 && py < img_height {
                                img.put_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                }
            }
            3 => { // Left edge
                for t in 0..thickness {
                    for dy in 0..h {
                        if (dy / dash_len) % 2 == 0 {
                            let px = x + t;
                            let py = y + dy;
                            if px >= 0 && px < img_width && py >= 0 && py < img_height {
                                img.put_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    
    /// Apply rotation to bounds, returning (x, y, w, h) in screen coordinates
    /// Per XFA spec: rotation is counter-clockwise about the anchor point (default topLeft)
    /// Angles are multiples of 90 degrees
    fn apply_rotation_to_bounds(
        node_x: Num, 
        node_y: Num, 
        node_width: Num, 
        node_height: Num,
        rotate: i32,
        scale: Num,
    ) -> (i32, i32, i32, i32) {
        let x = (node_x * scale).to_f32().unwrap_or(0.0);
        let y = (node_y * scale).to_f32().unwrap_or(0.0);
        let w = (node_width * scale).to_f32().unwrap_or(0.0);
        let h = (node_height * scale).to_f32().unwrap_or(0.0);
        
        // Normalize rotation to 0, 90, 180, 270
        let rot = rotate.rem_euclid(360);
        
        match rot {
            0 => (x as i32, y as i32, w as i32, h as i32),
            90 => {
                // 90 degrees counter-clockwise: content rotates, anchor stays at top-left
                // The bounding box on screen has swapped dimensions
                // Original top-left (x,y) becomes screen position, but box extends differently
                // For 90° CCW rotation around top-left: the rendered box goes UP and RIGHT from anchor
                // Screen coords: new_x = x, new_y = y - w (box extends upward by original width)
                // But we'll just render it as a vertical rectangle starting at the same position
                // Actually: the XFA spec says rotation about anchor point
                // For topLeft anchor at (x,y), 90° CCW means:
                //   - Original bottom-left becomes top-left
                //   - The rectangle that was horizontal becomes vertical
                // Screen position adjustment: the rotated box effectively starts at (x, y-w) with size (h, w)
                // But since we can't rotate text in imageproc easily, we'll just draw a vertical box
                (x as i32, (y - w) as i32, h as i32, w as i32)
            }
            180 => {
                // 180 degrees: box is mirrored around anchor
                // Screen position: (x-w, y-h) with same dimensions
                ((x - w) as i32, (y - h) as i32, w as i32, h as i32)
            }
            270 => {
                // 270 degrees CCW (same as 90 CW): 
                // Screen position: (x-h, y) with swapped dimensions
                ((x - h) as i32, y as i32, h as i32, w as i32)
            }
            _ => (x as i32, y as i32, w as i32, h as i32),
        }
    }
    
    /// Fill a rectangle with a solid color
    pub fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, color: Rgba<u8>) {
        let img_width = img.width() as i32;
        let img_height = img.height() as i32;
        
        for dy in 0..h {
            for dx in 0..w {
                let px = x + dx;
                let py = y + dy;
                if px >= 0 && px < img_width && py >= 0 && py < img_height {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
    
    /// Calculate text X position based on horizontal alignment
    /// Uses actual font metrics for accurate text width measurement
    fn calculate_text_x(box_x: i32, box_w: i32, text: &str, font_size: f32, para: &Option<Para>, font: &FontRef<'_>) -> i32 {
        let h_align = para.as_ref().map(|p| p.h_align).unwrap_or(HAlign::Left);
        
        // Measure actual text width using font metrics
        let scale = PxScale::from(font_size);
        let scaled_font = font.as_scaled(scale);
        let mut text_width: f32 = 0.0;
        for ch in text.chars() {
            let glyph_id = font.glyph_id(ch);
            if glyph_id.0 != 0 {
                text_width += scaled_font.h_advance(glyph_id);
            } else {
                // Fallback for missing glyphs
                text_width += font_size * 0.6;
            }
        }
        let text_width = text_width as i32;
        
        let margin_left = para.as_ref().and_then(|p| p.margin_left).map(|m| m.to_f32().unwrap_or(0.0) as i32).unwrap_or(0);
        let margin_right = para.as_ref().and_then(|p| p.margin_right).map(|m| m.to_f32().unwrap_or(0.0) as i32).unwrap_or(0);
        
        match h_align {
            HAlign::Left | HAlign::Justify | HAlign::JustifyAll => box_x + margin_left + 2,
            HAlign::Center => box_x + (box_w - text_width) / 2,
            HAlign::Right => box_x + box_w - text_width - margin_right - 2,
            HAlign::Radix => box_x + box_w / 2, // Simplified: center for radix
        }
    }
    
    /// Calculate text Y position based on vertical alignment using AXTE rules
    /// Per AXTE spec:
    /// - Baseline position: B = MT + TH - D
    /// - Text is drawn from baseline, so we need to position at baseline - ascent
    /// - Line gap is 20% of font size
    /// 
    /// The `render_scale` parameter converts points to pixels (e.g., 2.0 for 2x resolution).
    /// This is needed because `para` values (lineHeight, spaceAbove) are in points,
    /// while `font_size` is already in scaled pixels.
    fn calculate_text_y(box_y: i32, box_h: i32, font_size: f32, para: &Option<Para>, font: &FontRef<'_>, line_index: usize, total_lines: usize, render_scale: f32) -> i32 {
        let v_align = para.as_ref().map(|p| p.v_align).unwrap_or(VAlign::Top);
        // Scale paragraph values from points to pixels
        let space_above = para.as_ref().and_then(|p| p.space_above).map(|s| s.to_f32().unwrap_or(0.0) * render_scale).unwrap_or(0.0);
        let line_height_override = para.as_ref().and_then(|p| p.line_height).map(|lh| lh.to_f32().unwrap_or(0.0) * render_scale);
        
        // Get font metrics (for glyph scaling, not render scaling)
        let scale = PxScale::from(font_size);
        let scaled_font = font.as_scaled(scale);
        let ascent = scaled_font.ascent();
        let descent = scaled_font.descent().abs();
        
        // Per AXTE: ensure ascent + descent >= font_size
        let mut effective_ascent = ascent;
        if ascent + descent < font_size {
            effective_ascent = font_size - descent;
        }
        
        // Per AXTE: line gap is 20% of font size
        let line_gap = font_size * 0.2;
        
        // Text height: TH = A + D
        let text_height = effective_ascent + descent;
        
        // Derived line spacing: DS = TH + LG (unless overridden)
        let line_spacing = line_height_override.unwrap_or(text_height + line_gap);
        
        // Full height calculation for all lines
        let total_text_height = if total_lines == 1 {
            // Single line: no line gap at end
            text_height
        } else {
            // Multiple lines: full line spacing for all but last line
            let _is_last_line = line_index == total_lines - 1;
            let full_height_per_line = line_spacing;
            let last_line_height = text_height; // No line gap on last line
            
            (total_lines - 1) as f32 * full_height_per_line + last_line_height
        };
        
        // Calculate first line offset based on vertical alignment
        // Per AXTE: if total height > block height, treat as top-aligned
        let first_line_offset = if total_text_height > box_h as f32 {
            0.0
        } else {
            match v_align {
                VAlign::Top => 0.0,
                VAlign::Middle => (box_h as f32 - total_text_height) / 2.0,
                VAlign::Bottom => box_h as f32 - total_text_height,
            }
        };
        
        // Position for this specific line
        // Baseline position from top of text block: B = TH - D (for first line)
        // Y for drawing = block_y + first_line_offset + space_above + baseline - ascent
        // Since draw_text_mut positions at top-left of text, we use the top of the line
        let line_y = box_y as f32 + first_line_offset + space_above + (line_index as f32 * line_spacing);
        
        line_y as i32
    }
    
    /// Calculate text Y position (simple version for backward compatibility)
    fn calculate_text_y_simple(box_y: i32, box_h: i32, font_size: f32, para: &Option<Para>) -> i32 {
        let v_align = para.as_ref().map(|p| p.v_align).unwrap_or(VAlign::Top);
        let space_above = para.as_ref().and_then(|p| p.space_above).map(|s| s.to_f32().unwrap_or(0.0) as i32).unwrap_or(0);
        
        match v_align {
            VAlign::Top => box_y + space_above + 2,
            VAlign::Middle => box_y + (box_h - font_size as i32) / 2,
            VAlign::Bottom => box_y + box_h - font_size as i32 - 2,
        }
    }
    
    /// Text wrapping using actual font metrics for accurate width measurement
    fn wrap_text_with_font(text: &str, max_width: f32, font_size: f32, font: &FontRef<'_>) -> Vec<String> {
        if max_width <= 0.0 {
            return vec![text.to_string()];
        }
        
        let scale = PxScale::from(font_size);
        let scaled_font = font.as_scaled(scale);
        
        // Get space width
        let space_glyph = font.glyph_id(' ');
        let space_width = if space_glyph.0 != 0 {
            scaled_font.h_advance(space_glyph)
        } else {
            font_size * 0.3
        };
        
        let mut lines = Vec::new();
        let mut current_line = String::new();
        let mut current_width: f32 = 0.0;
        
        for word in text.split_whitespace() {
            // Measure word width
            let mut word_width: f32 = 0.0;
            for ch in word.chars() {
                let glyph_id = font.glyph_id(ch);
                if glyph_id.0 != 0 {
                    word_width += scaled_font.h_advance(glyph_id);
                } else {
                    word_width += font_size * 0.6;
                }
            }
            
            if current_line.is_empty() {
                // First word on line
                current_line = word.to_string();
                current_width = word_width;
            } else if current_width + space_width + word_width <= max_width {
                // Word fits on current line
                current_line.push(' ');
                current_line.push_str(word);
                current_width += space_width + word_width;
            } else {
                // Word doesn't fit, start new line
                lines.push(current_line);
                current_line = word.to_string();
                current_width = word_width;
            }
        }
        
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        
        if lines.is_empty() {
            lines.push(String::new());
        }
        
        lines
    }
    
    /// Simple text wrapping (fallback without font)
    fn wrap_text(text: &str, max_width: usize, font_size: f32) -> Vec<String> {
        let char_width = (font_size * 0.6) as usize; // Approximate character width
        if char_width == 0 {
            return vec![text.to_string()];
        }
        
        let chars_per_line = (max_width / char_width).max(1);
        let mut lines = Vec::new();
        let mut current_line = String::new();
        
        for word in text.split_whitespace() {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() <= chars_per_line {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
        }
        
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        
        if lines.is_empty() {
            lines.push(String::new());
        }
        
        lines
    }
    
    /// Calculate the total text block height using AXTE rules
    /// Per AXTE: FH = MT + DS + MB, with LG removed on last line
    pub fn calculate_text_block_height(text: &str, font_size: f32, max_width: f32, para: &Option<Para>, font: &FontRef<'_>) -> f32 {
        let lines = Self::wrap_text_with_font(text, max_width, font_size, font);
        let num_lines = lines.len();
        
        if num_lines == 0 {
            return 0.0;
        }
        
        // Get font metrics
        let scale = PxScale::from(font_size);
        let scaled_font = font.as_scaled(scale);
        let ascent = scaled_font.ascent();
        let descent = scaled_font.descent().abs();
        
        // Per AXTE: ensure ascent + descent >= font_size
        let mut effective_ascent = ascent;
        if ascent + descent < font_size {
            effective_ascent = font_size - descent;
        }
        
        // Per AXTE: line gap is 20% of font size
        let line_gap = font_size * 0.2;
        
        // Text height: TH = A + D
        let text_height = effective_ascent + descent;
        
        // Line spacing override from para element
        let line_spacing = para.as_ref()
            .and_then(|p| p.line_height)
            .map(|lh| lh.to_f32().unwrap_or(0.0))
            .unwrap_or(text_height + line_gap);
        
        // Paragraph margins
        let margin_top = para.as_ref()
            .and_then(|p| p.space_above)
            .map(|s| s.to_f32().unwrap_or(0.0))
            .unwrap_or(0.0);
        let margin_bottom = para.as_ref()
            .and_then(|p| p.space_below)
            .map(|s| s.to_f32().unwrap_or(0.0))
            .unwrap_or(0.0);
        
        // Calculate total height
        // Per AXTE: FH = MT + DS + MB for each line, but LG removed on last line
        if num_lines == 1 {
            // Single line: MT + TH + MB (no line gap)
            margin_top + text_height + margin_bottom
        } else {
            // Multiple lines:
            // - First line: MT + DS
            // - Middle lines: DS each
            // - Last line: TH + MB (no line gap)
            let first_line = margin_top + line_spacing;
            let middle_lines = (num_lines - 2).max(0) as f32 * line_spacing;
            let last_line = text_height + margin_bottom;
            
            first_line + middle_lines + last_line
        }
    }
    
    /// Draw a transparent hollow rectangle by blending with existing pixels
    pub fn draw_transparent_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, color: Rgba<u8>) {
        let img_width = img.width() as i32;
        let img_height = img.height() as i32;
        
        // Helper to blend a pixel with transparency
        let blend_pixel = |img: &mut RgbaImage, px: i32, py: i32, color: Rgba<u8>| {
            if px >= 0 && px < img_width && py >= 0 && py < img_height {
                let existing = img.get_pixel(px as u32, py as u32);
                let alpha = color[3] as f32 / 255.0;
                let inv_alpha = 1.0 - alpha;
                
                let blended = Rgba([
                    (color[0] as f32 * alpha + existing[0] as f32 * inv_alpha) as u8,
                    (color[1] as f32 * alpha + existing[1] as f32 * inv_alpha) as u8,
                    (color[2] as f32 * alpha + existing[2] as f32 * inv_alpha) as u8,
                    255u8, // Keep full opacity for the result
                ]);
                img.put_pixel(px as u32, py as u32, blended);
            }
        };
        
        // Draw top and bottom edges
        for dx in 0..w {
            blend_pixel(img, x + dx, y, color);           // Top edge
            blend_pixel(img, x + dx, y + h - 1, color);   // Bottom edge
        }
        
        // Draw left and right edges
        for dy in 0..h {
            blend_pixel(img, x, y + dy, color);           // Left edge
            blend_pixel(img, x + w - 1, y + dy, color);   // Right edge
        }
    }
    
    /// Load a fallback font for rendering when specific fonts aren't available
    /// This uses the font_manager's fallback mechanism
    pub fn load_fallback_font() -> Result<FontRef<'static>, String> {
        let manager = get_font_manager();
        let mut manager = manager.lock().map_err(|e| format!("Lock error: {}", e))?;
        manager.get_default_font().map_err(|e| e.to_string())
    }

    // ========================================================================
    // XFA-Compliant Rich Text Parsing
    // ========================================================================

    /// Parse HTML content from exData into a RichText structure.
    /// This handles:
    /// - Paragraph elements (<p>) with inline styles (text-indent, font-weight, etc.)
    /// - Span elements with xfa-spacerun:yes for preserved spaces
    /// - Non-breaking spaces (U+00A0, &#160;) 
    /// - Paragraph separators (U+2029)
    /// 
    /// The `default_h_align` is used when CSS doesn't specify text-align (inherited from XFA para element)
    pub fn parse_rich_text_from_html(children: &[XfaNode], default_h_align: HAlign) -> RichText {
        let mut paragraphs = Vec::new();
        Self::parse_html_nodes_to_rich_text(children, &mut paragraphs, false, false, false, default_h_align);
        
        // If no paragraphs were created but we have content, create a single paragraph
        if paragraphs.is_empty() {
            paragraphs.push(RichParagraph {
                h_align: default_h_align,
                ..RichParagraph::default()
            });
        }
        
        RichText { paragraphs }
    }

    /// Recursively parse HTML nodes into rich text paragraphs
    fn parse_html_nodes_to_rich_text(
        children: &[XfaNode],
        paragraphs: &mut Vec<RichParagraph>,
        preserve_spaces: bool,
        bold: bool,
        italic: bool,
        default_h_align: HAlign,
    ) {
        for child in children {
            match &child.kind {
                XfaNodeKind::Text { content } => {
                    // Handle text content - check for paragraph separators (U+2029)
                    // which should create new paragraphs
                    let segments: Vec<&str> = content.split('\u{2029}').collect();
                    
                    for (seg_idx, segment) in segments.iter().enumerate() {
                        // If not the first segment, create a new paragraph for each U+2029
                        if seg_idx > 0 {
                            paragraphs.push(RichParagraph {
                                h_align: default_h_align,
                                ..RichParagraph::default()
                            });
                        }
                        
                        let text = if preserve_spaces {
                            // xfa-spacerun: preserve all whitespace, convert NBSP to space
                            segment.replace('\u{00A0}', " ")
                        } else {
                            // Normal mode: collapse whitespace but preserve structure
                            Self::normalize_whitespace(segment)
                        };
                        
                        if !text.is_empty() || preserve_spaces {
                            // Ensure we have a paragraph
                            if paragraphs.is_empty() {
                                paragraphs.push(RichParagraph {
                                    h_align: default_h_align,
                                    ..RichParagraph::default()
                                });
                            }
                            
                            let para = paragraphs.last_mut().unwrap();
                            para.runs.push(RichRun {
                                text,
                                preserve_spaces,
                                bold,
                            italic,
                            underline: false,
                        });
                    }
                }
                }
                XfaNodeKind::Element { tag_name, text_content } => {
                    let tag_lower = tag_name.to_lowercase();
                    
                    match tag_lower.as_str() {
                        "body" => {
                            // Body element - recurse into children
                            Self::parse_html_nodes_to_rich_text(&child.children, paragraphs, preserve_spaces, bold, italic, default_h_align);
                        }
                        "p" => {
                            // Paragraph element - create new paragraph
                            let mut para = RichParagraph {
                                h_align: default_h_align,  // Use XFA default if CSS doesn't override
                                ..RichParagraph::default()
                            };
                            
                            // Parse paragraph styles from style attribute
                            let para_bold = if let Some(style) = child.attributes.get("style") {
                                para.text_indent = Self::parse_css_dimension(style, "text-indent");
                                // Only override h_align if CSS specifies it
                                let css_align = Self::parse_css_alignment_optional(style);
                                if let Some(align) = css_align {
                                    para.h_align = align;
                                }
                                
                                // Check for font-weight:bold in paragraph style
                                style.contains("font-weight:bold") || style.contains("font-weight: bold")
                            } else {
                                false
                            };
                            
                            // Add paragraph to list
                            paragraphs.push(para);
                            
                            // First, handle direct text_content of the <p> element
                            // Use helper that handles U+2029 paragraph separators
                            if let Some(text) = text_content {
                                Self::add_text_with_paragraph_splits(
                                    text,
                                    paragraphs,
                                    preserve_spaces,
                                    bold || para_bold,
                                    italic,
                                    default_h_align,
                                );
                            }
                            
                            // Then parse children with inherited styles
                            Self::parse_html_nodes_to_rich_text(
                                &child.children, 
                                paragraphs, 
                                preserve_spaces, 
                                bold || para_bold, 
                                italic,
                                default_h_align
                            );
                            
                            // Check if paragraph ended up empty (only whitespace spans)
                            if let Some(last_para) = paragraphs.last_mut()
                                && (last_para.runs.is_empty() || 
                                   last_para.runs.iter().all(|r| r.text.trim().is_empty())) {
                                    last_para.is_empty = true;
                                }
                        }
                        "span" => {
                            // Check for xfa-spacerun:yes style
                            let new_preserve = if let Some(style) = child.attributes.get("style") {
                                style.contains("xfa-spacerun:yes") || style.contains("xfa-spacerun: yes")
                            } else {
                                preserve_spaces
                            };
                            
                            // Handle text_content if present
                            // Handle text_content with U+2029 support
                            if let Some(text) = text_content {
                                if new_preserve {
                                    // For xfa-spacerun, count spaces but still handle U+2029
                                    let segments: Vec<&str> = text.split('\u{2029}').collect();
                                    for (seg_idx, segment) in segments.iter().enumerate() {
                                        if seg_idx > 0 {
                                            paragraphs.push(RichParagraph {
                                                h_align: default_h_align,
                                                ..RichParagraph::default()
                                            });
                                        }
                                        let space_count = segment.chars()
                                            .filter(|c| *c == ' ' || *c == '\u{00A0}')
                                            .count();
                                        if space_count > 0 {
                                            if paragraphs.is_empty() {
                                                paragraphs.push(RichParagraph {
                                                    h_align: default_h_align,
                                                    ..RichParagraph::default()
                                                });
                                            }
                                            paragraphs.last_mut().unwrap().runs.push(RichRun {
                                                text: " ".repeat(space_count),
                                                preserve_spaces: true,
                                                bold,
                                                italic,
                                                underline: false,
                                            });
                                        }
                                    }
                                } else {
                                    Self::add_text_with_paragraph_splits(
                                        text, paragraphs, false, bold, italic, default_h_align
                                    );
                                }
                            }
                            
                            // Recurse into span children
                            Self::parse_html_nodes_to_rich_text(&child.children, paragraphs, new_preserve, bold, italic, default_h_align);
                        }
                        "b" | "strong" => {
                            // Bold text - handle U+2029 paragraph separators
                            if let Some(text) = text_content {
                                Self::add_text_with_paragraph_splits(
                                    text, paragraphs, preserve_spaces, true, italic, default_h_align
                                );
                            }
                            Self::parse_html_nodes_to_rich_text(&child.children, paragraphs, preserve_spaces, true, italic, default_h_align);
                        }
                        "i" | "em" => {
                            // Italic text - handle U+2029 paragraph separators
                            if let Some(text) = text_content {
                                Self::add_text_with_paragraph_splits(
                                    text, paragraphs, preserve_spaces, bold, true, default_h_align
                                );
                            }
                            Self::parse_html_nodes_to_rich_text(&child.children, paragraphs, preserve_spaces, bold, true, default_h_align);
                        }
                        "br" => {
                            // Line break - start a new paragraph
                            paragraphs.push(RichParagraph {
                                h_align: default_h_align,
                                is_empty: true,
                                ..Default::default()
                            });
                        }
                        _ => {
                            // Unknown element - handle U+2029 paragraph separators
                            if let Some(text) = text_content {
                                Self::add_text_with_paragraph_splits(
                                    text, paragraphs, preserve_spaces, bold, italic, default_h_align
                                );
                            }
                            Self::parse_html_nodes_to_rich_text(&child.children, paragraphs, preserve_spaces, bold, italic, default_h_align);
                        }
                    }
                }
                _ => {
                    // Other node types - recurse into children
                    Self::parse_html_nodes_to_rich_text(&child.children, paragraphs, preserve_spaces, bold, italic, default_h_align);
                }
            }
        }
    }

    /// Helper to add text content to paragraphs, handling U+2029 paragraph separators.
    /// This splits text on U+2029 and creates new paragraphs as needed.
    fn add_text_with_paragraph_splits(
        text: &str,
        paragraphs: &mut Vec<RichParagraph>,
        preserve_spaces: bool,
        bold: bool,
        italic: bool,
        default_h_align: HAlign,
    ) {
        // Split on U+2029 paragraph separator
        let segments: Vec<&str> = text.split('\u{2029}').collect();
        
        for (seg_idx, segment) in segments.iter().enumerate() {
            // If not the first segment, create a new paragraph for each U+2029
            if seg_idx > 0 {
                paragraphs.push(RichParagraph {
                    h_align: default_h_align,
                    ..RichParagraph::default()
                });
            }
            
            let processed = if preserve_spaces {
                // xfa-spacerun: preserve all whitespace, convert NBSP to space
                segment.replace('\u{00A0}', " ")
            } else {
                // Normal mode: collapse whitespace but preserve structure
                Self::normalize_whitespace(segment)
            };
            
            if !processed.is_empty() || preserve_spaces {
                // Ensure we have a paragraph
                if paragraphs.is_empty() {
                    paragraphs.push(RichParagraph {
                        h_align: default_h_align,
                        ..RichParagraph::default()
                    });
                }
                
                paragraphs.last_mut().unwrap().runs.push(RichRun {
                    text: processed,
                    preserve_spaces,
                    bold,
                    italic,
                    underline: false,
                });
            }
        }
    }

    /// Normalize whitespace in text content per XFA/HTML rules.
    /// Collapses consecutive whitespace to single space, handles special chars.
    /// Note: U+2029 is NOT handled here - it should be split before calling this.
    fn normalize_whitespace(text: &str) -> String {
        let mut result = String::new();
        let mut last_was_space = true; // Start true to trim leading
        
        for ch in text.chars() {
            match ch {
                // Paragraph separator - treat as paragraph break marker
                '\u{2029}' => {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                }
                // Non-breaking space - convert to regular space but keep
                '\u{00A0}' => {
                    result.push(' ');
                    last_was_space = true;
                }
                // Regular whitespace
                ' ' | '\t' | '\n' | '\r' => {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                }
                // Regular character
                _ => {
                    result.push(ch);
                    last_was_space = false;
                }
            }
        }
        
        // Trim trailing space
        if result.ends_with(' ') {
            result.pop();
        }
        
        result
    }

    /// Parse a CSS dimension value from a style string.
    /// Looks for "property: Xpt" or "property: Xin" etc.
    fn parse_css_dimension(style: &str, property: &str) -> Option<f32> {
        let search = format!("{}:", property);
        if let Some(pos) = style.find(&search) {
            let rest = &style[pos + search.len()..];
            let value_str = rest.split(';').next()?.trim();
            
            // Parse the dimension with unit
            if value_str.ends_with("pt") {
                value_str[..value_str.len()-2].trim().parse().ok()
            } else if value_str.ends_with("in") {
                value_str[..value_str.len()-2].trim().parse::<f32>().ok().map(|v| v * 72.0)
            } else if value_str.ends_with("mm") {
                value_str[..value_str.len()-2].trim().parse::<f32>().ok().map(|v| v * 2.834_645_7)
            } else if value_str.ends_with("px") {
                // Approximate px to pt (1px ≈ 0.75pt at 96dpi)
                value_str[..value_str.len()-2].trim().parse::<f32>().ok().map(|v| v * 0.75)
            } else {
                // Try parsing as bare number (assume pt)
                value_str.parse().ok()
            }
        } else {
            None
        }
    }

    /// Parse CSS text-align property from style string
    fn parse_css_alignment(style: &str) -> HAlign {
        if style.contains("text-align:justify") || style.contains("text-align: justify") {
            HAlign::Justify
        } else if style.contains("text-align:center") || style.contains("text-align: center") {
            HAlign::Center
        } else if style.contains("text-align:right") || style.contains("text-align: right") {
            HAlign::Right
        } else {
            HAlign::Left
        }
    }

    /// Parse CSS text-align property from style string, returning None if not specified
    fn parse_css_alignment_optional(style: &str) -> Option<HAlign> {
        if style.contains("text-align:justify") || style.contains("text-align: justify") {
            Some(HAlign::Justify)
        } else if style.contains("text-align:center") || style.contains("text-align: center") {
            Some(HAlign::Center)
        } else if style.contains("text-align:right") || style.contains("text-align: right") {
            Some(HAlign::Right)
        } else if style.contains("text-align:left") || style.contains("text-align: left") {
            Some(HAlign::Left)
        } else {
            None  // CSS doesn't specify - use default
        }
    }

    // ========================================================================
    // XFA-Compliant Text Layout and Wrapping
    // ========================================================================

    /// Layout rich text into rendered lines with proper word wrapping.
    /// This handles:
    /// - Per-paragraph text-indent (first line only)
    /// - Preserved spaces (xfa-spacerun)
    /// - Proper word breaking (don't break on NBSP)
    /// - Justify preparation (marking first/last lines)
    /// Per XFA spec: letterSpacing affects interword and interletter spacings
    pub fn layout_rich_text(
        rich_text: &RichText,
        max_width: f32,
        font_size: f32,
        font: &FontRef<'_>,
        scale: f32,
        letter_spacing: f32,
    ) -> Vec<RenderedLine> {
        let mut lines = Vec::new();
        let px_scale = PxScale::from(font_size);
        let scaled_font = font.as_scaled(px_scale);
        
        // Get space width (also affected by letter spacing per XFA spec)
        let space_glyph = font.glyph_id(' ');
        let base_space_width = if space_glyph.0 != 0 {
            scaled_font.h_advance(space_glyph)
        } else {
            font_size * 0.3
        };
        let space_width = base_space_width + letter_spacing;
        
        for para in &rich_text.paragraphs {
            // Handle empty paragraphs as blank lines
            if para.is_empty {
                lines.push(RenderedLine {
                    words: vec![],
                    y: 0.0, // Will be calculated later
                    is_first_line: true,
                    is_last_line: true,
                    text_indent: 0.0,
                    h_align: para.h_align,
                    content_width: 0.0,
                });
                continue;
            }
            
            // Calculate effective indent (in pixels after scaling)
            let para_indent = para.text_indent.unwrap_or(0.0) * scale;
            
            // Collect all text from runs into tokens for wrapping
            let tokens = Self::tokenize_paragraph_runs(&para.runs, font_size, font, letter_spacing);
            
            if tokens.is_empty() {
                // Empty paragraph - add blank line
                lines.push(RenderedLine {
                    words: vec![],
                    y: 0.0,
                    is_first_line: true,
                    is_last_line: true,
                    text_indent: para_indent,
                    h_align: para.h_align,
                    content_width: 0.0,
                });
                continue;
            }
            
            // Word-wrap the tokens
            let para_lines = Self::wrap_tokens_to_lines(&tokens, max_width, para_indent, space_width);
            let num_para_lines = para_lines.len();
            
            for (i, line_tokens) in para_lines.into_iter().enumerate() {
                let is_first = i == 0;
                let is_last = i == num_para_lines - 1;
                
                // Calculate content width
                let mut content_width: f32 = 0.0;
                for (j, token) in line_tokens.iter().enumerate() {
                    content_width += token.width;
                    if j < line_tokens.len() - 1 {
                        content_width += space_width;
                    }
                }
                
                // Convert tokens to rendered words (positioning happens during render)
                let words: Vec<RenderedWord> = line_tokens.into_iter().map(|t| RenderedWord {
                    text: t.text,
                    x: 0.0, // Will be calculated during render
                    preserve_spaces: t.preserve_spaces,
                    bold: t.bold,
                    italic: t.italic,
                }).collect();
                
                lines.push(RenderedLine {
                    words,
                    y: 0.0, // Will be calculated later
                    is_first_line: is_first,
                    is_last_line: is_last,
                    text_indent: if is_first { para_indent } else { 0.0 },
                    h_align: para.h_align,
                    content_width,
                });
            }
        }
        
        lines
    }

    /// Tokenize paragraph runs into layout tokens
    /// Per XFA spec: letterSpacing affects interword and interletter spacings
    fn tokenize_paragraph_runs(
        runs: &[RichRun],
        font_size: f32,
        font: &FontRef<'_>,
        letter_spacing: f32,
    ) -> Vec<LayoutToken> {
        let px_scale = PxScale::from(font_size);
        let _scaled_font = font.as_scaled(px_scale);
        
        let mut tokens = Vec::new();
        
        for run in runs {
            if run.preserve_spaces {
                // Preserved space run - keep as single token
                let width = Self::measure_text_width(&run.text, font_size, font, letter_spacing);
                if !run.text.is_empty() {
                    tokens.push(LayoutToken {
                        text: run.text.clone(),
                        width,
                        preserve_spaces: true,
                        bold: run.bold,
                        italic: run.italic,
                    });
                }
            } else {
                // Normal text - split into words
                let mut current_word = String::new();
                
                for ch in run.text.chars() {
                    if ch == ' ' {
                        if !current_word.is_empty() {
                            let width = Self::measure_text_width(&current_word, font_size, font, letter_spacing);
                            tokens.push(LayoutToken {
                                text: current_word.clone(),
                                width,
                                preserve_spaces: false,
                                bold: run.bold,
                                italic: run.italic,
                            });
                            current_word.clear();
                        }
                    } else {
                        current_word.push(ch);
                    }
                }
                
                // Don't forget the last word
                if !current_word.is_empty() {
                    let width = Self::measure_text_width(&current_word, font_size, font, letter_spacing);
                    tokens.push(LayoutToken {
                        text: current_word,
                        width,
                        preserve_spaces: false,
                        bold: run.bold,
                        italic: run.italic,
                    });
                }
            }
        }
        
        tokens
    }

    /// Wrap tokens into lines respecting max width and indentation
    fn wrap_tokens_to_lines(
        tokens: &[LayoutToken],
        max_width: f32,
        first_line_indent: f32,
        space_width: f32,
    ) -> Vec<Vec<LayoutToken>> {
        if tokens.is_empty() {
            return vec![vec![]];
        }
        
        let mut lines: Vec<Vec<LayoutToken>> = Vec::new();
        let mut current_line: Vec<LayoutToken> = Vec::new();
        let mut current_width: f32 = 0.0;
        let mut is_first_line = true;
        
        for token in tokens {
            let effective_max = if is_first_line {
                max_width - first_line_indent
            } else {
                max_width
            };
            
            let token_space = if current_line.is_empty() { 0.0 } else { space_width };
            
            if current_width + token_space + token.width <= effective_max || current_line.is_empty() {
                // Token fits on current line
                if !current_line.is_empty() {
                    current_width += space_width;
                }
                current_width += token.width;
                current_line.push(token.clone());
            } else {
                // Token doesn't fit - start new line
                lines.push(current_line);
                current_line = vec![token.clone()];
                current_width = token.width;
                is_first_line = false;
            }
        }
        
        // Don't forget the last line
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        
        if lines.is_empty() {
            lines.push(vec![]);
        }
        
        lines
    }

    /// Measure text width using font metrics
    /// Per XFA spec: letterSpacing "specifies an adjustment to the spacing that would
    /// otherwise be used between successive grapheme clusters"
    fn measure_text_width(text: &str, font_size: f32, font: &FontRef<'_>, letter_spacing: f32) -> f32 {
        let px_scale = PxScale::from(font_size);
        let scaled_font = font.as_scaled(px_scale);
        
        let mut width: f32 = 0.0;
        let char_count = text.chars().count();
        for (i, ch) in text.chars().enumerate() {
            let glyph_id = font.glyph_id(ch);
            if glyph_id.0 != 0 {
                width += scaled_font.h_advance(glyph_id);
            } else {
                // Fallback for missing glyphs
                width += font_size * 0.6;
            }
            // Add letter spacing between characters (not after the last one)
            if i < char_count - 1 {
                width += letter_spacing;
            }
        }
        width
    }

    // ========================================================================
    // XFA-Compliant Glyph-by-Glyph Text Rendering
    // ========================================================================

    /// Render text with proper glyph-by-glyph positioning.
    /// This handles:
    /// - Justify alignment (distributes extra space between words)
    /// - Text-indent on first line of paragraphs
    /// - Preserved spaces (xfa-spacerun)
    /// - Bold/italic variants for styled text
    /// Per XFA spec: letterSpacing affects spacing between grapheme clusters
    pub fn render_text_glyph_by_glyph(
        img: &mut RgbaImage,
        lines: &[RenderedLine],
        box_x: i32,
        box_y: i32,
        box_w: i32,
        box_h: i32,
        font_size: f32,
        base_font: &FontRef<'_>,
        bold_font: Option<&FontRef<'_>>,
        italic_font: Option<&FontRef<'_>>,
        bold_italic_font: Option<&FontRef<'_>>,
        color: Rgba<u8>,
        para: &Option<Para>,
        scale: f32,
        letter_spacing: f32,
    ) {
        let px_scale = PxScale::from(font_size);
        let scaled_font = base_font.as_scaled(px_scale);
        
        // Get space width and font metrics (space width affected by letter spacing)
        let space_glyph = base_font.glyph_id(' ');
        let base_space_width = if space_glyph.0 != 0 {
            scaled_font.h_advance(space_glyph)
        } else {
            font_size * 0.3
        };
        let space_width = base_space_width + letter_spacing;
        
        // Get vertical alignment and spacing from para
        let v_align = para.as_ref().map(|p| p.v_align).unwrap_or(VAlign::Top);
        let space_above = para.as_ref()
            .and_then(|p| p.space_above)
            .map(|s| s.to_f32().unwrap_or(0.0) * scale)
            .unwrap_or(0.0);
        let line_height_override = para.as_ref()
            .and_then(|p| p.line_height)
            .map(|lh| lh.to_f32().unwrap_or(0.0) * scale);
        
        // Font metrics
        let ascent = scaled_font.ascent();
        let descent = scaled_font.descent().abs();
        let line_gap = font_size * 0.2;
        let text_height = ascent + descent;
        let line_spacing = line_height_override.unwrap_or(text_height + line_gap);
        
        // Calculate total text block height
        let total_lines = lines.len();
        let total_height = if total_lines == 0 {
            0.0
        } else if total_lines == 1 {
            text_height
        } else {
            (total_lines - 1) as f32 * line_spacing + text_height
        };
        
        // Calculate starting Y based on vertical alignment
        let start_y = match v_align {
            VAlign::Top => box_y as f32 + space_above,
            VAlign::Middle => box_y as f32 + (box_h as f32 - total_height) / 2.0,
            VAlign::Bottom => box_y as f32 + box_h as f32 - total_height,
        };
        
        // Get paragraph margins
        let margin_left = para.as_ref()
            .and_then(|p| p.margin_left)
            .map(|m| m.to_f32().unwrap_or(0.0) * scale)
            .unwrap_or(0.0);
        let margin_right = para.as_ref()
            .and_then(|p| p.margin_right)
            .map(|m| m.to_f32().unwrap_or(0.0) * scale)
            .unwrap_or(0.0);
        
        let effective_width = box_w as f32 - margin_left - margin_right;
        
        // Render each line
        for (line_idx, line) in lines.iter().enumerate() {
            let line_y = start_y + (line_idx as f32 * line_spacing);
            
            if line_y < 0.0 || line_y > img.height() as f32 {
                continue;
            }
            
            if line.words.is_empty() {
                continue;
            }
            
            // Calculate available width (considering text-indent for first line)
            let available_width = effective_width - line.text_indent;
            
            // Determine alignment and spacing
            let (start_x, extra_space) = match line.h_align {
                HAlign::Left => {
                    (box_x as f32 + margin_left + line.text_indent, 0.0)
                }
                HAlign::Center => {
                    let offset = (available_width - line.content_width) / 2.0;
                    (box_x as f32 + margin_left + line.text_indent + offset, 0.0)
                }
                HAlign::Right => {
                    let offset = available_width - line.content_width;
                    (box_x as f32 + margin_left + line.text_indent + offset, 0.0)
                }
                HAlign::Justify | HAlign::JustifyAll => {
                    // Only justify if not the last line (unless JustifyAll)
                    if line.is_last_line && line.h_align != HAlign::JustifyAll {
                        // Last line of paragraph - left align
                        (box_x as f32 + margin_left + line.text_indent, 0.0)
                    } else if line.words.len() > 1 {
                        // Distribute extra space between words
                        let extra = available_width - line.content_width;
                        let gaps = (line.words.len() - 1) as f32;
                        (box_x as f32 + margin_left + line.text_indent, extra / gaps)
                    } else {
                        (box_x as f32 + margin_left + line.text_indent, 0.0)
                    }
                }
                HAlign::Radix => {
                    // Simplified: treat as center
                    let offset = (available_width - line.content_width) / 2.0;
                    (box_x as f32 + margin_left + line.text_indent + offset, 0.0)
                }
            };
            
            // Render words with proper spacing
            let mut x = start_x;
            for (word_idx, word) in line.words.iter().enumerate() {
                // Select the appropriate font variant based on word style
                let word_font: &FontRef<'_> = if word.bold && word.italic {
                    bold_italic_font.unwrap_or(bold_font.unwrap_or(base_font))
                } else if word.bold {
                    bold_font.unwrap_or(base_font)
                } else if word.italic {
                    italic_font.unwrap_or(base_font)
                } else {
                    base_font
                };
                
                // Render each glyph with letter spacing
                Self::render_glyphs(
                    img,
                    &word.text,
                    x as i32,
                    line_y as i32,
                    font_size,
                    word_font,
                    color,
                    letter_spacing,
                );
                
                // Advance position (word width already includes letter spacing)
                let word_width = Self::measure_text_width(&word.text, font_size, word_font, letter_spacing);
                x += word_width;
                
                // Add space between words (space_width already includes letter_spacing)
                if word_idx < line.words.len() - 1 {
                    x += space_width + extra_space;
                }
            }
        }
    }

    /// Render individual glyphs for a text string with letter spacing
    /// Per XFA spec: letterSpacing affects spacing between grapheme clusters
    fn render_glyphs(
        img: &mut RgbaImage,
        text: &str,
        x: i32,
        y: i32,
        font_size: f32,
        font: &FontRef<'_>,
        color: Rgba<u8>,
        letter_spacing: f32,
    ) {
        let px_scale = PxScale::from(font_size);
        
        // If no letter spacing, use the fast path
        if letter_spacing.abs() < 0.001 {
            draw_text_mut(img, color, x, y, px_scale, font, text);
            return;
        }
        
        // Render each character with letter spacing
        let scaled_font = font.as_scaled(px_scale);
        let mut current_x = x as f32;
        let char_count = text.chars().count();
        
        for (i, ch) in text.chars().enumerate() {
            let ch_str = ch.to_string();
            draw_text_mut(img, color, current_x as i32, y, px_scale, font, &ch_str);
            
            // Advance position by glyph width + letter spacing
            let glyph_id = font.glyph_id(ch);
            let advance = if glyph_id.0 != 0 {
                scaled_font.h_advance(glyph_id)
            } else {
                font_size * 0.6
            };
            current_x += advance;
            
            // Add letter spacing between characters (not after the last one)
            if i < char_count - 1 {
                current_x += letter_spacing;
            }
        }
    }

    // ========================================================================
    // Helper: Extract rich text from exData for draw elements
    // ========================================================================

    /// Extract rich text from exData HTML content.
    /// Returns RichText structure if HTML content is found, None otherwise.
    pub fn extract_rich_text_from_exdata(children: &[XfaNode], default_h_align: HAlign) -> Option<RichText> {
        for child in children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                && tag_name == "body" {
                    return Some(Self::parse_rich_text_from_html(&[child.clone()], default_h_align));
                }
            // Recurse into children
            if let Some(rich_text) = Self::extract_rich_text_from_exdata(&child.children, default_h_align) {
                return Some(rich_text);
            }
        }
        None
    }

    /// Check if the value node contains rich text (exData with HTML)
    pub fn has_rich_text_content(children: &[XfaNode]) -> bool {
        for child in children {
            if matches!(child.kind, XfaNodeKind::Value) {
                for value_child in &child.children {
                    if let XfaNodeKind::Element { tag_name, .. } = &value_child.kind
                        && tag_name == "exData" {
                            // Check if it has HTML body content
                            for ex_child in &value_child.children {
                                if let XfaNodeKind::Element { tag_name: inner_tag, .. } = &ex_child.kind
                                    && inner_tag == "body" {
                                        return true;
                                    }
                            }
                        }
                }
            }
        }
        false
    }

    /// Get rich text from a node's value if present
    pub fn get_rich_text_from_value(children: &[XfaNode], default_h_align: HAlign) -> Option<RichText> {
        for child in children {
            if matches!(child.kind, XfaNodeKind::Value) {
                for value_child in &child.children {
                    if let XfaNodeKind::Element { tag_name, .. } = &value_child.kind
                        && tag_name == "exData" {
                            return Self::extract_rich_text_from_exdata(&value_child.children, default_h_align);
                        }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xfa::{Para, VAlign, num};
    use crate::font_manager::get_font_manager;
    
    /// Test that line height from para element is properly scaled.
    /// This tests the fix for the "Vereinbarung" title line spacing issue.
    /// 
    /// Per XFA spec: lineHeight attribute on para element specifies the line spacing
    /// in points. When rendering, this must be scaled to pixels using the render scale.
    #[test]
    fn test_line_height_scaling() {
        // Get a font for testing
        let font_manager = get_font_manager();
        let mut mgr = font_manager.lock().unwrap();
        let default_font = crate::xfa::Font::default();
        let font = mgr.get_font(&default_font).unwrap();
        
        // Create para with lineHeight of 22.5pt (like the T_FormTitle element)
        let para = Some(Para {
            h_align: crate::xfa::HAlign::Left,
            v_align: VAlign::Top,
            line_height: Some(num(22.5)),  // 22.5pt line height
            space_above: Some(num(5.0)),   // 5pt space above
            space_below: None,
            text_indent: None,
            margin_left: None,
            margin_right: None,
        });
        
        // Test with scale=1.0 (1x resolution)
        let font_size_scaled = 18.0; // 18pt font, already scaled
        let render_scale = 1.0;
        
        // Calculate Y positions for two lines
        let y_line_0 = Flattened::calculate_text_y(0, 100, font_size_scaled, &para, &font, 0, 2, render_scale);
        let y_line_1 = Flattened::calculate_text_y(0, 100, font_size_scaled, &para, &font, 1, 2, render_scale);
        
        // Line spacing should be approximately lineHeight (22.5pt) at scale 1.0
        let line_spacing = (y_line_1 - y_line_0) as f32;
        assert!((line_spacing - 22.5).abs() < 1.0, 
            "Line spacing at scale 1.0 should be ~22.5, got {}", line_spacing);
        
        // Test with scale=2.0 (2x resolution, like Retina)
        let font_size_scaled_2x = 36.0; // 18pt * 2 = 36px
        let render_scale_2x = 2.0;
        
        let y_line_0_2x = Flattened::calculate_text_y(0, 200, font_size_scaled_2x, &para, &font, 0, 2, render_scale_2x);
        let y_line_1_2x = Flattened::calculate_text_y(0, 200, font_size_scaled_2x, &para, &font, 1, 2, render_scale_2x);
        
        // Line spacing should be approximately lineHeight * scale (22.5 * 2 = 45px) at scale 2.0
        let line_spacing_2x = (y_line_1_2x - y_line_0_2x) as f32;
        assert!((line_spacing_2x - 45.0).abs() < 2.0, 
            "Line spacing at scale 2.0 should be ~45, got {}", line_spacing_2x);
    }
    
    /// Test that space_above from para element is properly scaled.
    #[test]
    fn test_space_above_scaling() {
        let font_manager = get_font_manager();
        let mut mgr = font_manager.lock().unwrap();
        let default_font = crate::xfa::Font::default();
        let font = mgr.get_font(&default_font).unwrap();
        
        // Create para with spaceAbove of 10pt
        let para_with_space = Some(Para {
            h_align: crate::xfa::HAlign::Left,
            v_align: VAlign::Top,
            line_height: None,
            space_above: Some(num(10.0)),  // 10pt space above
            space_below: None,
            text_indent: None,
            margin_left: None,
            margin_right: None,
        });
        
        let para_without_space = Some(Para {
            h_align: crate::xfa::HAlign::Left,
            v_align: VAlign::Top,
            line_height: None,
            space_above: None,
            space_below: None,
            text_indent: None,
            margin_left: None,
            margin_right: None,
        });
        
        let font_size = 12.0;
        let render_scale = 2.0;
        
        // Y with space_above at 2x scale
        let y_with_space = Flattened::calculate_text_y(0, 100, font_size * render_scale, &para_with_space, &font, 0, 1, render_scale);
        let y_without_space = Flattened::calculate_text_y(0, 100, font_size * render_scale, &para_without_space, &font, 0, 1, render_scale);
        
        // Difference should be space_above * scale = 10 * 2 = 20 pixels
        let space_diff = (y_with_space - y_without_space) as f32;
        assert!((space_diff - 20.0).abs() < 1.0,
            "Space above at scale 2.0 should add ~20px, got {}", space_diff);
    }
}