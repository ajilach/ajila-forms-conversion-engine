//! Markdown conversion utilities for InlineText.
//!
//! Provides bidirectional conversion between InlineText and markdown format,
//! preserving bold (**text**) and italic (*text*) formatting.

use blueprint::structured::{InlineNode, InlineText, TranslatedText};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Convert TranslatedText to a markdown-formatted string for a specific language.
///
/// Preserves bold (Strong) and italic (Emphasis) formatting using markdown syntax.
/// Extracts the InlineText for the specified language and renders it as markdown.
pub fn inline_text_to_markdown(text: &TranslatedText, language: Option<&str>) -> String {
    let inline = language
        .and_then(|lang| text.get(lang))
        .or_else(|| text.0.values().next());
    match inline {
        Some(t) => inline_text_to_markdown_inner(t),
        None => String::new(),
    }
}

/// Convert a single InlineText to markdown.
fn inline_text_to_markdown_inner(text: &InlineText) -> String {
    let mut result = String::new();
    for node in &text.0 {
        inline_node_to_markdown(node, &mut result);
    }
    result
}

fn inline_node_to_markdown(node: &InlineNode, out: &mut String) {
    match node {
        InlineNode::Text(s) => {
            out.push_str(s);
        }
        InlineNode::Link(link) => {
            out.push('[');
            for child in &link.content.0 {
                inline_node_to_markdown(child, out);
            }
            out.push_str("](");
            out.push_str(&link.href);
            out.push(')');
        }
        InlineNode::Strong(inner) => {
            out.push_str("**");
            inline_node_to_markdown(inner, out);
            out.push_str("**");
        }
        InlineNode::Emphasis(inner) => {
            out.push('*');
            inline_node_to_markdown(inner, out);
            out.push('*');
        }
        InlineNode::Superscript(inner) => {
            out.push_str("<sup>");
            inline_node_to_markdown(inner, out);
            out.push_str("</sup>");
        }
    }
}

/// Parse a markdown string into InlineText.
///
/// Supports bold (**text**), italic (*text*), and links [text](url).
/// Returns plain text nodes plus Strong/Emphasis wrappers as appropriate.
pub fn markdown_to_inline_text(markdown: &str) -> InlineText {
    // Escape block-level syntax (lists, blockquotes) that would otherwise consume
    // user text. We only want inline formatting (bold, italic, links).
    let escaped = escape_block_syntax(markdown);
    let options = Options::empty();
    let parser = Parser::new_ext(&escaped, options);

    let mut nodes: Vec<InlineNode> = Vec::new();
    let mut stack: Vec<FormattingContext> = Vec::new();

    for event in parser {
        match event {
            Event::Text(text) => {
                let text_str = text.to_string();
                if !text_str.is_empty() {
                    if let Some(ctx) = stack.last_mut() {
                        ctx.text.push_str(&text_str);
                    } else {
                        nodes.push(InlineNode::Text(text_str));
                    }
                }
            }
            Event::Code(code) => {
                // Treat inline code as plain text (no special formatting in InlineNode)
                let code_str = code.to_string();
                if let Some(ctx) = stack.last_mut() {
                    ctx.text.push_str(&code_str);
                } else {
                    nodes.push(InlineNode::Text(code_str));
                }
            }
            Event::Start(tag) => match tag {
                Tag::Strong => {
                    stack.push(FormattingContext {
                        kind: FormattingKind::Strong,
                        text: String::new(),
                    });
                }
                Tag::Emphasis => {
                    stack.push(FormattingContext {
                        kind: FormattingKind::Emphasis,
                        text: String::new(),
                    });
                }
                Tag::Link { dest_url, .. } => {
                    stack.push(FormattingContext {
                        kind: FormattingKind::Link(dest_url.to_string()),
                        text: String::new(),
                    });
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Strong => {
                    if let Some(ctx) = stack.pop()
                        && ctx.kind == FormattingKind::Strong
                    {
                        let inner = InlineNode::Text(ctx.text);
                        let node = InlineNode::Strong(Box::new(inner));
                        push_to_context_or_nodes(&mut stack, &mut nodes, node);
                    }
                }
                TagEnd::Emphasis => {
                    if let Some(ctx) = stack.pop()
                        && ctx.kind == FormattingKind::Emphasis
                    {
                        let inner = InlineNode::Text(ctx.text);
                        let node = InlineNode::Emphasis(Box::new(inner));
                        push_to_context_or_nodes(&mut stack, &mut nodes, node);
                    }
                }
                TagEnd::Link => {
                    if let Some(ctx) = stack.pop()
                        && let FormattingKind::Link(href) = ctx.kind
                    {
                        let node = InlineNode::Link(blueprint::structured::LinkNode {
                            href,
                            content: InlineText(vec![InlineNode::Text(ctx.text)]),
                        });
                        push_to_context_or_nodes(&mut stack, &mut nodes, node);
                    }
                }
                _ => {}
            },
            Event::SoftBreak | Event::HardBreak => {
                // Convert line breaks to space
                if let Some(ctx) = stack.last_mut() {
                    ctx.text.push(' ');
                } else if let Some(InlineNode::Text(last)) = nodes.last_mut() {
                    last.push(' ');
                } else {
                    nodes.push(InlineNode::Text(" ".to_string()));
                }
            }
            _ => {}
        }
    }

    // Merge adjacent Text nodes for cleaner output
    let merged = merge_adjacent_text_nodes(nodes);

    InlineText(merged)
}

/// Escape block-level markdown syntax so that only inline formatting is parsed.
/// This prevents text like "3. foo" from being consumed as a list item.
fn escape_block_syntax(input: &str) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(input.len() + 8);
    for line in input.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("> ") || trimmed.starts_with(">") && trimmed.len() == 1 {
            // Blockquote
            let prefix = &line[..line.len() - trimmed.len()];
            let _ = write!(result, "{prefix}\\>{}", &trimmed[1..]);
        } else if is_ordered_list_start(trimmed) {
            // Ordered list: "1. ", "2) " etc.
            let prefix = &line[..line.len() - trimmed.len()];
            let dot_pos = trimmed.find(['.', ')']).unwrap();
            let _ = write!(
                result,
                "{prefix}{}\\{}{}",
                &trimmed[..dot_pos],
                &trimmed[dot_pos..dot_pos + 1],
                &trimmed[dot_pos + 1..]
            );
        } else if is_unordered_list_start(trimmed) {
            // Unordered list: "- ", "* ", "+ "
            let prefix = &line[..line.len() - trimmed.len()];
            let _ = write!(result, "{prefix}\\{}", trimmed);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    // Remove trailing newline if input didn't end with one
    if !input.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

fn is_ordered_list_start(s: &str) -> bool {
    // CommonMark: up to 9 digits, followed by '.' or ')', followed by space
    let mut chars = s.chars();
    let first = chars.next();
    if !first.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    for c in chars {
        if c == '.' || c == ')' {
            // Must be followed by a space (or end)
            let rest = &s[s.find(c).unwrap() + 1..];
            return rest.is_empty() || rest.starts_with(' ');
        }
        if !c.is_ascii_digit() {
            return false;
        }
    }
    false
}

fn is_unordered_list_start(s: &str) -> bool {
    matches!(s.as_bytes(), [b'-' | b'*' | b'+', b' ', ..])
}

#[derive(Debug, PartialEq)]
enum FormattingKind {
    Strong,
    Emphasis,
    Link(String),
}

struct FormattingContext {
    kind: FormattingKind,
    text: String,
}

fn push_to_context_or_nodes(
    stack: &mut [FormattingContext],
    nodes: &mut Vec<InlineNode>,
    node: InlineNode,
) {
    // For nested formatting, we need to convert the node to text and append
    if let Some(ctx) = stack.last_mut() {
        // Nested formatting: flatten to text (limitation of current InlineNode structure)
        ctx.text.push_str(&node_to_plain_text(&node));
    } else {
        nodes.push(node);
    }
}

fn node_to_plain_text(node: &InlineNode) -> String {
    match node {
        InlineNode::Text(s) => s.clone(),
        InlineNode::Strong(inner)
        | InlineNode::Emphasis(inner)
        | InlineNode::Superscript(inner) => node_to_plain_text(inner),
        InlineNode::Link(link) => link.content.as_plain_text(),
    }
}

fn merge_adjacent_text_nodes(nodes: Vec<InlineNode>) -> Vec<InlineNode> {
    let mut result: Vec<InlineNode> = Vec::new();

    for node in nodes {
        if let InlineNode::Text(text) = &node
            && let Some(InlineNode::Text(last)) = result.last_mut()
        {
            last.push_str(text);
            continue;
        }
        result.push(node);
    }

    result
}

/// Parse markdown and update a specific language's InlineText in a TranslatedText.
///
/// With the new TranslatedText model, each language has its own independent
/// InlineText tree. Editing one language simply replaces that language's tree.
pub fn markdown_to_inline_text_multilingual(
    markdown: &str,
    language: &str,
    existing: &TranslatedText,
) -> TranslatedText {
    let parsed = markdown_to_inline_text(markdown);
    let mut result = existing.clone();
    result.insert(language.to_string(), parsed);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_roundtrip() {
        let text = TranslatedText::single("en", InlineText(vec![InlineNode::Text("Hello world".to_string())]));
        let md = inline_text_to_markdown(&text, Some("en"));
        assert_eq!(md, "Hello world");

        let parsed = markdown_to_inline_text(&md);
        assert_eq!(parsed.as_plain_text(), "Hello world");
    }

    #[test]
    fn test_bold_roundtrip() {
        let text = TranslatedText::single("it", InlineText(vec![
            InlineNode::Text("A tal fine, il Cliente ".to_string()),
            InlineNode::Strong(Box::new(InlineNode::Text("dichiara".to_string()))),
            InlineNode::Text(" di avere".to_string()),
        ]));

        let md = inline_text_to_markdown(&text, Some("it"));
        assert_eq!(md, "A tal fine, il Cliente **dichiara** di avere");

        let parsed = markdown_to_inline_text(&md);
        assert_eq!(parsed.0.len(), 3);
        assert!(matches!(&parsed.0[1], InlineNode::Strong(_)));
    }

    #[test]
    fn test_italic_roundtrip() {
        let text = TranslatedText::single("en", InlineText(vec![
            InlineNode::Text("Some ".to_string()),
            InlineNode::Emphasis(Box::new(InlineNode::Text("italic".to_string()))),
            InlineNode::Text(" text".to_string()),
        ]));

        let md = inline_text_to_markdown(&text, Some("en"));
        assert_eq!(md, "Some *italic* text");

        let parsed = markdown_to_inline_text(&md);
        assert_eq!(parsed.0.len(), 3);
        assert!(matches!(&parsed.0[1], InlineNode::Emphasis(_)));
    }

    #[test]
    fn test_mixed_formatting() {
        let text = TranslatedText::single("en", InlineText(vec![
            InlineNode::Strong(Box::new(InlineNode::Text("bold".to_string()))),
            InlineNode::Text(" and ".to_string()),
            InlineNode::Emphasis(Box::new(InlineNode::Text("italic".to_string()))),
        ]));

        let md = inline_text_to_markdown(&text, Some("en"));
        assert_eq!(md, "**bold** and *italic*");
    }

    #[test]
    fn test_multilingual_preserves_formatting() {
        // Start with translated text: each language has its own InlineText
        let mut existing = TranslatedText::empty();
        existing.insert("de", InlineText::plain("Hallo Welt"));
        existing.insert("en", InlineText::plain("Hello World"));

        // Edit German with bold
        let result = markdown_to_inline_text_multilingual("**Hallo** Welt", "de", &existing);

        // German should now have bold formatting
        let de_text = result.get("de").unwrap();
        assert_eq!(de_text.0.len(), 2);
        assert!(matches!(&de_text.0[0], InlineNode::Strong(_)));

        // English should be unchanged (plain text)
        let en_text = result.get("en").unwrap();
        assert_eq!(en_text.as_plain_text(), "Hello World");
    }

    #[test]
    fn test_multilingual_display_per_language() {
        // Create multilingual text with different formatting per language
        let mut text = TranslatedText::empty();
        text.insert("de", InlineText(vec![
            InlineNode::Strong(Box::new(InlineNode::Text("fett".to_string()))),
            InlineNode::Text(" Text".to_string()),
        ]));
        text.insert("en", InlineText(vec![
            InlineNode::Strong(Box::new(InlineNode::Text("bold".to_string()))),
            InlineNode::Text(" text".to_string()),
        ]));

        // Display for German
        let md_de = inline_text_to_markdown(&text, Some("de"));
        assert_eq!(md_de, "**fett** Text");

        // Display for English
        let md_en = inline_text_to_markdown(&text, Some("en"));
        assert_eq!(md_en, "**bold** text");
    }

    #[test]
    fn test_numbered_prefix_not_consumed_as_list() {
        // "3. Section Title" should NOT be treated as an ordered list
        let result = markdown_to_inline_text("3. Section Title");
        assert_eq!(result.as_plain_text(), "3. Section Title");

        let result = markdown_to_inline_text("12. Another heading");
        assert_eq!(result.as_plain_text(), "12. Another heading");

        // Unordered list markers should also be preserved
        let result = markdown_to_inline_text("- some text");
        assert_eq!(result.as_plain_text(), "- some text");

        let result = markdown_to_inline_text("* starred text");
        assert_eq!(result.as_plain_text(), "* starred text");

        // Blockquote marker
        let result = markdown_to_inline_text("> quoted");
        assert_eq!(result.as_plain_text(), "> quoted");

        // Inline formatting should still work with escaped block syntax
        let result = markdown_to_inline_text("3. **bold** title");
        assert_eq!(result.0.len(), 3);
        assert_eq!(result.as_plain_text(), "3. bold title");
        assert!(matches!(&result.0[1], InlineNode::Strong(_)));
    }
}
