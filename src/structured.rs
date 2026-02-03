use rust_decimal::Decimal;
use serde::Serialize;

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
    pub field_name: String,
    pub value: InputValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum InputValue {
    Text(String),
    Number(Decimal),
    Date(String),
    Email(String),
    Tel(String),
    Checkbox(bool),
    Radio(String),
    Select(String),
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
    Checkbox,
    Radio {
        options: Vec<String>,
        /// Internal field names corresponding to each option (e.g., ["RB_1", "RB_2", "RB_3"])
        /// Used to map from internal names to option labels in conditionals
        #[serde(skip_serializing_if = "Option::is_none")]
        option_names: Option<Vec<String>>,
    },
    Select {
        options: Vec<String>,
    },
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
