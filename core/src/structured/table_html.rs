//! Shared [`TableNode`] -> HTML `<table>` renderer.
//!
//! Two targets need the same markup from the same node: Redacto renders a table
//! into a Quill text asset, and the AEM HTML component
//! ([`crate::aem::AemNode::HtmlDisplayer`]) renders one into a form. Only the
//! inline vocabulary differs (`<strong>`/`<em>` for Quill, `<b>`/`<i>` for AEM)
//! and Redacto additionally rewrites footnote markers into links, so both are
//! supplied by the caller as one `inline` closure.

use super::{ListNode, StructuredNode, TableNode, TranslatedText};

/// How one inline run is rendered to HTML: `(text, language) -> html`.
///
/// Callers pass `inline_text_to_html_with(text, lang, <their tag set>)`, wrapped
/// in whatever post-processing they need.
pub type InlineRenderer<'a> = dyn Fn(&TranslatedText, &str) -> String + 'a;

/// Render a [`TableNode`] as a self-contained HTML `<table>` in `language`.
///
/// A cell that is not inline content renders empty: [`render_cell_html`] knows
/// paragraphs, headings, groups and lists, and nothing else. A table holding an
/// input field must therefore NOT be routed here -- the field would vanish
/// without a trace. Callers on the AEM side check that first (see
/// `table_is_static` in `crate::aem`).
pub fn render_table_html(table: &TableNode, language: &str, inline: &InlineRenderer<'_>) -> String {
    let mut out = String::from("<table>");
    if let Some(caption) = &table.caption {
        out.push_str(&format!("<caption>{}</caption>", inline(caption, language)));
    }
    if let Some(header) = &table.header {
        out.push_str("<thead><tr>");
        for cell in &header.cells {
            out.push_str(&format!(
                "<th>{}</th>",
                render_cell_html(cell, language, inline)
            ));
        }
        out.push_str("</tr></thead>");
    }
    out.push_str("<tbody>");
    for row in &table.rows {
        out.push_str("<tr>");
        for cell in &row.cells {
            out.push_str(&format!(
                "<td>{}</td>",
                render_cell_html(cell, language, inline)
            ));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out
}

/// Render a table cell as inline content, flattening any nested structure.
pub fn render_cell_html(
    node: &StructuredNode,
    language: &str,
    inline: &InlineRenderer<'_>,
) -> String {
    match node {
        StructuredNode::Paragraph(p) => inline(&p.content, language),
        StructuredNode::Heading(h) => inline(&h.content, language),
        StructuredNode::Group(g) => g
            .children
            .iter()
            .map(|c| render_cell_html(c, language, inline))
            .collect::<Vec<_>>()
            .join(" "),
        StructuredNode::List(l) => render_plain_list_html(l, language, inline),
        _ => String::new(),
    }
}

/// Render a [`ListNode`] as a plain `<ul>` / `<ol>`, with no style attribute.
///
/// This is the shape a table cell and a Redacto asset want. The AEM converter's
/// own `render_list_html` carries a `list-style-type` for a stand-alone list
/// draw and is a different function on purpose.
pub fn render_plain_list_html(
    list: &ListNode,
    language: &str,
    inline: &InlineRenderer<'_>,
) -> String {
    let tag = if list.list_style.is_ordered() {
        "ol"
    } else {
        "ul"
    };
    let mut out = format!("<{tag}>");
    for item in &list.items {
        out.push_str("<li>");
        out.push_str(&inline(&item.content, language));
        if let Some(sub) = &item.sublist {
            out.push_str(&render_plain_list_html(sub, language, inline));
        }
        out.push_str("</li>");
    }
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
    out
}
