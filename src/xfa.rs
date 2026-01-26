use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use rust_decimal::Decimal;
use std::str::FromStr;

/// Numeric type alias for precision - using Decimal for arbitrary precision
pub type Num = Decimal;

/// Helper to create a Num from a float literal
pub fn num(value: f64) -> Num {
    Decimal::from_str(&value.to_string()).unwrap_or(Decimal::ZERO)
}

/// Border edge stroke style
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum StrokeStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    DashDot,
    DashDotDot,
    Lowered,
    Raised,
    Etched,
    Embossed,
}

impl FromStr for StrokeStyle {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "solid" => StrokeStyle::Solid,
            "dashed" => StrokeStyle::Dashed,
            "dotted" => StrokeStyle::Dotted,
            "dashDot" => StrokeStyle::DashDot,
            "dashDotDot" => StrokeStyle::DashDotDot,
            "lowered" => StrokeStyle::Lowered,
            "raised" => StrokeStyle::Raised,
            "etched" => StrokeStyle::Etched,
            "embossed" => StrokeStyle::Embossed,
            _ => StrokeStyle::Solid,
        })
    }
}

/// Corner join style for borders
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum JoinStyle {
    #[default]
    Square,
    Round,
}

impl FromStr for JoinStyle {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "round" => JoinStyle::Round,
            _ => JoinStyle::Square,
        })
    }
}

/// Border edge properties
#[derive(Debug, Clone, Default)]
pub struct Edge {
    pub thickness: Option<Num>,
    pub stroke: StrokeStyle,
    pub presence: String,  // "visible", "hidden", "inactive"
    pub color: Option<(u8, u8, u8)>,
}

/// Border corner properties
#[derive(Debug, Clone, Default)]
pub struct Corner {
    pub thickness: Option<Num>,
    pub join: JoinStyle,
    pub presence: String,
    pub radius: Option<Num>,  // for inverted corners
}

/// Border properties
/// Per XFA spec: border has up to 4 edges (top, right, bottom, left) and 4 corners (top-left, top-right, bottom-right, bottom-left)
/// If fewer than 4 are specified, the last one is reused
#[derive(Debug, Clone, Default)]
pub struct Border {
    pub edges: Vec<Edge>,
    pub corners: Vec<Corner>,
    pub fill: Option<Fill>,
    pub presence: String,
    /// Border margins (insets from the widget edge)
    pub margin_left: Option<Num>,
    pub margin_top: Option<Num>,
    pub margin_right: Option<Num>,
    pub margin_bottom: Option<Num>,
}

impl Border {
    /// Get edge for a specific side (0=top, 1=right, 2=bottom, 3=left)
    pub fn get_edge(&self, index: usize) -> Option<&Edge> {
        if self.edges.is_empty() {
            None
        } else {
            // If fewer than 4 edges, reuse the last one
            Some(&self.edges[index.min(self.edges.len() - 1)])
        }
    }
    
    /// Get corner for a specific position (0=top-left, 1=top-right, 2=bottom-right, 3=bottom-left)
    pub fn get_corner(&self, index: usize) -> Option<&Corner> {
        if self.corners.is_empty() {
            None
        } else {
            Some(&self.corners[index.min(self.corners.len() - 1)])
        }
    }
    
    /// Check if the border should be rendered
    pub fn is_visible(&self) -> bool {
        self.presence != "hidden" && self.presence != "inactive" && !self.edges.is_empty()
    }
}

/// Fill properties for backgrounds
#[derive(Debug, Clone, Default)]
pub struct Fill {
    pub color: Option<(u8, u8, u8)>,
    pub presence: String,
}

/// Generic font family for fallback per XFA spec
/// Per XFA spec section 28: "The genericFamily attribute values are defined by [CSS2]."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GenericFamily {
    /// Sans-serif fonts (no serifs) - this is the default
    #[default]
    SansSerif,
    /// Serif fonts (with serifs)
    Serif,
    /// Cursive/script fonts (handwritten look)
    Cursive,
    /// Fantasy/decorative fonts
    Fantasy,
    /// Monospace/fixed-width fonts
    Monospace,
}

impl FromStr for GenericFamily {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "serif" => GenericFamily::Serif,
            "sansserif" | "sans-serif" => GenericFamily::SansSerif,
            "cursive" => GenericFamily::Cursive,
            "fantasy" => GenericFamily::Fantasy,
            "monospace" => GenericFamily::Monospace,
            _ => GenericFamily::SansSerif, // Default per XFA spec
        })
    }
}

impl GenericFamily {
    
    /// Get XFA attribute value string
    pub fn as_xfa_str(&self) -> &'static str {
        match self {
            GenericFamily::SansSerif => "sansSerif",
            GenericFamily::Serif => "serif",
            GenericFamily::Cursive => "cursive",
            GenericFamily::Fantasy => "fantasy",
            GenericFamily::Monospace => "monospace",
        }
    }
}

/// Font properties
/// Per XFA spec section 17 (Template Reference - font element)
#[derive(Debug, Clone)]
pub struct Font {
    pub typeface: String,
    pub size: Num,
    pub weight: FontWeight,
    pub posture: FontPosture,
    pub underline: bool,
    pub line_through: bool,
    pub color: Option<(u8, u8, u8)>,
    pub baseline_shift: Option<Num>,
    /// Letter spacing adjustment (relative measurement, can be negative)
    /// Per XFA spec: "specifies an adjustment to the spacing that would otherwise
    /// be used between successive grapheme clusters. Interword as well as
    /// interletter spacings are affected." Default is 0.
    pub letter_spacing: Option<Num>,
    /// Generic font family for fallback when typeface is not available
    /// Per XFA spec section 28 (Font Mapping): used to select appropriate fallback
    pub generic_family: Option<GenericFamily>,
}

impl Default for Font {
    fn default() -> Self {
        // Per XFA spec section 17 "Template Reference":
        // - typeface: Default is "Courier"
        // - size: Default is 10pt
        // - weight: Default is "normal"
        // - posture: Default is "normal"
        // - letterSpacing: Default is 0
        Font {
            typeface: "Courier".to_string(),
            size: num(10.0),  // 10pt default
            weight: FontWeight::Normal,
            posture: FontPosture::Normal,
            underline: false,
            line_through: false,
            color: None,
            baseline_shift: None,
            letter_spacing: None,  // 0 default (no adjustment)
            generic_family: Some(GenericFamily::Monospace), // Courier is monospace
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontWeight {
    #[default]
    Normal,
    Bold,
}

impl FromStr for FontWeight {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "bold" => FontWeight::Bold,
            _ => FontWeight::Normal,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontPosture {
    #[default]
    Normal,
    Italic,
}

impl FromStr for FontPosture {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "italic" => FontPosture::Italic,
            _ => FontPosture::Normal,
        })
    }
}

/// Paragraph properties for text alignment and spacing
#[derive(Debug, Clone, Default)]
pub struct Para {
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub line_height: Option<Num>,
    pub space_above: Option<Num>,
    pub space_below: Option<Num>,
    pub text_indent: Option<Num>,
    pub margin_left: Option<Num>,
    pub margin_right: Option<Num>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
    JustifyAll,
    Radix,  // Align on decimal point
}

impl HAlign {
    pub fn from_str(s: &str) -> Self {
        match s {
            "center" => HAlign::Center,
            "right" => HAlign::Right,
            "justify" => HAlign::Justify,
            "justifyAll" => HAlign::JustifyAll,
            "radix" => HAlign::Radix,
            _ => HAlign::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    pub fn from_str(s: &str) -> Self {
        match s {
            "middle" => VAlign::Middle,
            "bottom" => VAlign::Bottom,
            _ => VAlign::Top,
        }
    }
}

/// Main XFA node structure containing layout information and node-specific data
#[derive(Debug, Clone)]
pub struct XfaNode {
    /// Node-specific information
    pub kind: XfaNodeKind,
    
    /// Layout-related attributes (parsed from attributes)
    pub x: Option<Num>,
    pub y: Option<Num>,
    pub w: Option<Num>,
    pub h: Option<Num>,
    pub min_w: Option<Num>,
    pub min_h: Option<Num>,
    pub max_w: Option<Num>,
    pub max_h: Option<Num>,
    pub layout: Option<String>,
    /// Rotation in degrees (counter-clockwise, multiples of 90)
    pub rotate: i32,
    
    /// Margin insets (parsed from child <margin> element)
    pub margin_top: Option<Num>,
    pub margin_bottom: Option<Num>,
    pub margin_left: Option<Num>,
    pub margin_right: Option<Num>,
    
    /// Border, font, and paragraph properties (parsed from child elements)
    pub border: Option<Border>,
    pub font: Option<Font>,
    pub para: Option<Para>,
    
    /// Node name (for named nodes like subforms, fields, etc.)
    pub name: Option<String>,
    
    /// All attributes from the XML (including layout attrs)
    pub attributes: HashMap<String, String>,
    
    /// Child nodes
    pub children: Vec<XfaNode>,
}

/// Enum representing the specific kind/type of XFA node
#[derive(Debug, Clone)]
pub enum XfaNodeKind {
    /// Template node (root of form template)
    Template,
    
    /// Subform container
    Subform,
    
    /// Input field
    Field,
    
    /// Page set container
    PageSet,
    
    /// Page area definition
    PageArea,
    
    /// Content area within a page
    ContentArea,
    
    /// Draw/static text element
    Draw,
    
    /// Value container
    Value,
    
    /// Text content
    Text { content: String },
    
    /// Bind element
    Bind,
    
    /// Generic/unknown element
    Element { tag_name: String, text_content: Option<String> },
}

impl XfaNode {
    /// Create a new XFA node
    pub fn new(kind: XfaNodeKind, attributes: HashMap<String, String>) -> Self {
        // Extract layout-related attributes
        let x = attributes.get("x").and_then(|v| Self::parse_dimension(v).ok());
        let y = attributes.get("y").and_then(|v| Self::parse_dimension(v).ok());
        let w = attributes.get("w").and_then(|v| Self::parse_dimension(v).ok());
        let h = attributes.get("h").and_then(|v| Self::parse_dimension(v).ok());
        let min_w = attributes.get("minW").and_then(|v| Self::parse_dimension(v).ok());
        let min_h = attributes.get("minH").and_then(|v| Self::parse_dimension(v).ok());
        let max_w = attributes.get("maxW").and_then(|v| Self::parse_dimension(v).ok());
        let max_h = attributes.get("maxH").and_then(|v| Self::parse_dimension(v).ok());
        let layout = attributes.get("layout").cloned();
        let rotate = attributes.get("rotate").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let name = attributes.get("name").cloned();
        
        XfaNode {
            kind,
            x,
            y,
            w,
            h,
            min_w,
            min_h,
            max_w,
            max_h,
            layout,
            rotate,
            margin_top: None,
            margin_bottom: None,
            margin_left: None,
            margin_right: None,
            border: None,
            font: None,
            para: None,
            name,
            attributes,
            children: Vec::new(),
        }
    }
    
    /// Extract margins from child <margin> element after parsing children
    pub fn extract_margins_from_children(&mut self) {
        for child in &self.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "margin" {
                    self.margin_top = child.attributes.get("topInset")
                        .and_then(|v| Self::parse_dimension(v).ok());
                    self.margin_bottom = child.attributes.get("bottomInset")
                        .and_then(|v| Self::parse_dimension(v).ok());
                    self.margin_left = child.attributes.get("leftInset")
                        .and_then(|v| Self::parse_dimension(v).ok());
                    self.margin_right = child.attributes.get("rightInset")
                        .and_then(|v| Self::parse_dimension(v).ok());
                    break; // Only one margin element expected
                }
            }
        }
    }
    
    /// Extract border, font, and para from child elements after parsing children
    pub fn extract_styling_from_children(&mut self) {
        // Extract margins first
        self.extract_margins_from_children();
        
        for child in &self.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                match tag_name.as_str() {
                    "border" => {
                        self.border = Some(Self::parse_border(child));
                    }
                    "font" => {
                        self.font = Some(Self::parse_font(child));
                    }
                    "para" => {
                        self.para = Some(Self::parse_para(child));
                    }
                    _ => {}
                }
            }
        }
    }
    
    /// Parse a <border> element
    fn parse_border(node: &XfaNode) -> Border {
        let mut border = Border {
            presence: node.attributes.get("presence").cloned().unwrap_or_else(|| "visible".to_string()),
            ..Default::default()
        };
        
        for child in &node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                match tag_name.as_str() {
                    "edge" => {
                        border.edges.push(Self::parse_edge(child));
                    }
                    "corner" => {
                        border.corners.push(Self::parse_corner(child));
                    }
                    "fill" => {
                        border.fill = Some(Self::parse_fill(child));
                    }
                    "margin" => {
                        border.margin_top = child.attributes.get("topInset")
                            .and_then(|v| Self::parse_dimension(v).ok());
                        border.margin_bottom = child.attributes.get("bottomInset")
                            .and_then(|v| Self::parse_dimension(v).ok());
                        border.margin_left = child.attributes.get("leftInset")
                            .and_then(|v| Self::parse_dimension(v).ok());
                        border.margin_right = child.attributes.get("rightInset")
                            .and_then(|v| Self::parse_dimension(v).ok());
                    }
                    _ => {}
                }
            }
        }
        
        border
    }
    
    /// Parse an <edge> element
    fn parse_edge(node: &XfaNode) -> Edge {
        Edge {
            thickness: node.attributes.get("thickness")
                .and_then(|v| Self::parse_dimension(v).ok()),
            stroke: node.attributes.get("stroke")
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            presence: node.attributes.get("presence")
                .cloned()
                .unwrap_or_else(|| "visible".to_string()),
            color: Self::parse_color_from_children(&node.children),
        }
    }
    
    /// Parse a <corner> element
    fn parse_corner(node: &XfaNode) -> Corner {
        Corner {
            thickness: node.attributes.get("thickness")
                .and_then(|v| Self::parse_dimension(v).ok()),
            join: node.attributes.get("join")
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            presence: node.attributes.get("presence")
                .cloned()
                .unwrap_or_else(|| "visible".to_string()),
            radius: node.attributes.get("radius")
                .and_then(|v| Self::parse_dimension(v).ok()),
        }
    }
    
    /// Parse a <fill> element
    fn parse_fill(node: &XfaNode) -> Fill {
        Fill {
            presence: node.attributes.get("presence")
                .cloned()
                .unwrap_or_else(|| "visible".to_string()),
            color: Self::parse_color_from_children(&node.children),
        }
    }
    
    /// Parse a <font> element
    /// Per XFA spec section 17: typeface defaults to Courier, size to 10pt
    /// Per XFA spec section 28: genericFamily is used for font fallback
    fn parse_font(node: &XfaNode) -> Font {
        let typeface = node.attributes.get("typeface")
            .cloned()
            .unwrap_or_else(|| "Courier".to_string());
        
        let weight_attr = node.attributes.get("weight");
        
        // Determine generic family: from attribute, or infer from typeface
        let generic_family = node.attributes.get("genericFamily")
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                // Infer generic family from common typeface names
                let tf_lower = typeface.to_lowercase();
                if tf_lower.contains("courier") || tf_lower.contains("mono") || tf_lower.contains("consolas") {
                    Some(GenericFamily::Monospace)
                } else if tf_lower.contains("times") || tf_lower.contains("georgia") || tf_lower.contains("serif") {
                    Some(GenericFamily::Serif)
                } else if tf_lower.contains("helvetica") || tf_lower.contains("arial") || tf_lower.contains("verdana") {
                    Some(GenericFamily::SansSerif)
                } else if tf_lower.contains("comic") || tf_lower.contains("script") || tf_lower.contains("cursive") {
                    Some(GenericFamily::Cursive)
                } else {
                    None // Will use default
                }
            });
        
        Font {
            typeface,
            size: node.attributes.get("size")
                .and_then(|v| Self::parse_dimension(v).ok())
                .unwrap_or_else(|| num(10.0)),
            weight: node.attributes.get("weight")
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            posture: node.attributes.get("posture")
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            underline: node.attributes.get("underline")
                .map(|s| s != "0")
                .unwrap_or(false),
            line_through: node.attributes.get("lineThrough")
                .map(|s| s != "0")
                .unwrap_or(false),
            color: Self::parse_color_from_children(&node.children),
            baseline_shift: node.attributes.get("baselineShift")
                .and_then(|v| Self::parse_dimension(v).ok()),
            // Per XFA spec: letterSpacing is a relative measurement (e.g., "0.5pt", "-0.1em")
            // that adjusts spacing between grapheme clusters. Default is 0.
            letter_spacing: node.attributes.get("letterSpacing")
                .and_then(|v| Self::parse_dimension(v).ok()),
            generic_family,
        }
    }
    
    /// Parse a <para> element
    fn parse_para(node: &XfaNode) -> Para {
        Para {
            h_align: node.attributes.get("hAlign")
                .map(|s| HAlign::from_str(s))
                .unwrap_or_default(),
            v_align: node.attributes.get("vAlign")
                .map(|s| VAlign::from_str(s))
                .unwrap_or_default(),
            line_height: node.attributes.get("lineHeight")
                .and_then(|v| Self::parse_dimension(v).ok()),
            space_above: node.attributes.get("spaceAbove")
                .and_then(|v| Self::parse_dimension(v).ok()),
            space_below: node.attributes.get("spaceBelow")
                .and_then(|v| Self::parse_dimension(v).ok()),
            text_indent: node.attributes.get("textIndent")
                .and_then(|v| Self::parse_dimension(v).ok()),
            margin_left: node.attributes.get("marginLeft")
                .and_then(|v| Self::parse_dimension(v).ok()),
            margin_right: node.attributes.get("marginRight")
                .and_then(|v| Self::parse_dimension(v).ok()),
        }
    }
    
    /// Parse color from child <color> element (value attribute like "255,0,0")
    fn parse_color_from_children(children: &[XfaNode]) -> Option<(u8, u8, u8)> {
        for child in children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "color" {
                    if let Some(value) = child.attributes.get("value") {
                        let parts: Vec<&str> = value.split(',').collect();
                        if parts.len() >= 3 {
                            let r = parts[0].trim().parse::<u8>().ok()?;
                            let g = parts[1].trim().parse::<u8>().ok()?;
                            let b = parts[2].trim().parse::<u8>().ok()?;
                            return Some((r, g, b));
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Parse dimension string to Num (handles units like pt, mm, in)
    pub fn parse_dimension(s: &str) -> Result<Num, String> {
        let s = s.trim();
        
        // Conversion constants with full precision
        let pts_per_inch = Decimal::from_str("72").unwrap();
        let pts_per_mm = Decimal::from_str("2.834645669291339").unwrap(); // 72 / 25.4 with high precision
        let pts_per_cm = Decimal::from_str("28.34645669291339").unwrap(); // 72 / 2.54 with high precision
        
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
    
    /// Parse XFA XML structure from a buffer
    pub fn parse(buffer: &[u8]) -> Result<Vec<XfaNode>, String> {
        let mut reader = Reader::from_reader(buffer);
        reader.config_mut().trim_text(true);
        
        let mut root_nodes = Vec::new();
        
        Self::parse_nodes(&mut reader, &mut root_nodes, None)?;
        
        Ok(root_nodes)
    }
    
    fn parse_nodes(
        reader: &mut Reader<&[u8]>,
        nodes: &mut Vec<XfaNode>,
        end_tag: Option<&str>,
    ) -> Result<(), String> {
        let mut buf = Vec::new();
        let mut text_content = String::new();
        
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag_name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    
                    let attributes = Self::parse_attributes(&e)?;
                    
                    let kind = match tag_name.as_str() {
                        "template" => XfaNodeKind::Template,
                        "subform" => XfaNodeKind::Subform,
                        "field" => XfaNodeKind::Field,
                        "pageSet" => XfaNodeKind::PageSet,
                        "pageArea" => XfaNodeKind::PageArea,
                        "contentArea" => XfaNodeKind::ContentArea,
                        "draw" => XfaNodeKind::Draw,
                        "value" => XfaNodeKind::Value,
                        "bind" => XfaNodeKind::Bind,
                        _ => XfaNodeKind::Element { 
                            tag_name: tag_name.clone(), 
                            text_content: None 
                        },
                    };
                    
                    let mut node = Self::new(kind, attributes);
                    
                    // Parse children (except for bind which we skip)
                    if matches!(node.kind, XfaNodeKind::Bind) {
                        Self::skip_to_end(reader, &tag_name)?;
                    } else if matches!(node.kind, XfaNodeKind::Element { .. }) {
                        let mut text = String::new();
                        Self::parse_element_content(reader, &mut node.children, &mut text, &tag_name)?;
                        
                        // Update the kind with text content if present
                        if !text.trim().is_empty() {
                            node.kind = XfaNodeKind::Element {
                                tag_name,
                                text_content: Some(text),
                            };
                        }
                    } else {
                        Self::parse_nodes(reader, &mut node.children, Some(&tag_name))?;
                    }
                    
                    // Extract styling (margins, border, font, para) from child elements
                    node.extract_styling_from_children();
                    
                    nodes.push(node);
                }
                Ok(Event::End(e)) => {
                    let tag_name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    
                    if let Some(expected_end) = end_tag {
                        if tag_name == expected_end {
                            return Ok(());
                        }
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = e.unescape()
                        .map_err(|e| format!("Failed to unescape text: {}", e))?;
                    text_content.push_str(&text);
                }
                Ok(Event::Empty(_e)) => {
                    // Self-closing tags - we can handle them if needed
                }
                Ok(Event::Eof) => {
                    return Ok(());
                }
                Err(e) => {
                    return Err(format!("Error parsing XML at position {}: {}", reader.buffer_position(), e));
                }
                _ => {}
            }
            buf.clear();
        }
    }
    
    fn parse_element_content(
        reader: &mut Reader<&[u8]>,
        children: &mut Vec<XfaNode>,
        text_content: &mut String,
        end_tag: &str,
    ) -> Result<(), String> {
        let mut buf = Vec::new();
        
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag_name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    
                    let attributes = Self::parse_attributes(&e)?;
                    
                    // Use the same tag name matching as parse_tree
                    let kind = match tag_name.as_str() {
                        "template" => XfaNodeKind::Template,
                        "subform" => XfaNodeKind::Subform,
                        "field" => XfaNodeKind::Field,
                        "pageSet" => XfaNodeKind::PageSet,
                        "pageArea" => XfaNodeKind::PageArea,
                        "contentArea" => XfaNodeKind::ContentArea,
                        "draw" => XfaNodeKind::Draw,
                        "value" => XfaNodeKind::Value,
                        "bind" => XfaNodeKind::Bind,
                        _ => XfaNodeKind::Element {
                            tag_name: tag_name.clone(),
                            text_content: None,
                        },
                    };
                    
                    let mut child_node = Self::new(kind, attributes);
                    let mut child_text = String::new();
                    
                    Self::parse_element_content(reader, &mut child_node.children, &mut child_text, &tag_name)?;
                    
                    // Update kind with text if present (only for Element type)
                    if !child_text.trim().is_empty() {
                        if let XfaNodeKind::Element { tag_name: t, .. } = &child_node.kind {
                            child_node.kind = XfaNodeKind::Element {
                                tag_name: t.clone(),
                                text_content: Some(child_text),
                            };
                        }
                    }
                    
                    // Extract styling (margins, border, font, para) from child elements
                    child_node.extract_styling_from_children();
                    
                    children.push(child_node);
                }
                Ok(Event::End(e)) => {
                    let tag_name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    
                    if tag_name == end_tag {
                        return Ok(());
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = e.unescape()
                        .map_err(|e| format!("Failed to unescape text: {}", e))?;
                    text_content.push_str(&text);
                }
                Ok(Event::Empty(e)) => {
                    // Handle self-closing tags like <contentArea ... />
                    let tag_name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    
                    let attributes = Self::parse_attributes(&e)?;
                    
                    let kind = match tag_name.as_str() {
                        "template" => XfaNodeKind::Template,
                        "subform" => XfaNodeKind::Subform,
                        "field" => XfaNodeKind::Field,
                        "pageSet" => XfaNodeKind::PageSet,
                        "pageArea" => XfaNodeKind::PageArea,
                        "contentArea" => XfaNodeKind::ContentArea,
                        "draw" => XfaNodeKind::Draw,
                        "value" => XfaNodeKind::Value,
                        "bind" => XfaNodeKind::Bind,
                        _ => XfaNodeKind::Element {
                            tag_name: tag_name.clone(),
                            text_content: None,
                        },
                    };
                    
                    let child_node = Self::new(kind, attributes);
                    children.push(child_node);
                }
                Ok(Event::Eof) => return Ok(()),
                Err(e) => {
                    return Err(format!("Error parsing XML at position {}: {}", reader.buffer_position(), e));
                }
                _ => {}
            }
            buf.clear();
        }
    }
    
    fn skip_to_end(reader: &mut Reader<&[u8]>, tag_name: &str) -> Result<(), String> {
        let mut buf = Vec::new();
        let mut depth = 1;
        
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    if name == tag_name {
                        depth += 1;
                    }
                }
                Ok(Event::End(e)) => {
                    let name = std::str::from_utf8(e.name().as_ref())
                        .map_err(|e| format!("Invalid UTF-8: {}", e))?
                        .to_string();
                    if name == tag_name {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(());
                        }
                    }
                }
                Ok(Event::Empty(_)) => {}
                Ok(Event::Eof) => return Ok(()),
                Err(e) => {
                    return Err(format!("Error skipping to end at position {}: {}", reader.buffer_position(), e));
                }
                _ => {}
            }
            buf.clear();
        }
    }
    
    fn parse_attributes<'a>(e: &'a quick_xml::events::BytesStart<'a>) -> Result<HashMap<String, String>, String> {
        let mut attributes = HashMap::new();
        
        for attr in e.attributes() {
            let attr = attr.map_err(|e| format!("Failed to parse attribute: {}", e))?;
            let key = std::str::from_utf8(attr.key.as_ref())
                .map_err(|e| format!("Invalid UTF-8 in attribute key: {}", e))?
                .to_string();
            let value = std::str::from_utf8(&attr.value)
                .map_err(|e| format!("Invalid UTF-8 in attribute value: {}", e))?
                .to_string();
            attributes.insert(key, value);
        }
        
        Ok(attributes)
    }
    
    /// Count all nodes recursively
    pub fn count_nodes(nodes: &[XfaNode]) -> usize {
        let mut count = nodes.len();
        for node in nodes {
            count += Self::count_nodes(&node.children);
        }
        count
    }
    
    /// Get a summary of the structure
    pub fn summarize_structure(nodes: &[XfaNode], indent: usize) -> String {
        let mut result = String::new();
        let indent_str = "  ".repeat(indent);
        
        for node in nodes.iter().take(10) { // Limit to first 10 at each level
            let node_type = match &node.kind {
                XfaNodeKind::Template => "Template",
                XfaNodeKind::Subform => "Subform",
                XfaNodeKind::Field => "Field",
                XfaNodeKind::PageSet => "PageSet",
                XfaNodeKind::PageArea => "PageArea",
                XfaNodeKind::ContentArea => "ContentArea",
                XfaNodeKind::Draw => "Draw",
                XfaNodeKind::Value => "Value",
                XfaNodeKind::Text { .. } => "Text",
                XfaNodeKind::Bind => "Bind",
                XfaNodeKind::Element { tag_name, .. } => tag_name,
            };
            
            let name_str = if let Some(name) = &node.name {
                format!(" '{}'", name)
            } else {
                String::new()
            };
            
            let text_str = if let XfaNodeKind::Element { text_content: Some(text), .. } = &node.kind {
                format!(" = '{}'", text.chars().take(30).collect::<String>())
            } else if let XfaNodeKind::Text { content } = &node.kind {
                format!(" = '{}'", content.chars().take(30).collect::<String>())
            } else {
                String::new()
            };
            
            result.push_str(&format!("{}{}{} ({} children){}\n", 
                indent_str, node_type, name_str, node.children.len(), text_str));
            
            if indent < 3 && !node.children.is_empty() {
                result.push_str(&Self::summarize_structure(&node.children, indent + 1));
            }
        }
        
        result
    }
    
    /// Recursively find all nodes matching a predicate
    pub fn find_nodes_by_type<'a, F>(&'a self, predicate: &F, results: &mut Vec<&'a XfaNode>)
    where
        F: Fn(&XfaNode) -> bool,
    {
        if predicate(self) {
            results.push(self);
        }
        
        for child in &self.children {
            child.find_nodes_by_type(predicate, results);
        }
    }
}