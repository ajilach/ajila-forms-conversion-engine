//! Rendering of inline structured text (`TranslatedText` / `InlineNode`) to HTML.
//!
//! Different output targets expect different tag vocabularies for the same
//! semantics: AEM rich text uses `<b>` / `<i>`, while Quill-based editors (the
//! Redacto authoring UI) use `<strong>` / `<em>`. The tag set is therefore a
//! parameter rather than being hard-coded per target.

use super::{InlineNode, TranslatedText};
use crate::util::escape_html;

/// Tag names used when rendering [`InlineNode`]s to HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineHtmlTags {
    /// Tag for [`InlineNode::Strong`].
    pub strong: &'static str,
    /// Tag for [`InlineNode::Emphasis`].
    pub emphasis: &'static str,
    /// Tag for [`InlineNode::Superscript`].
    pub superscript: &'static str,
}

/// AEM rich text vocabulary: `<b>` / `<i>` / `<sup>`.
pub const AEM_TAGS: InlineHtmlTags = InlineHtmlTags {
    strong: "b",
    emphasis: "i",
    superscript: "sup",
};

/// Quill / Redacto asset vocabulary: `<strong>` / `<em>` / `<sup>`.
pub const QUILL_TAGS: InlineHtmlTags = InlineHtmlTags {
    strong: "strong",
    emphasis: "em",
    superscript: "sup",
};

/// Convert the `language` variant of `text` to an HTML string.
///
/// Falls back to the first available language when `language` is missing, so
/// the result is never empty just because one translation is absent.
pub fn inline_text_to_html_with(
    text: &TranslatedText,
    language: &str,
    tags: InlineHtmlTags,
) -> String {
    match text.get(language).or_else(|| text.0.values().next()) {
        Some(t) => {
            let mut out = String::new();
            inline_nodes_to_html_with(&t.0, tags, &mut out);
            out
        }
        None => String::new(),
    }
}

/// Append the HTML rendering of `nodes` to `out`.
pub fn inline_nodes_to_html_with(nodes: &[InlineNode], tags: InlineHtmlTags, out: &mut String) {
    for node in nodes {
        inline_node_to_html_with(node, tags, out);
    }
}

fn inline_node_to_html_with(node: &InlineNode, tags: InlineHtmlTags, out: &mut String) {
    match node {
        InlineNode::Text(s) => {
            out.push_str(&escape_html(s));
        }
        InlineNode::Link(link) => {
            out.push_str("<a href=\"");
            out.push_str(&escape_html(&link.href));
            out.push_str("\">");
            inline_nodes_to_html_with(&link.content.0, tags, out);
            out.push_str("</a>");
        }
        InlineNode::Strong(inner) => wrap(tags.strong, inner, tags, out),
        InlineNode::Emphasis(inner) => wrap(tags.emphasis, inner, tags, out),
        InlineNode::Superscript(inner) => wrap(tags.superscript, inner, tags, out),
    }
}

fn wrap(tag: &str, inner: &InlineNode, tags: InlineHtmlTags, out: &mut String) {
    out.push('<');
    out.push_str(tag);
    out.push('>');
    inline_node_to_html_with(inner, tags, out);
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

/// Strip the leading marker number and whitespace from rendered footnote text.
///
/// E.g. `"1 Once opted up..."` → `"Once opted up..."`.
pub fn strip_footnote_marker(html: &str, marker: &str) -> String {
    let trimmed = html.trim_start();
    if let Some(rest) = trimmed.strip_prefix(marker) {
        rest.trim_start().to_string()
    } else {
        html.to_string()
    }
}
