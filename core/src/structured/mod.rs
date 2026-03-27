mod merge_engine;
mod merger;
mod structured_converter;
mod translation_merger;

pub use merger::{MergeInput, RecursiveMerger, Selection, SelectionKind};
pub use structured_converter::{convert, convert_with_context};
pub use translation_merger::{MergeError, calculate_structural_similarity, merge_translations};

use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use crate::context::Context;
use crate::xfa::scripting::SomPath;

// ── Semantic matching context (feature-gated) ────────────────────────────────

/// Opaque semantic matching context threaded through translation merge.
///
/// When the `semantic-matching` feature is enabled, this is an alias for
/// [`crate::semantic::SemanticMatcher`].  Otherwise it is a zero-sized dummy
/// type so that function signatures remain identical in both configurations.
#[cfg(feature = "semantic-matching")]
pub type SemanticCtx = crate::semantic::SemanticMatcher;

/// Dummy zero-sized type when semantic matching is not available.
#[cfg(not(feature = "semantic-matching"))]
pub struct SemanticCtx;

/// Check whether a space separator is needed between two adjacent text
/// segments that are being concatenated.  Returns `true` when neither side
/// already provides whitespace at the boundary.
pub(crate) fn needs_separator(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let l = left.as_bytes().last().copied().unwrap_or(b' ');
    let r = right.as_bytes().first().copied().unwrap_or(b' ');
    !l.is_ascii_whitespace() && !r.is_ascii_whitespace()
}

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
    MultiColumnLayout(MultiColumnLayout),
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

/// A multi-column layout of non-interactive content (text, headings, lists, etc.).
///
/// Each inner `Vec<StructuredNode>` represents one column, with nodes in
/// top-to-bottom reading order.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiColumnLayout {
    /// Number of columns (equals `columns.len()`)
    pub num_columns: usize,
    /// Content for each column, in left-to-right column order
    pub columns: Vec<Vec<StructuredNode>>,
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

    /// Structural text comparison with language-aware semantics.
    ///
    /// Rules:
    /// - Plain vs Plain: direct string equality.
    /// - Translated vs Translated: at least one shared language key must exist
    ///   and have the same value.
    /// - Plain vs Translated: treated as a plain-text fallback and considered
    ///   equal when any translated value matches the plain text.
    pub fn structural_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TranslatableString::Plain(a), TranslatableString::Plain(b)) => a == b,
            (TranslatableString::Translated(a), TranslatableString::Translated(b)) => {
                translated_maps_match_on_shared_language(a, b)
            }
            (TranslatableString::Plain(a), TranslatableString::Translated(b))
            | (TranslatableString::Translated(b), TranslatableString::Plain(a)) => {
                b.values().any(|value| value == a)
            }
        }
    }
}

fn translated_maps_match_on_shared_language(
    left: &HashMap<String, String>,
    right: &HashMap<String, String>,
) -> bool {
    left.iter()
        .filter_map(|(lang, left_text)| right.get(lang).map(|right_text| (left_text, right_text)))
        .any(|(left_text, right_text)| left_text == right_text)
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
    #[serde(skip)]
    pub som_path: Option<SomPath>,
    #[serde(skip)]
    pub source_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadingNode {
    pub level: HeadingLevel,
    pub content: InlineText,
    #[serde(skip)]
    pub som_path: Option<SomPath>,
    #[serde(skip)]
    pub source_name: Option<String>,
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

    /// Return a new `InlineText` with `Strong` and `Emphasis` wrappers removed,
    /// keeping only the inner text content. Adjacent text nodes are consolidated.
    pub fn to_plain(&self) -> Self {
        fn strip(node: &InlineNode) -> InlineNode {
            match node {
                InlineNode::Text(_) | InlineNode::TranslatedText(_) => node.clone(),
                InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => strip(inner),
                InlineNode::Link(link) => InlineNode::Link(LinkNode {
                    href: link.href.clone(),
                    content: link.content.to_plain(),
                }),
            }
        }
        InlineText::new(self.0.iter().map(strip).collect())
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

impl InlineNode {
    /// Return the trailing plain-text content of this node (if any).
    ///
    /// For `TranslatedText` nodes, returns the first value that would cause
    /// a separator to be needed (i.e., does not end with whitespace). Falls
    /// back to the first available value.
    pub(crate) fn trailing_text(&self) -> Option<&str> {
        match self {
            InlineNode::Text(s) => Some(s.as_str()),
            InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => inner.trailing_text(),
            InlineNode::TranslatedText(map) => {
                // Prefer a value that doesn't end with whitespace (worst case
                // for separator decisions) to avoid nondeterminism.
                map.values()
                    .find(|s| {
                        !s.is_empty() && !s.as_bytes().last().unwrap_or(&b' ').is_ascii_whitespace()
                    })
                    .or_else(|| map.values().find(|s| !s.is_empty()))
                    .map(|s| s.as_str())
            }
            InlineNode::Link(link) => link.content.0.last().and_then(|n| n.trailing_text()),
        }
    }

    /// Return the leading plain-text content of this node (if any).
    ///
    /// For `TranslatedText` nodes, returns the first value that would cause
    /// a separator to be needed (i.e., does not start with whitespace). Falls
    /// back to the first available value.
    pub(crate) fn leading_text(&self) -> Option<&str> {
        match self {
            InlineNode::Text(s) => Some(s.as_str()),
            InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => inner.leading_text(),
            InlineNode::TranslatedText(map) => {
                // Prefer a value that doesn't start with whitespace (worst case
                // for separator decisions) to avoid nondeterminism.
                map.values()
                    .find(|s| {
                        !s.is_empty()
                            && !s.as_bytes().first().unwrap_or(&b' ').is_ascii_whitespace()
                    })
                    .or_else(|| map.values().find(|s| !s.is_empty()))
                    .map(|s| s.as_str())
            }
            InlineNode::Link(link) => link.content.0.first().and_then(|n| n.leading_text()),
        }
    }
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
    /// Returns the SOM path of this node, if it carries one.
    ///
    /// SOM paths are available on Field, Paragraph, and Heading nodes.
    pub fn som_path(&self) -> Option<&SomPath> {
        match self {
            StructuredNode::Field(f) => f.som_path.as_ref(),
            StructuredNode::Paragraph(p) => p.som_path.as_ref(),
            StructuredNode::Heading(h) => h.som_path.as_ref(),
            StructuredNode::Conditional(c) => c.content.som_path(),
            _ => None,
        }
    }

    /// Returns the best available language-independent anchor key for this node.
    ///
    /// Prefers SOM path when available, falls back to `source_name` (the XFA
    /// draw node `name` attribute, which is language-independent for same-template
    /// forms).
    ///
    /// For container nodes (Group, Repeatable, GridLayout, Table) that lack
    /// their own SOM path, a key is derived from the first anchored child.
    /// These derived keys are prefixed with a type tag (e.g. `g:`, `r:`) to
    /// prevent collisions with direct SOM-path anchors at the same list level.
    pub fn anchor_key(&self) -> Option<String> {
        if let Some(sp) = self.som_path() {
            return Some(sp.as_str().to_owned());
        }
        match self {
            StructuredNode::Paragraph(p) => p.source_name.clone(),
            StructuredNode::Heading(h) => h.source_name.clone(),
            StructuredNode::Group(g) => g
                .children
                .iter()
                .find_map(|c| c.anchor_key())
                .map(|k| format!("g:{k}")),
            StructuredNode::Repeatable(r) => r.item.anchor_key().map(|k| format!("r:{k}")),
            StructuredNode::GridLayout(gl) => gl
                .elements
                .iter()
                .find_map(|e| e.node.anchor_key())
                .map(|k| format!("gl:{k}")),
            StructuredNode::Table(t) => t
                .header
                .as_ref()
                .and_then(|h| h.cells.iter().find_map(|c| c.anchor_key()))
                .or_else(|| {
                    t.rows
                        .first()
                        .and_then(|r| r.cells.iter().find_map(|c| c.anchor_key()))
                })
                .map(|k| format!("t:{k}")),
            StructuredNode::MultiColumnLayout(mc) => mc
                .columns
                .iter()
                .flat_map(|col| col.iter())
                .find_map(|n| n.anchor_key())
                .map(|k| format!("mc:{k}")),
            _ => None,
        }
    }

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
            (StructuredNode::Field(a), StructuredNode::Field(b)) => {
                // In IgnoreText mode (used for translation merging), Fields match by
                // input type structure only — FieldIds are derived from SOM paths which
                // can differ across languages for the same logical field.
                if mode == CompareMode::IgnoreText {
                    a.input_type.structural_eq(&b.input_type)
                } else {
                    a.structural_eq(b)
                }
            }
            (StructuredNode::Repeatable(a), StructuredNode::Repeatable(b)) => {
                a.min_occurrences == b.min_occurrences
                    && a.max_occurrences == b.max_occurrences
                    && a.item.structural_cmp(&b.item, mode)
            }
            (StructuredNode::Group(a), StructuredNode::Group(b)) => {
                // In IgnoreText mode (used for translation merging), Groups match by type
                // only — the child count may differ across languages because rich text
                // can produce a different number of <p> elements. merge_node_lists will
                // use LCS to align the children correctly.
                mode == CompareMode::IgnoreText
                    || (a.children.len() == b.children.len()
                        && a.children
                            .iter()
                            .zip(b.children.iter())
                            .all(|(ca, cb)| ca.structural_cmp(cb, mode)))
            }
            (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) => {
                if mode == CompareMode::IgnoreText {
                    // In IgnoreText mode (translation merging), match conditionals
                    // by their condition (field_name + value) so that the LCS
                    // correctly pairs e.g. Cond(CL_ClientType=="Firma") across
                    // languages, even when content structure differs.
                    a.condition == b.condition
                } else {
                    // In Full mode (exhaustive state merging), two ConditionalNodes
                    // are structurally equal only when both their condition AND their
                    // content match.  Comparing only content would silently equate
                    // Cond(fieldA, P) with Cond(fieldB, P) and drop one.
                    a.condition == b.condition && a.content.structural_cmp(&b.content, mode)
                }
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
            (StructuredNode::MultiColumnLayout(a), StructuredNode::MultiColumnLayout(b)) => {
                a.num_columns == b.num_columns
                    && a.columns.len() == b.columns.len()
                    && a.columns.iter().zip(b.columns.iter()).all(|(ca, cb)| {
                        ca.len() == cb.len()
                            && ca
                                .iter()
                                .zip(cb.iter())
                                .all(|(na, nb)| na.structural_cmp(nb, mode))
                    })
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
            StructuredNode::MultiColumnLayout(_) => 11,
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
            StructuredNode::MultiColumnLayout(mc) => {
                for col in &mc.columns {
                    for node in col {
                        node.collect_languages(langs);
                    }
                }
            }
            _ => {}
        }
    }
}

impl InlineText {
    /// Check if two InlineText are structurally equal (compare text content)
    pub fn structural_eq(&self, other: &Self) -> bool {
        let mut self_langs = BTreeSet::new();
        let mut other_langs = BTreeSet::new();
        self.collect_languages(&mut self_langs);
        other.collect_languages(&mut other_langs);

        let shared_langs: Vec<&str> = self_langs
            .intersection(&other_langs)
            .map(String::as_str)
            .collect();

        if !shared_langs.is_empty() {
            return shared_langs
                .iter()
                .any(|lang| self.plain_text_in(lang) == other.plain_text_in(lang));
        }

        // If either side uses translated content but there is no shared
        // language key, they are considered non-equal.
        if !self_langs.is_empty() || !other_langs.is_empty() {
            return false;
        }

        // Plain-text fallback for non-translated content.
        self.as_plain_text() == other.as_plain_text()
    }
}

impl FieldNode {
    /// Returns the SOM path string of this field, or an empty string if unavailable.
    pub fn som_path_str(&self) -> &str {
        self.som_path.as_ref().map(|p| p.as_str()).unwrap_or("")
    }

    /// Check if two fields are structurally equal.
    /// Compares name, text-bearing metadata, and input type structure, but NOT value.
    pub fn structural_eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.label.structural_eq(&other.label)
            && self.placeholder.structural_eq(&other.placeholder)
            && self.input_type.structural_eq(&other.input_type)
    }
}

trait OptionStructuralEq<T> {
    fn structural_eq(&self, other: &Self) -> bool;
}

impl OptionStructuralEq<InlineText> for Option<InlineText> {
    fn structural_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (None, None) => true,
            (Some(a), Some(b)) => a.structural_eq(b),
            _ => false,
        }
    }
}

impl OptionStructuralEq<TranslatableString> for Option<TranslatableString> {
    fn structural_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (None, None) => true,
            (Some(a), Some(b)) => a.structural_eq(b),
            _ => false,
        }
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
                // Compare by values and language-aware option names.
                opts1.len() == opts2.len()
                    && opts1
                        .iter()
                        .zip(opts2.iter())
                        .all(|(o1, o2)| o1.value == o2.value && o1.name.structural_eq(&o2.name))
            }
            (FieldType::Select { options: opts1 }, FieldType::Select { options: opts2 }) => {
                // Compare by values and language-aware option names.
                opts1.len() == opts2.len()
                    && opts1
                        .iter()
                        .zip(opts2.iter())
                        .all(|(o1, o2)| o1.value == o2.value && o1.name.structural_eq(&o2.name))
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
        let caption_eq =
            mode == CompareMode::IgnoreText || self.caption.structural_eq(&other.caption);

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

    /// The number of exhaustive form states that were merged to produce this envelope.
    /// Used to detect mismatches between language variants of the same form.
    #[serde(default = "default_state_count")]
    pub state_count: usize,
}
