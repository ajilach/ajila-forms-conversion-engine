pub enum StructuredNode {
    HeadingNode(HeadingNode),
    ParagraphNode(ParagraphNode),
    InlineNode(InlineNode),
    FieldNode(FieldNode),
}

pub struct FieldNode {
    pub label: String,
}

pub struct ParagraphNode {
    pub content: Vec<InlineNode>,
}

pub struct HeadingNode {
    pub level: HeadingLevel,
    pub content: Vec<InlineNode>,
}

pub enum InlineNode {
    TextNode(String),
    LinkNode(InlineNode),
    StrongNode(InlineNode),
    EmphasisNode(InlineNode),
}

pub enum HeadingLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
}