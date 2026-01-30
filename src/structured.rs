pub enum StructuredNode {
    HeadingNode(HeadingNode),
    ParagraphNode(ParagraphNode),
    FieldNode(FieldNode),
}

pub struct FieldNode {
    pub label: InlineText,
}

pub struct ParagraphNode {
    pub content: InlineText,
}

pub struct HeadingNode {
    pub level: HeadingLevel,
    pub content: InlineText,
}

pub struct InlineText(Vec<InlineNode>);

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