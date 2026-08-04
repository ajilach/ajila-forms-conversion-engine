//! Rendering of a single structured block to a Redacto text-asset body.
//!
//! Asset content is the HTML fragment the Redacto authoring UI (Quill) round-
//! trips and the renderer injects verbatim via Thymeleaf `th:utext`.

use crate::structured::{
    FootnoteNode, HeadingNode, ListNode, ParagraphNode, QUILL_TAGS, StructuredNode, TableNode,
    TranslatedText, inline_text_to_html_with, strip_footnote_marker,
};

/// The empty-paragraph spacer Redacto documents use for vertical whitespace.
const SPACER: &str = "<p><br></p>";

/// Footnote markers, in document order, used to turn `<sup>N</sup>` in a body
/// block into a link to the footnote asset.
pub(super) type FootnoteMarkers = [String];

/// Render one structured block as a Redacto text-asset body.
///
/// Returns `None` for nodes that carry no content of their own (containers,
/// input fields, images) — the caller decides how to handle those.
pub fn render_block_html(
    node: &StructuredNode,
    language: &str,
    markers: &FootnoteMarkers,
) -> Option<String> {
    match node {
        StructuredNode::Heading(h) => Some(render_heading(h, language, markers)),
        StructuredNode::Paragraph(p) => Some(render_paragraph(p, language, markers)),
        StructuredNode::List(l) => Some(render_list(l, language, markers)),
        StructuredNode::Table(t) => Some(render_table(t, language, markers)),
        StructuredNode::Footnote(f) => Some(render_footnote(f, language)),
        _ => None,
    }
}

/// Render inline content and link any footnote references it contains.
fn inline(text: &TranslatedText, language: &str, markers: &FootnoteMarkers) -> String {
    let html = inline_text_to_html_with(text, language, QUILL_TAGS);
    link_footnote_references(&html, markers)
}

fn render_heading(h: &HeadingNode, language: &str, markers: &FootnoteMarkers) -> String {
    let level = h.level.as_u8();
    format!("<h{level}>{}</h{level}>", inline(&h.content, language, markers))
}

fn render_paragraph(p: &ParagraphNode, language: &str, markers: &FootnoteMarkers) -> String {
    let body = inline(&p.content, language, markers);
    if body.trim().is_empty() {
        SPACER.to_string()
    } else {
        format!("<p>{body}</p>")
    }
}

fn render_list(list: &ListNode, language: &str, markers: &FootnoteMarkers) -> String {
    let tag = if list.list_style.is_ordered() {
        "ol"
    } else {
        "ul"
    };
    let mut out = format!("<{tag}>");
    for item in &list.items {
        out.push_str("<li>");
        out.push_str(&inline(&item.content, language, markers));
        if let Some(sub) = &item.sublist {
            out.push_str(&render_list(sub, language, markers));
        }
        out.push_str("</li>");
    }
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
    out
}

fn render_table(table: &TableNode, language: &str, markers: &FootnoteMarkers) -> String {
    let mut out = String::from("<table>");
    if let Some(caption) = &table.caption {
        out.push_str(&format!(
            "<caption>{}</caption>",
            inline(caption, language, markers)
        ));
    }
    if let Some(header) = &table.header {
        out.push_str("<thead><tr>");
        for cell in &header.cells {
            out.push_str(&format!("<th>{}</th>", render_cell(cell, language, markers)));
        }
        out.push_str("</tr></thead>");
    }
    out.push_str("<tbody>");
    for row in &table.rows {
        out.push_str("<tr>");
        for cell in &row.cells {
            out.push_str(&format!("<td>{}</td>", render_cell(cell, language, markers)));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out
}

/// Render a table cell as inline content, flattening any nested structure.
fn render_cell(node: &StructuredNode, language: &str, markers: &FootnoteMarkers) -> String {
    match node {
        StructuredNode::Paragraph(p) => inline(&p.content, language, markers),
        StructuredNode::Heading(h) => inline(&h.content, language, markers),
        StructuredNode::Group(g) => g
            .children
            .iter()
            .map(|c| render_cell(c, language, markers))
            .collect::<Vec<_>>()
            .join(" "),
        StructuredNode::List(l) => render_list(l, language, markers),
        _ => String::new(),
    }
}

fn render_footnote(f: &FootnoteNode, language: &str) -> String {
    // Footnote bodies never link other footnotes, so no marker substitution.
    let body = inline_text_to_html_with(&f.content, language, QUILL_TAGS);
    match &f.marker {
        Some(marker) => format!(
            "<p id=\"footnote-{marker}\"><sup>{marker}</sup> {}</p>",
            strip_footnote_marker(&body, marker)
        ),
        None => format!("<p>{body}</p>"),
    }
}

/// Turn every `<sup>MARKER</sup>` occurrence into a link to the corresponding
/// footnote asset, mirroring the anchors the Redacto stylesheet expects.
fn link_footnote_references(html: &str, markers: &FootnoteMarkers) -> String {
    let mut result = html.to_string();
    for marker in markers {
        let pattern = format!("<sup>{marker}</sup>");
        if result.contains(&pattern) {
            let replacement =
                format!("<sup><a href=\"#footnote-{marker}\">{marker}</a></sup>");
            result = result.replace(&pattern, &replacement);
        }
    }
    result
}
