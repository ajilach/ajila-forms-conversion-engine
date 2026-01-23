use crate::xfa::{XfaNode, XfaNodeKind, Border, Font, Para, HAlign, VAlign, StrokeStyle, Num, num};
use crate::scripting::{XfaScriptEngine, parse_events_from_node, ScriptContentType, EventActivity, EventRef};
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
    },
    
    /// Input field
    Field {
        name: String,
        value: String,
        label: String,
    },
}

impl FlattenedNode {
    /// Create a new text node
    pub fn new_text(content: String, font_size: Num, font_name: String, x: Num, y: Num, width: Num, height: Num) -> Self {
        FlattenedNode {
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name: None },
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
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name: None },
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
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name: None },
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
            kind: FlattenedNodeKind::Text { content, font_size, font_name, source_name },
            x,
            y,
            width,
            height,
            rotate,
            style,
        }
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

/// Context for embed resolution during flattening
/// Bundles the data needed to resolve xfa:embed references
struct EmbedContext<'a> {
    /// Map of field name -> computed value from scripts
    computed_values: &'a HashMap<String, String>,
    /// Map of element ID -> field name for resolving embed URI references
    id_to_field: &'a HashMap<String, String>,
}

impl<'a> EmbedContext<'a> {
    fn new(computed_values: &'a HashMap<String, String>, id_to_field: &'a HashMap<String, String>) -> Self {
        EmbedContext { computed_values, id_to_field }
    }
}

// Thread-local storage for embed context during flattening
// This allows us to pass the context through recursive calls without changing all function signatures
thread_local! {
    static EMBED_CONTEXT: std::cell::RefCell<Option<(HashMap<String, String>, HashMap<String, String>)>> = 
        std::cell::RefCell::new(None);
}

/// Set the embed context for the current thread during flattening
fn set_embed_context(computed_values: HashMap<String, String>, id_to_field: HashMap<String, String>) {
    EMBED_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = Some((computed_values, id_to_field));
    });
}

/// Clear the embed context after flattening
fn clear_embed_context() {
    EMBED_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = None;
    });
}

/// Get text content with embed resolution using thread-local context
fn extract_text_with_embed_context(children: &[XfaNode]) -> Option<String> {
    EMBED_CONTEXT.with(|ctx| {
        let borrowed = ctx.borrow();
        if let Some((computed_values, id_to_field)) = borrowed.as_ref() {
            Flattened::extract_text_content_with_embed(children, computed_values, id_to_field)
        } else {
            Flattened::extract_text_content(children)
        }
    })
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
    /// 2. Executes all form-ready scripts to compute field values
    /// 3. Builds an ID-to-field-name map for resolving xfa:embed references
    /// 4. Uses those computed values during flattening
    /// 
    /// Parameters:
    /// - `xfa_nodes`: The parsed XFA template nodes
    /// - `language`: The language code (e.g., "DE", "EN", "SP") for translations
    /// - `form_id`: The form ID (e.g., "AAAB_019_DE") used by some scripts
    pub fn from_xfa_with_scripts(xfa_nodes: &[XfaNode], language: &str, form_id: &str) -> Result<Self, String> {
        // Execute scripts and collect computed values
        let computed_values = Self::execute_form_ready_scripts(xfa_nodes, language, form_id)?;
        
        // Build ID-to-field-name map for xfa:embed resolution
        let id_to_field = Self::build_id_to_field_map(xfa_nodes);
        
        // Flatten with computed values and ID map
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
    
    /// Execute all form-ready scripts and return a map of field name -> computed value
    fn execute_form_ready_scripts(
        xfa_nodes: &[XfaNode], 
        language: &str, 
        form_id: &str
    ) -> Result<HashMap<String, String>, String> {
        let mut computed_values = HashMap::new();
        let mut engine = XfaScriptEngine::new();
        
        // Register control fields used by scripts
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", language);
        engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", form_id);
        
        // Extract and register translation objects from the XFA
        Self::extract_and_register_translations(xfa_nodes, &mut engine);
        
        // Find all events recursively
        fn find_all_events(nodes: &[XfaNode], events: &mut Vec<(String, crate::scripting::XfaScript)>) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                
                // Look for event children
                let node_events = parse_events_from_node(&node.children);
                for event in node_events {
                    events.push((name.clone(), event));
                }
                
                // Recurse into children
                find_all_events(&node.children, events);
            }
        }
        
        let mut all_events = Vec::new();
        find_all_events(xfa_nodes, &mut all_events);
        
        // Execute all form-ready JavaScript events
        for (field_name, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript 
                && script.activity == EventActivity::Ready 
                && script.event_ref == EventRef::Form 
                && !field_name.is_empty() 
            {
                // Set up field context
                engine.set_current_field(field_name, field_name, "");
                
                // Execute the script
                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(field_name.clone(), value);
                }
            }
        }
        
        Ok(computed_values)
    }
    
    /// Extract translation objects (myDE, myEN, mySP) from XFA <variables> sections
    /// Extract and execute variable scripts from the XFA template.
    /// According to XFA 3.3 spec (page 376-377), scripts in <variables> elements
    /// are compiled into script objects when the subform is instantiated during data binding.
    /// The script object is registered with the subform and can be referenced by name.
    fn extract_and_register_translations(xfa_nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        // Collect all script contents from <variables> elements
        let mut variable_scripts: Vec<(String, String)> = Vec::new();
        Self::collect_variable_scripts(xfa_nodes, &mut variable_scripts);
        
        // Execute each variable script wrapped to create a named script object
        // In XFA, a script in <variables name="foo"> becomes accessible as "foo" with its
        // functions and variables as properties/methods.
        for (name, content) in &variable_scripts {
            // Wrap the script content to create a named object
            // The script typically defines functions like setupVariables() which become methods
            let wrapped = format!(
                r#"
                var {name} = (function() {{
                    var _obj = {{}};
                    // Execute the script in the context where 'this' could be used
                    (function() {{
                        {content}
                        // Copy any functions defined to _obj
                        if (typeof setupVariables === 'function') {{
                            _obj.setupVariables = setupVariables;
                        }}
                    }})();
                    return _obj;
                }})();
                "#,
                name = name,
                content = content
            );
            
            let _ = engine.execute_variable_script(&wrapped);
        }
        
        // Initialize translation objects before calling setupVariables
        let init_globals = r#"
            if (typeof myDE === 'undefined') { myDE = {}; }
            if (typeof myEN === 'undefined') { myEN = {}; }
            if (typeof mySP === 'undefined') { mySP = {}; }
            if (typeof my66522D === 'undefined') { my66522D = {}; }
            if (typeof my70334D === 'undefined') { my70334D = {}; }
        "#;
        let _ = engine.execute_variable_script(init_globals);
        
        // Now call setupVariables() on each script object that has it
        // This mimics the initialize event: soCommonLabelDefinition.setupVariables()
        for (name, _) in &variable_scripts {
            let call_setup = format!(
                "if (typeof {name} !== 'undefined' && typeof {name}.setupVariables === 'function') {{ {name}.setupVariables(); }}",
                name = name
            );
            let _ = engine.execute_variable_script(&call_setup);
        }
    }
    
    /// Recursively collect script content from <variables> elements
    fn collect_variable_scripts(nodes: &[XfaNode], scripts: &mut Vec<(String, String)>) {
        for node in nodes {
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                if tag_name == "variables" {
                    // Look for script children
                    for child in &node.children {
                        if let XfaNodeKind::Element { tag_name: child_tag, text_content, .. } = &child.kind {
                            if child_tag == "script" {
                                if let Some(name) = child.name.as_ref().or_else(|| child.attributes.get("name")) {
                                    if let Some(content) = text_content {
                                        scripts.push((name.clone(), content.clone()));
                                    }
                                }
                            }
                        }
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
    /// - `xfa_nodes`: The parsed XFA template nodes
    /// - `computed_values`: Map of field name -> computed value from scripts
    /// - `id_to_field`: Map of element ID -> field name for resolving xfa:embed references
    fn from_xfa_with_computed_values(
        xfa_nodes: &[XfaNode], 
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
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
            
            for child in &page_area.children {
                // Skip contentArea and medium - these define page structure, not content
                if matches!(child.kind, XfaNodeKind::ContentArea) {
                    continue;
                }
                if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                    if tag_name == "contentArea" || tag_name == "medium" {
                        continue;
                    }
                }
                
                // Render page background element with positioned layout relative to page origin
                Self::flatten_single_node(child, page_position, Layout::Position, &mut flattened_nodes)?;
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
        
        // Set up embed context for resolving xfa:embed references during text extraction
        set_embed_context(computed_values.clone(), id_to_field.clone());
        
        // Find and flatten the root content subform (the Form DOM)
        // This is the sibling to pageSet, NOT inside pageArea
        let flatten_result = if let Some(root_subform) = Self::find_root_subform(xfa_nodes) {
            // Get the layout from the root subform (often "tb" for top-to-bottom)
            let layout = root_subform.layout.as_ref()
                .map(|l| Layout::from_str(l))
                .unwrap_or(Layout::Position);
            
            Self::flatten_nodes(&root_subform.children, root_position, layout, &mut flattened_nodes)
        } else {
            // Fallback: flatten all nodes (old behavior for simple forms without proper structure)
            Self::flatten_nodes(xfa_nodes, root_position, Layout::Position, &mut flattened_nodes)
        };
        
        // Clear embed context after flattening
        clear_embed_context();
        
        // Check for flatten errors
        flatten_result?;
        
        // Apply computed values from scripts to nodes
        for node in &mut flattened_nodes {
            match &mut node.kind {
                FlattenedNodeKind::Field { name, value, .. } => {
                    // If we have a computed value for this field and it currently has no value,
                    // use the computed value
                    if value.is_empty() {
                        if let Some(computed) = computed_values.get(name) {
                            *value = computed.clone();
                        }
                    }
                }
                FlattenedNodeKind::Text { content, source_name, .. } => {
                    // For Draw elements with a source name, check if we have a computed value
                    if let Some(name) = source_name {
                        if content.is_empty() {
                            if let Some(computed) = computed_values.get(name) {
                                *content = computed.clone();
                            }
                        }
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
        fn find_content_subform_in_container<'a>(container: &'a XfaNode) -> Option<&'a XfaNode> {
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
                if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                    if tag_name == "subform" {
                        return Some(child);
                    }
                }
            }
            None
        }
        
        fn search_recursive<'a>(nodes: &'a [XfaNode]) -> Option<&'a XfaNode> {
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
                            let has_page_set = child.children.iter().any(|c| is_page_structure(c));
                            if !has_page_set {
                                return Some(child);
                            }
                        }
                    }
                }
                
                // Check Element nodes for template
                if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                    if tag_name == "template" {
                        for child in &node.children {
                            let is_subform = matches!(child.kind, XfaNodeKind::Subform) ||
                                matches!(&child.kind, XfaNodeKind::Element { tag_name: ct, .. } if ct == "subform");
                            
                            if is_subform {
                                // This is the root container subform
                                if let Some(content_subform) = find_content_subform_in_container(child) {
                                    return Some(content_subform);
                                }
                                // Fallback: use the container if no pageSet
                                let has_page_set = child.children.iter().any(|c| is_page_structure(c));
                                if !has_page_set {
                                    return Some(child);
                                }
                            }
                        }
                    }
                }
                
                // Only recurse into Template or container nodes, skip data elements
                if !is_data_element(node) {
                    if let Some(result) = search_recursive(&node.children) {
                        return Some(result);
                    }
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
        parent_layout: Layout,
        flattened_nodes: &mut Vec<FlattenedNode>,
    ) -> Result<(), String> {
        // For positioned layout, use node's x,y directly
        let x = node.x.unwrap_or(Decimal::ZERO);
        let y = node.y.unwrap_or(Decimal::ZERO);
        
        // Per XFA spec: if w is not specified, the element is horizontally growable.
        // Use minW as the width, or calculate natural width for Draw elements.
        let width = node.w.unwrap_or_else(|| {
            // For Draw elements without explicit width, use minW or natural text width
            if let XfaNodeKind::Draw = &node.kind {
                let text = extract_text_with_embed_context(&node.children).unwrap_or_default();
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
                // Use embed context to resolve xfa:embed references
                let text_content = extract_text_with_embed_context(&node.children).unwrap_or_default();
                let font_size = Self::extract_font_size(node);
                let font_name = Self::extract_font_name(node);
                let style = Self::extract_style(node);
                
                flattened_nodes.push(FlattenedNode::new_text_styled_rotated_named(
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
                    Self::flatten_single_node(child, pos, Layout::Position, flattened_nodes)?;
                }
            }
            _ => {}
        }
        
        Ok(())
    }
    
    fn find_page_and_content_area(nodes: &[XfaNode]) -> Option<(&XfaNode, &XfaNode)> {
        fn search_recursive<'a>(nodes: &'a [XfaNode]) -> Option<(&'a XfaNode, &'a XfaNode)> {
            for node in nodes {
                // Check for PageArea node type
                if matches!(node.kind, XfaNodeKind::PageArea) {
                    // Found pageArea, now look for contentArea within it
                    for child in &node.children {
                        if matches!(child.kind, XfaNodeKind::ContentArea) {
                            return Some((node, child));
                        }
                        // Also check Element nodes that might be contentArea
                        if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                            if tag_name == "contentArea" {
                                return Some((node, child));
                            }
                        }
                    }
                    // If no contentArea found, return pageArea twice (use page dimensions)
                    return Some((node, node));
                }
                
                // Check for pageArea as Element
                if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                    if tag_name == "pageArea" {
                        // Found pageArea as Element, look for contentArea
                        for child in &node.children {
                            if matches!(child.kind, XfaNodeKind::ContentArea) {
                                return Some((node, child));
                            }
                            if let XfaNodeKind::Element { tag_name: ca_tag, .. } = &child.kind {
                                if ca_tag == "contentArea" {
                                    return Some((node, child));
                                }
                            }
                        }
                        return Some((node, node));
                    }
                }
                
                // Recurse into all container-like nodes to find pageArea
                let should_recurse = matches!(node.kind, 
                    XfaNodeKind::Template | XfaNodeKind::PageSet | XfaNodeKind::Subform)
                    || matches!(&node.kind, XfaNodeKind::Element { .. });
                    
                if should_recurse {
                    if let Some(result) = search_recursive(&node.children) {
                        return Some(result);
                    }
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
    
    fn flatten_nodes(
        nodes: &[XfaNode],
        parent_position: Position,
        parent_layout: Layout,
        flattened_nodes: &mut Vec<FlattenedNode>,
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
            // Check presence attribute
            // Per XFA spec (section 2, "Explicitly Concealing Containers"):
            // - "visible" - element is rendered and participates in layout (normal behavior)
            // - "invisible" - element takes up space but is NOT rendered (participates in layout)
            // - "hidden" - element does NOT take up space and is NOT rendered (no layout)
            // - "inactive" - element does NOT take up space and is NOT rendered (no layout, no automation)
            let presence = node.attributes.get("presence").map(|s| s.as_str()).unwrap_or("visible");
            let skip_render = presence == "hidden" || presence == "invisible" || presence == "inactive";
            let skip_layout = presence == "hidden" || presence == "inactive";
            
            if skip_layout {
                // Hidden/Inactive: skip entirely - don't render, don't consume layout space
                continue;
            }
            
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
                    )?;
                    
                    // For positioned layout, track the max extent (relative to parent_position)
                    if parent_layout == Layout::Position {
                        let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                        max_extent_y = max_extent_y.max(node_bottom);
                    }
                    
                    // Recurse into subform children with the content position (inside margins)
                    // The subform's layout applies to its children
                    let children_height = Self::flatten_nodes(&node.children, content_pos, layout, flattened_nodes)?;
                    
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
                    )?;
                    
                    // For positioned layout, track the max extent (relative to parent_position)
                    if parent_layout == Layout::Position {
                        let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                        max_extent_y = max_extent_y.max(node_bottom);
                    }
                    
                    // Only add to output if not hidden
                    if !skip_render {
                        // Extract text content from draw node, or use empty (scripts may fill it)
                        // Use embed context to resolve xfa:embed references
                        let text_content = extract_text_with_embed_context(&node.children).unwrap_or_default();
                        let font_size = Self::extract_font_size(node);
                        let font_name = Self::extract_font_name(node);
                        let style = Self::extract_style(node);
                        
                        flattened_nodes.push(FlattenedNode::new_text_styled_rotated_named(
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
                            )?;
                            
                            // For positioned layout, track the max extent (relative to parent_position)
                            if parent_layout == Layout::Position {
                                let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                                max_extent_y = max_extent_y.max(node_bottom);
                            }
                            
                            let children_height = Self::flatten_nodes(&node.children, content_pos, layout, flattened_nodes)?;
                            
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
                            )?;
                            
                            // For positioned layout, track the max extent (relative to parent_position)
                            if parent_layout == Layout::Position {
                                let node_bottom = (outer_pos.y - parent_position.y) + outer_pos.height;
                                max_extent_y = max_extent_y.max(node_bottom);
                            }
                            
                            // Only add to output if not hidden
                            if !skip_render {
                                // Draw nodes render text or images - use empty string if no content (scripts may fill it)
                                // Use embed context to resolve xfa:embed references
                                let text_content = extract_text_with_embed_context(&node.children).unwrap_or_default();
                                let font_size = Self::extract_font_size(node);
                                let font_name = Self::extract_font_name(node);
                                let style = Self::extract_style(node);
                                
                                flattened_nodes.push(FlattenedNode::new_text_styled_rotated_named(
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
                            
                            Self::flatten_nodes(&node.children, parent_position, child_layout, flattened_nodes)?;
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
                            Self::flatten_nodes(&node.children, parent_position, parent_layout, flattened_nodes)?;
                        }
                    }
                }
                XfaNodeKind::Template | XfaNodeKind::ContentArea | XfaNodeKind::PageSet => {
                    // NOTE: These should NOT normally be encountered when processing Form DOM content.
                    // This handles fallback cases. Pass through with same parent position and layout.
                    Self::flatten_nodes(&node.children, parent_position, parent_layout, flattened_nodes)?;
                }
                XfaNodeKind::PageArea => {
                    // NOTE: PageArea should NOT normally be encountered when processing Form DOM content.
                    // Page background (pageArea children) are handled separately in from_xfa().
                    // This fallback handles edge cases - pass through with positioned layout.
                    Self::flatten_nodes(&node.children, parent_position, Layout::Position, flattened_nodes)?;
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
                    let text = extract_text_with_embed_context(&node.children).unwrap_or_default();
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
                    let text = extract_text_with_embed_context(&node.children).unwrap_or_default();
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
                    // Use embed context to resolve xfa:embed references for accurate height
                    let natural_content_height = if let Some(text) = extract_text_with_embed_context(&node.children) {
                        Self::calculate_natural_text_height(
                            &text, 
                            &node.font, 
                            &node.para, 
                            width
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
                            // Use embed context to resolve xfa:embed references for accurate height
                            let natural_content_height = if let Some(text) = extract_text_with_embed_context(&node.children) {
                                Self::calculate_natural_text_height(
                                    &text, 
                                    &node.font, 
                                    &node.para, 
                                    width
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
        
        // Count approximate number of lines
        // Approximate character width as 60% of font size
        let char_width = font_size_f32 * 0.6;
        let max_width_f32 = max_width.to_f32().unwrap_or(1000.0);
        let chars_per_line = (max_width_f32 / char_width).max(1.0) as usize;
        
        // Count words and estimate lines
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut num_lines: usize = 1;
        let mut current_line_chars: usize = 0;
        
        for word in words {
            if current_line_chars == 0 {
                current_line_chars = word.len();
            } else if current_line_chars + 1 + word.len() <= chars_per_line {
                current_line_chars += 1 + word.len();
            } else {
                num_lines += 1;
                current_line_chars = word.len();
            }
        }
        
        if text.is_empty() {
            num_lines = 1;
        }
        
        // Per AXTE: line gap is 20% of font size
        let line_gap = font_size * num(0.2);
        
        // Per AXTE: if (ascent + descent) < font_size, use font_size as text height
        // We'll use font_size as an approximation for text height since we don't have
        // the actual font metrics here. For most fonts, ascent + descent ≈ font_size.
        let text_height = font_size;
        
        // Line spacing: use override if provided, else TH + LG
        let line_spacing = para.as_ref()
            .and_then(|p| p.line_height)
            .unwrap_or(text_height + line_gap);
        
        // Paragraph margins
        let margin_top = para.as_ref()
            .and_then(|p| p.space_above)
            .unwrap_or(Decimal::ZERO);
        let margin_bottom = para.as_ref()
            .and_then(|p| p.space_below)
            .unwrap_or(Decimal::ZERO);
        
        // Calculate total height per AXTE rules
        // FH = MT + DS + MB, with LG removed on last line
        let total_height = if num_lines == 1 {
            // Single line: MT + TH + MB (no line gap)
            margin_top + text_height + margin_bottom
        } else {
            // Multiple lines:
            // - First N-1 lines: each has height = DS (line_spacing)
            // - Last line: TH (text_height, no line gap)
            let first_lines = num(num_lines as f64 - 1.0) * line_spacing;
            let last_line = text_height;
            margin_top + first_lines + last_line + margin_bottom
        };
        
        total_height
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
                    if let XfaNodeKind::Element { text_content, .. } = &value_child.kind {
                        if let Some(text) = text_content {
                            return text.clone();
                        }
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
            if matches!(child.kind, XfaNodeKind::Value) {
                if let Some(text) = Self::extract_value_text_with_embed(&child.children, computed_values, id_to_field) {
                    return Some(text);
                }
            }
            // Also check for Element with tag_name "value" (when parsed via parse_element_content)
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "value" {
                    if let Some(text) = Self::extract_value_text_with_embed(&child.children, computed_values, id_to_field) {
                        return Some(text);
                    }
                }
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
                if tag_name == "text" {
                    if let Some(text) = text_content {
                        return Some(text.clone());
                    }
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
                    if tag_name == "p" || tag_name == "br" {
                        if !text_parts.is_empty() {
                            text_parts.push(" ".to_string());
                        }
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
    fn resolve_embed_reference(
        embed_ref: &str,
        computed_values: &HashMap<String, String>,
        id_to_field: &HashMap<String, String>
    ) -> Option<String> {
        // Handle URI reference (starts with #)
        if embed_ref.starts_with('#') {
            let id = &embed_ref[1..]; // Remove the # prefix
            
            // Look up the field name from the ID
            if let Some(field_name) = id_to_field.get(id) {
                // Get the computed value for this field
                return computed_values.get(field_name).cloned();
            }
            
            // Fallback: try the ID directly as a field name
            return computed_values.get(id).cloned();
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
        
        // Get a default fallback font for debug text
        let fallback_font = Self::load_fallback_font()?;
        
        // Colors (RGBA - last value is alpha: 255=opaque, 0=transparent)
        let black = Rgba([0u8, 0u8, 0u8, 255u8]);
        let dark_gray = Rgba([80u8, 80u8, 80u8, 255u8]);
        let light_blue_fill = Rgba([200u8, 220u8, 255u8, 255u8]); // Light blue for field backgrounds
        // Transparent red for debug overlay (alpha=100 for ~40% opacity)
        let debug_red = Rgba([255u8, 0u8, 0u8, 180u8]); // More visible red for field names
        let debug_red_outline = Rgba([255u8, 0u8, 0u8, 20u8]);
        
        // ============================================
        // PASS 1: Draw actual content (as in PDF)
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
            if let Some(border) = &node.style.border {
                if let Some(fill) = &border.fill {
                    if fill.presence != "hidden" && fill.presence != "inactive" {
                        if let Some((r, g, b)) = fill.color {
                            Self::fill_rect(&mut img, x, y, w, h, Rgba([r, g, b, 255u8]));
                        }
                    }
                }
            }
            
            // Draw border if present and visible
            if let Some(border) = &node.style.border {
                if border.is_visible() {
                    Self::draw_border(&mut img, x, y, w, h, border, scale);
                }
            }
            
            match &node.kind {
                FlattenedNodeKind::Field { value, .. } => {
                    // Draw light blue fill for field background (no border)
                    Self::fill_rect(&mut img, x, y, w, h, light_blue_fill);
                    
                    // Note: Field names are drawn in Pass 2 as debug/meta info in red
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
                        let text_y = Self::calculate_text_y(content_y, content_h, scaled_font_size, &node.style.para, &render_font, 0, 1);
                        
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
                FlattenedNodeKind::Text { content, font_size, .. } => {
                    // Draw text content (draw elements/labels)
                    if !content.is_empty() {
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
                        let render_font = {
                            let mut mgr = font_manager.lock().map_err(|e| format!("Lock error: {}", e))?;
                            mgr.get_font(&xfa_font).unwrap_or_else(|_| fallback_font.clone())
                        };
                        
                        // Get text color from style or use dark gray
                        let text_color = node.style.font.as_ref()
                            .and_then(|f| f.color)
                            .map(|(r, g, b)| Rgba([r, g, b, 255u8]))
                            .unwrap_or(dark_gray);
                        
                        // Calculate content area inside border margins
                        // Per XFA box model: content is drawn inside the border margins (insets)
                        // Per XFA spec: if h is null/0, the container is vertically growable
                        // and height should be computed from content
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
                            
                            // Per XFA spec: if h is 0 or very small, the container is vertically growable
                            // Height should be derived from content (use font size as minimum)
                            let effective_h = if h <= (scaled_font_size as i32) {
                                // Growable: use natural text height based on font metrics
                                // Estimate line count from content (rough approximation)
                                let line_gap = scaled_font_size * 0.2;
                                let natural_height = (scaled_font_size + line_gap) as i32;
                                natural_height.max(scaled_font_size as i32)
                            } else {
                                // Fixed height: apply border margins
                                (h - mt - mb).max(scaled_font_size as i32)
                            };
                            
                            (x + ml, y + mt, (w - ml - mr).max(0), effective_h)
                        };
                        
                        // Word-wrap text using proper font metrics and the correct font
                        let lines = Self::wrap_text_with_font(content, content_w as f32, scaled_font_size, &render_font);
                        let total_lines = lines.len();
                        
                        for (i, line) in lines.iter().enumerate() {
                            // Calculate x position based on alignment (within content area)
                            let line_x = Self::calculate_text_x(content_x, content_w, line, scaled_font_size, &node.style.para, &render_font);
                            
                            // Calculate y position using AXTE-compliant method (within content area)
                            let line_y = Self::calculate_text_y(content_y, content_h, scaled_font_size, &node.style.para, &render_font, i, total_lines);
                            
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
        
        // Save the image
        img.save(output_path.as_ref())
            .map_err(|e| format!("Failed to save image: {}", e))?;
        
        Ok(())
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
        let rot = ((rotate % 360) + 360) % 360;
        
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
    fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, color: Rgba<u8>) {
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
    fn calculate_text_y(box_y: i32, box_h: i32, font_size: f32, para: &Option<Para>, font: &FontRef<'_>, line_index: usize, total_lines: usize) -> i32 {
        let v_align = para.as_ref().map(|p| p.v_align).unwrap_or(VAlign::Top);
        let space_above = para.as_ref().and_then(|p| p.space_above).map(|s| s.to_f32().unwrap_or(0.0)).unwrap_or(0.0);
        let line_height_override = para.as_ref().and_then(|p| p.line_height).map(|lh| lh.to_f32().unwrap_or(0.0));
        
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
        
        // Derived line spacing: DS = TH + LG (unless overridden)
        let line_spacing = line_height_override.unwrap_or(text_height + line_gap);
        
        // Full height calculation for all lines
        let total_text_height = if total_lines == 1 {
            // Single line: no line gap at end
            text_height
        } else {
            // Multiple lines: full line spacing for all but last line
            let is_last_line = line_index == total_lines - 1;
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
    fn draw_transparent_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, color: Rgba<u8>) {
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
    fn load_fallback_font() -> Result<FontRef<'static>, String> {
        let manager = get_font_manager();
        let mut manager = manager.lock().map_err(|e| format!("Lock error: {}", e))?;
        manager.get_default_font()
    }
}