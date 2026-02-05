mod merger;
mod structured_converter;

pub use merger::{MergeInput, RecursiveMerger, Selection};
pub use structured_converter::convert;

use rust_decimal::Decimal;
use serde::Serialize;

use crate::xfa::scripting::SomPath;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StructuredNode {
    Heading(HeadingNode),
    Paragraph(ParagraphNode),
    Image(ImageNode),
    Table(TableNode),
    Field(FieldNode),
    Repeatable(RepeatableNode),
    Group(GroupNode),
    Conditional(ConditionalNode),
    Empty,
    GridLayout(GridLayout),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridLayout {
    pub columns: usize,
    pub elements: Vec<GridLayoutElement>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridLayoutElement {
    pub span: usize,
    pub node: StructuredNode,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableNode {
    pub header: Option<TableHeader>,
    pub rows: Vec<TableRow>,
    pub caption: Option<InlineText>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub cells: Vec<StructuredNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableHeader {
    pub cells: Vec<StructuredNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageNode {
    #[serde(skip)]
    pub content: Vec<u8>,
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupNode {
    pub children: Vec<StructuredNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatableNode {
    pub item: Box<StructuredNode>,
    pub min_occurrences: u32,
    pub max_occurrences: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionalNode {
    pub condition: FieldCondition,
    pub content: Box<StructuredNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldCondition {
    pub field_name: SomPath,
    pub value: InputValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum InputValue {
    Text(String),
    Number(Decimal),
    Bool(bool),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FieldType {
    Text {
        regex: Option<String>,
        max_length: Option<usize>,
        min_length: Option<usize>,
    },
    Number {
        min: Option<Decimal>,
        max: Option<Decimal>,
        step: Option<Decimal>,
    },
    Date,
    Email,
    Tel,
    Bool,
    Radio {
        options: Vec<NameValue>,
    },
    Select {
        options: Vec<NameValue>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NameValue {
    pub name: String,
    pub value: InputValue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldNode {
    pub name: String,
    pub label: Option<InlineText>,
    pub input_type: FieldType,
    pub value: Option<InputValue>,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphNode {
    pub content: InlineText,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadingNode {
    pub level: HeadingLevel,
    pub content: InlineText,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct InlineText(pub Vec<InlineNode>);

impl InlineText {
    /// Create an empty inline text
    pub fn empty() -> Self {
        InlineText(Vec::new())
    }

    /// Create inline text from a plain string
    pub fn plain(text: impl Into<String>) -> Self {
        InlineText(vec![InlineNode::Text(text.into())])
    }

    /// Create inline text from nodes, consolidating consecutive nodes of the same type
    pub fn new(nodes: Vec<InlineNode>) -> Self {
        let mut result = InlineText(nodes);
        result.consolidate();
        result
    }

    /// Consolidate consecutive InlineNodes of the same type into single nodes
    pub fn consolidate(&mut self) {
        if self.0.len() <= 1 {
            return;
        }

        let nodes = std::mem::take(&mut self.0);
        let mut consolidated = Vec::with_capacity(nodes.len());
        let mut iter = nodes.into_iter();

        if let Some(mut current) = iter.next() {
            for next in iter {
                let merged = match (&mut current, &next) {
                    // Merge consecutive Text nodes
                    (InlineNode::Text(text), InlineNode::Text(next_text)) => {
                        text.push_str(next_text);
                        true
                    }
                    // Merge consecutive Strong nodes with Text content
                    (InlineNode::Strong(inner), InlineNode::Strong(next_inner)) => {
                        if let (InlineNode::Text(text), InlineNode::Text(next_text)) =
                            (inner.as_mut(), next_inner.as_ref())
                        {
                            text.push_str(next_text);
                            true
                        } else {
                            false
                        }
                    }
                    // Merge consecutive Emphasis nodes with Text content
                    (InlineNode::Emphasis(inner), InlineNode::Emphasis(next_inner)) => {
                        if let (InlineNode::Text(text), InlineNode::Text(next_text)) =
                            (inner.as_mut(), next_inner.as_ref())
                        {
                            text.push_str(next_text);
                            true
                        } else {
                            false
                        }
                    }
                    // Different types
                    _ => false,
                };

                if !merged {
                    consolidated.push(current);
                    current = next;
                }
            }
            consolidated.push(current);
        }

        self.0 = consolidated;
    }

    /// Check if the inline text is empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
            || self.0.iter().all(|node| match node {
                InlineNode::Text(s) => s.is_empty(),
                _ => false,
            })
    }

    /// Get the plain text content (stripping formatting)
    pub fn as_plain_text(&self) -> String {
        fn collect_text(node: &InlineNode, out: &mut String) {
            match node {
                InlineNode::Text(s) => out.push_str(s),
                InlineNode::Link(link) => {
                    for child in &link.content.0 {
                        collect_text(child, out);
                    }
                }
                InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
                    collect_text(inner, out);
                }
            }
        }
        let mut result = String::new();
        for node in &self.0 {
            collect_text(node, &mut result);
        }
        result
    }
}

impl Default for InlineText {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "content", rename_all = "camelCase")]
pub enum InlineNode {
    Text(String),
    Link(LinkNode),
    Strong(Box<InlineNode>),
    Emphasis(Box<InlineNode>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkNode {
    pub href: String,
    pub content: InlineText,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadingLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

impl HeadingLevel {
    /// Create a heading level from a u8 (clamped to 1-6)
    pub fn from_u8(level: u8) -> Self {
        match level {
            1 => HeadingLevel::H1,
            2 => HeadingLevel::H2,
            3 => HeadingLevel::H3,
            4 => HeadingLevel::H4,
            5 => HeadingLevel::H5,
            _ => HeadingLevel::H6,
        }
    }

    /// Get the numeric level (1-6)
    pub fn as_u8(&self) -> u8 {
        match self {
            HeadingLevel::H1 => 1,
            HeadingLevel::H2 => 2,
            HeadingLevel::H3 => 3,
            HeadingLevel::H4 => 4,
            HeadingLevel::H5 => 5,
            HeadingLevel::H6 => 6,
        }
    }
}

// ============================================================================
// Structural Equality
// ============================================================================
//
// Structural equality compares nodes by their type, field names, and text content,
// ignoring field values. This is used for merging multiple form states.

impl StructuredNode {
    /// Check if two nodes are structurally equal.
    ///
    /// Structural equality compares:
    /// - Node type (variant)
    /// - Field names
    /// - Text content (for text-bearing nodes like Paragraph, Heading)
    /// - Heading level
    /// - Child structure (recursively)
    ///
    /// It does NOT compare:
    /// - Field values (InputValue)
    /// - Image content
    pub fn structural_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StructuredNode::Heading(a), StructuredNode::Heading(b)) => {
                a.level.as_u8() == b.level.as_u8() && a.content.structural_eq(&b.content)
            }
            (StructuredNode::Paragraph(a), StructuredNode::Paragraph(b)) => {
                a.content.structural_eq(&b.content)
            }
            (StructuredNode::Image(a), StructuredNode::Image(b)) => {
                // Compare by alt text only (content is binary data)
                a.alt_text == b.alt_text
            }
            (StructuredNode::Table(a), StructuredNode::Table(b)) => a.structural_eq(b),
            (StructuredNode::Field(a), StructuredNode::Field(b)) => a.structural_eq(b),
            (StructuredNode::Repeatable(a), StructuredNode::Repeatable(b)) => {
                a.min_occurrences == b.min_occurrences
                    && a.max_occurrences == b.max_occurrences
                    && a.item.structural_eq(&b.item)
            }
            (StructuredNode::Group(a), StructuredNode::Group(b)) => {
                a.children.len() == b.children.len()
                    && a.children
                        .iter()
                        .zip(b.children.iter())
                        .all(|(ca, cb)| ca.structural_eq(cb))
            }
            (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) => {
                // Conditionals are structurally equal if their contents are
                a.content.structural_eq(&b.content)
            }
            (StructuredNode::Empty, StructuredNode::Empty) => true,
            (StructuredNode::GridLayout(a), StructuredNode::GridLayout(b)) => {
                a.columns == b.columns
                    && a.elements.len() == b.elements.len()
                    && a.elements
                        .iter()
                        .zip(b.elements.iter())
                        .all(|(ea, eb)| ea.span == eb.span && ea.node.structural_eq(&eb.node))
            }
            // Different variants are never structurally equal
            _ => false,
        }
    }

    /// Get a structural discriminant for this node type.
    /// Used for quick inequality checks before deep comparison.
    pub fn structural_discriminant(&self) -> u8 {
        match self {
            StructuredNode::Heading(_) => 0,
            StructuredNode::Paragraph(_) => 1,
            StructuredNode::Image(_) => 2,
            StructuredNode::Table(_) => 3,
            StructuredNode::Field(_) => 4,
            StructuredNode::Repeatable(_) => 5,
            StructuredNode::Group(_) => 6,
            StructuredNode::Conditional(_) => 7,
            StructuredNode::Empty => 8,
            StructuredNode::GridLayout(_) => 9,
        }
    }
}

impl InlineText {
    /// Check if two InlineText are structurally equal (compare text content)
    pub fn structural_eq(&self, other: &Self) -> bool {
        // For text content, the actual text IS the structure
        self.as_plain_text() == other.as_plain_text()
    }
}

impl FieldNode {
    /// Check if two fields are structurally equal.
    /// Compares name, label, and input type structure, but NOT value.
    pub fn structural_eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.label.as_ref().map(|l| l.as_plain_text())
                == other.label.as_ref().map(|l| l.as_plain_text())
            && self.input_type.structural_eq(&other.input_type)
            && self.placeholder == other.placeholder
    }
}

impl FieldType {
    /// Check if two field types are structurally equal.
    pub fn structural_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                FieldType::Text {
                    regex: r1,
                    max_length: max1,
                    min_length: min1,
                },
                FieldType::Text {
                    regex: r2,
                    max_length: max2,
                    min_length: min2,
                },
            ) => r1 == r2 && max1 == max2 && min1 == min2,
            (
                FieldType::Number {
                    min: min1,
                    max: max1,
                    step: step1,
                },
                FieldType::Number {
                    min: min2,
                    max: max2,
                    step: step2,
                },
            ) => min1 == min2 && max1 == max2 && step1 == step2,
            (FieldType::Date, FieldType::Date) => true,
            (FieldType::Email, FieldType::Email) => true,
            (FieldType::Tel, FieldType::Tel) => true,
            (FieldType::Bool, FieldType::Bool) => true,
            (
                FieldType::Radio { options: opts1 },
                FieldType::Radio { options: opts2 },
            ) => opts1 == opts2,
            (
                FieldType::Select { options: opts1 },
                FieldType::Select { options: opts2 },
            ) => opts1 == opts2,
            _ => false,
        }
    }
}

impl TableNode {
    /// Check if two tables are structurally equal.
    pub fn structural_eq(&self, other: &Self) -> bool {
        // Compare header structure
        let header_eq = match (&self.header, &other.header) {
            (None, None) => true,
            (Some(h1), Some(h2)) => {
                h1.cells.len() == h2.cells.len()
                    && h1
                        .cells
                        .iter()
                        .zip(h2.cells.iter())
                        .all(|(c1, c2)| c1.structural_eq(c2))
            }
            _ => false,
        };

        // Compare row structure
        let rows_eq = self.rows.len() == other.rows.len()
            && self.rows.iter().zip(other.rows.iter()).all(|(r1, r2)| {
                r1.cells.len() == r2.cells.len()
                    && r1
                        .cells
                        .iter()
                        .zip(r2.cells.iter())
                        .all(|(c1, c2)| c1.structural_eq(c2))
            });

        // Compare caption
        let caption_eq = self.caption.as_ref().map(|c| c.as_plain_text())
            == other.caption.as_ref().map(|c| c.as_plain_text());

        header_eq && rows_eq && caption_eq
    }
}
