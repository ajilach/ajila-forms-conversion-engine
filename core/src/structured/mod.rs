mod merger;
mod structured_converter;
mod translation_merger;

pub use merger::{MergeInput, RecursiveMerger, Selection, SelectionKind};
pub use structured_converter::{convert, convert_with_context};
pub use translation_merger::{MergeError, merge_translations};

use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::context::Context;
use crate::xfa::scripting::SomPath;

// ============================================================================
// FieldId — deterministic UUID derived from SOM path
// ============================================================================

/// Namespace UUID used for deterministic FieldId generation (UUID v5).
const NAMESPACE_FIELD_ID: Uuid = Uuid::from_bytes([
    0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x47, 0x89, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78,
]);

/// A deterministic field identifier derived from a SOM path.
///
/// `FieldId` wraps a UUID v5 that is computed from the field's SOM path using
/// a fixed namespace. Two fields with the same SOM path always produce the
/// same `FieldId`, making output reproducible across runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldId(Uuid);

impl FieldId {
    /// Create a `FieldId` by hashing a `SomPath` into a deterministic UUID v5.
    pub fn from_som_path(path: &SomPath) -> Self {
        Self(Uuid::new_v5(&NAMESPACE_FIELD_ID, path.as_str().as_bytes()))
    }

    /// Get the underlying UUID.
    pub fn uuid(&self) -> &Uuid {
        &self.0
    }
}

impl std::fmt::Display for FieldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for FieldId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl From<&SomPath> for FieldId {
    fn from(path: &SomPath) -> Self {
        Self::from_som_path(path)
    }
}

impl From<SomPath> for FieldId {
    fn from(path: SomPath) -> Self {
        Self::from_som_path(&path)
    }
}

impl From<&str> for FieldId {
    fn from(s: &str) -> Self {
        Self::from_som_path(&SomPath::new(s))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StructuredNode {
    Heading(HeadingNode),
    Paragraph(ParagraphNode),
    Image(ImageNode),
    Table(TableNode),
    Field(FieldNode),
    //UnorderedList(UnorderedListNode),
    //OrderedList(OrderedListNode),
    Repeatable(RepeatableNode),
    Group(GroupNode),
    Conditional(ConditionalNode),
    Empty,
    GridLayout(GridLayout),
    List(ListNode),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNode {
    pub list_style: crate::document::ListStyleType,
    pub items: Vec<InlineText>,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldCondition {
    pub field_name: FieldId,
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
    pub name: TranslatableString,
    pub value: InputValue,
}

/// A string that can have translations
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TranslatableString {
    Plain(String),
    Translated(std::collections::HashMap<String, String>),
}

impl TranslatableString {
    /// Get the string in the specified language, or the first available
    pub fn get(&self, lang: &str) -> Option<&str> {
        match self {
            TranslatableString::Plain(s) => Some(s),
            TranslatableString::Translated(map) => map
                .get(lang)
                .map(|s| s.as_str())
                .or_else(|| map.values().next().map(|s| s.as_str())),
        }
    }

    /// Get the string in the specified language, or the first available, or empty string
    pub fn get_or_default(&self, lang: &str) -> &str {
        self.get(lang).unwrap_or("")
    }

    /// Returns the plain string if Plain, or the first available translation.
    /// Useful in tests that work with single-language documents.
    pub fn as_str(&self) -> &str {
        match self {
            TranslatableString::Plain(s) => s.as_str(),
            TranslatableString::Translated(map) => {
                map.values().next().map(|s| s.as_str()).unwrap_or("")
            }
        }
    }

    /// Check if any contained string contains the given substring.
    pub fn contains(&self, needle: &str) -> bool {
        match self {
            TranslatableString::Plain(s) => s.contains(needle),
            TranslatableString::Translated(map) => map.values().any(|s| s.contains(needle)),
        }
    }

    /// Merge two `TranslatableString` values, combining their translations into a
    /// single `Translated` map. `Plain` values are inserted under their respective
    /// language keys. Already-`Translated` maps are merged directly.
    pub fn merge(&self, self_lang: &str, other: &Self, other_lang: &str) -> Self {
        let mut map = std::collections::HashMap::new();
        match self {
            TranslatableString::Plain(s) => {
                map.insert(self_lang.to_string(), s.clone());
            }
            TranslatableString::Translated(m) => {
                map.extend(m.clone());
            }
        }
        match other {
            TranslatableString::Plain(s) => {
                map.insert(other_lang.to_string(), s.clone());
            }
            TranslatableString::Translated(m) => {
                map.extend(m.clone());
            }
        }
        TranslatableString::Translated(map)
    }
}

impl std::fmt::Display for TranslatableString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranslatableString::Plain(s) => write!(f, "{}", s),
            TranslatableString::Translated(map) => {
                // Display first available value
                if let Some(s) = map.values().next() {
                    write!(f, "{}", s)
                } else {
                    Ok(())
                }
            }
        }
    }
}

impl From<String> for TranslatableString {
    fn from(s: String) -> Self {
        TranslatableString::Plain(s)
    }
}

impl From<&str> for TranslatableString {
    fn from(s: &str) -> Self {
        TranslatableString::Plain(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldNode {
    pub name: FieldId,
    #[serde(skip)]
    pub som_path: Option<SomPath>,
    pub label: Option<InlineText>,
    pub input_type: FieldType,
    pub value: Option<InputValue>,
    pub placeholder: Option<TranslatableString>,
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
                InlineNode::TranslatedText(translations) => {
                    // For plain text, use the first available translation
                    if let Some(text) = translations.values().next() {
                        out.push_str(text);
                    }
                }
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

    /// Get the plain text content in a specific language (stripping formatting).
    ///
    /// For `TranslatedText` nodes, prefers the given `lang`; falls back to
    /// the first available translation if the language is not present.
    pub fn plain_text_in(&self, lang: &str) -> String {
        fn collect_text(node: &InlineNode, lang: &str, out: &mut String) {
            match node {
                InlineNode::Text(s) => out.push_str(s),
                InlineNode::TranslatedText(translations) => {
                    let text = translations
                        .get(lang)
                        .or_else(|| translations.values().next());
                    if let Some(text) = text {
                        out.push_str(text);
                    }
                }
                InlineNode::Link(link) => {
                    for child in &link.content.0 {
                        collect_text(child, lang, out);
                    }
                }
                InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
                    collect_text(inner, lang, out);
                }
            }
        }
        let mut result = String::new();
        for node in &self.0 {
            collect_text(node, lang, &mut result);
        }
        result
    }

    /// Collect all language codes from `TranslatedText` nodes in this inline text.
    pub fn collect_languages(&self, langs: &mut BTreeSet<String>) {
        fn walk(node: &InlineNode, langs: &mut BTreeSet<String>) {
            match node {
                InlineNode::TranslatedText(map) => {
                    langs.extend(map.keys().cloned());
                }
                InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
                    walk(inner, langs);
                }
                InlineNode::Link(link) => {
                    for child in &link.content.0 {
                        walk(child, langs);
                    }
                }
                InlineNode::Text(_) => {}
            }
        }
        for node in &self.0 {
            walk(node, langs);
        }
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
    TranslatedText(std::collections::HashMap<String, String>),
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

/// Controls what is compared when checking structural equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareMode {
    /// Full structural comparison including text content.
    Full,
    /// Ignore text content (for translation merging where structure matches
    /// but text differs by language).
    IgnoreText,
}

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
        self.structural_cmp(other, CompareMode::Full)
    }

    /// Check if two nodes are structurally equal, ignoring all text content.
    ///
    /// This is used for translation merging, where the same document in different
    /// languages has identical structure but different text. It compares:
    /// - Node type (variant)
    /// - Heading level
    /// - Field names and input type structure (but NOT labels, placeholders, text)
    /// - Children count and structure (recursively)
    ///
    /// It does NOT compare:
    /// - Any text content (Paragraph, Heading, InlineText, captions)
    /// - Field labels and placeholders (may be translated)
    /// - Radio/Select option names (may be translated)
    /// - Field values
    /// - Image content
    pub fn structural_eq_ignore_text(&self, other: &Self) -> bool {
        self.structural_cmp(other, CompareMode::IgnoreText)
    }

    /// Unified structural comparison parameterized by [`CompareMode`].
    fn structural_cmp(&self, other: &Self, mode: CompareMode) -> bool {
        match (self, other) {
            (StructuredNode::Heading(a), StructuredNode::Heading(b)) => {
                a.level.as_u8() == b.level.as_u8()
                    && (mode == CompareMode::IgnoreText || a.content.structural_eq(&b.content))
            }
            (StructuredNode::Paragraph(a), StructuredNode::Paragraph(b)) => {
                // In IgnoreText mode all paragraphs match (text differs by language)
                mode == CompareMode::IgnoreText || a.content.structural_eq(&b.content)
            }
            (StructuredNode::Image(a), StructuredNode::Image(b)) => a.alt_text == b.alt_text,
            (StructuredNode::Table(a), StructuredNode::Table(b)) => a.structural_cmp(b, mode),
            (StructuredNode::Field(a), StructuredNode::Field(b)) => a.structural_eq(b),
            (StructuredNode::Repeatable(a), StructuredNode::Repeatable(b)) => {
                a.min_occurrences == b.min_occurrences
                    && a.max_occurrences == b.max_occurrences
                    && a.item.structural_cmp(&b.item, mode)
            }
            (StructuredNode::Group(a), StructuredNode::Group(b)) => {
                a.children.len() == b.children.len()
                    && a.children
                        .iter()
                        .zip(b.children.iter())
                        .all(|(ca, cb)| ca.structural_cmp(cb, mode))
            }
            (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) => {
                a.content.structural_cmp(&b.content, mode)
            }
            (StructuredNode::Empty, StructuredNode::Empty) => true,
            (StructuredNode::GridLayout(a), StructuredNode::GridLayout(b)) => {
                a.columns == b.columns
                    && a.elements.len() == b.elements.len()
                    && a.elements.iter().zip(b.elements.iter()).all(|(ea, eb)| {
                        ea.span == eb.span && ea.node.structural_cmp(&eb.node, mode)
                    })
            }
            (StructuredNode::List(a), StructuredNode::List(b)) => {
                a.list_style == b.list_style
                    && a.items.len() == b.items.len()
                    && (mode == CompareMode::IgnoreText
                        || a.items
                            .iter()
                            .zip(b.items.iter())
                            .all(|(ia, ib)| ia.structural_eq(ib)))
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
            StructuredNode::List(_) => 10,
        }
    }

    /// Collect all language codes used in translatable content within this node tree.
    ///
    /// Recursively walks the node and its children, gathering language keys from
    /// `TranslatedText` inline nodes and `TranslatableString::Translated` maps.
    pub fn collect_languages(&self, langs: &mut BTreeSet<String>) {
        match self {
            StructuredNode::Heading(h) => h.content.collect_languages(langs),
            StructuredNode::Paragraph(p) => p.content.collect_languages(langs),
            StructuredNode::Field(f) => {
                if let Some(label) = &f.label {
                    label.collect_languages(langs);
                }
                if let Some(TranslatableString::Translated(map)) = &f.placeholder {
                    langs.extend(map.keys().cloned());
                }
                match &f.input_type {
                    FieldType::Radio { options } | FieldType::Select { options } => {
                        for opt in options {
                            if let TranslatableString::Translated(map) = &opt.name {
                                langs.extend(map.keys().cloned());
                            }
                        }
                    }
                    _ => {}
                }
            }
            StructuredNode::Table(t) => {
                if let Some(caption) = &t.caption {
                    caption.collect_languages(langs);
                }
                if let Some(header) = &t.header {
                    for cell in &header.cells {
                        cell.collect_languages(langs);
                    }
                }
                for row in &t.rows {
                    for cell in &row.cells {
                        cell.collect_languages(langs);
                    }
                }
            }
            StructuredNode::Group(g) => {
                for child in &g.children {
                    child.collect_languages(langs);
                }
            }
            StructuredNode::Repeatable(r) => r.item.collect_languages(langs),
            StructuredNode::Conditional(c) => c.content.collect_languages(langs),
            StructuredNode::GridLayout(g) => {
                for elem in &g.elements {
                    elem.node.collect_languages(langs);
                }
            }
            StructuredNode::List(l) => {
                for item in &l.items {
                    item.collect_languages(langs);
                }
            }
            _ => {}
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
    /// Returns the SOM path string of this field, or an empty string if unavailable.
    pub fn som_path_str(&self) -> &str {
        self.som_path.as_ref().map(|p| p.as_str()).unwrap_or("")
    }

    /// Check if two fields are structurally equal.
    /// Compares name and input type structure, but NOT label text or value.
    /// Labels may differ across languages, so we ignore them.
    pub fn structural_eq(&self, other: &Self) -> bool {
        self.name == other.name && self.input_type.structural_eq(&other.input_type)
        // Note: placeholder comparison removed as they may be translated
    }
}

impl FieldType {
    /// Check if two field types are structurally equal.
    /// For Radio/Select options, we compare by value only (names may be translated).
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
            (FieldType::Radio { options: opts1 }, FieldType::Radio { options: opts2 }) => {
                // Compare only by values (names may differ due to translation)
                opts1.len() == opts2.len()
                    && opts1
                        .iter()
                        .zip(opts2.iter())
                        .all(|(o1, o2)| o1.value == o2.value)
            }
            (FieldType::Select { options: opts1 }, FieldType::Select { options: opts2 }) => {
                // Compare only by values (names may differ due to translation)
                opts1.len() == opts2.len()
                    && opts1
                        .iter()
                        .zip(opts2.iter())
                        .all(|(o1, o2)| o1.value == o2.value)
            }
            _ => false,
        }
    }
}

impl TableNode {
    /// Check if two tables are structurally equal.
    pub fn structural_eq(&self, other: &Self) -> bool {
        self.structural_cmp(other, CompareMode::Full)
    }

    /// Check if two tables are structurally equal, ignoring text content.
    /// Used for translation merging.
    pub fn structural_eq_ignore_text(&self, other: &Self) -> bool {
        self.structural_cmp(other, CompareMode::IgnoreText)
    }

    /// Unified structural comparison parameterized by [`CompareMode`].
    fn structural_cmp(&self, other: &Self, mode: CompareMode) -> bool {
        // Compare header structure
        let header_eq = match (&self.header, &other.header) {
            (None, None) => true,
            (Some(h1), Some(h2)) => {
                h1.cells.len() == h2.cells.len()
                    && h1
                        .cells
                        .iter()
                        .zip(h2.cells.iter())
                        .all(|(c1, c2)| c1.structural_cmp(c2, mode))
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
                        .all(|(c1, c2)| c1.structural_cmp(c2, mode))
            });

        // Caption is only compared in Full mode
        let caption_eq = mode == CompareMode::IgnoreText
            || self.caption.as_ref().map(|c| c.as_plain_text())
                == other.caption.as_ref().map(|c| c.as_plain_text());

        header_eq && rows_eq && caption_eq
    }
}

/// Document envelope containing the structured content and context metadata.
///
/// This is the top-level structure that wraps the document's structured nodes
/// along with the processing context that was enriched throughout the pipeline.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentEnvelope {
    /// Context metadata enriched throughout processing
    pub context: Context,

    /// The structured document content
    pub content: Vec<StructuredNode>,
}
