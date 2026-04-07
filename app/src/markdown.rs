//! Markdown conversion utilities for InlineText.
//!
//! Provides bidirectional conversion between InlineText and markdown format,
//! preserving bold (**text**) and italic (*text*) formatting.

use blueprint::structured::{InlineNode, InlineText};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Convert InlineText to a markdown-formatted string.
///
/// Preserves bold (Strong) and italic (Emphasis) formatting using markdown syntax.
/// For multilingual content, only the specified language is extracted.
pub fn inline_text_to_markdown(text: &InlineText, language: Option<&str>) -> String {
    let mut result = String::new();
    for node in &text.0 {
        inline_node_to_markdown(node, language, &mut result);
    }
    result
}

fn inline_node_to_markdown(node: &InlineNode, language: Option<&str>, out: &mut String) {
    match node {
        InlineNode::Text(s) => {
            out.push_str(s);
        }
        InlineNode::TranslatedText(map) => {
            // Get text for the specified language, or fallback to first available
            let text = language
                .and_then(|lang| map.get(lang))
                .or_else(|| map.values().next())
                .map(|s| s.as_str())
                .unwrap_or("");
            out.push_str(text);
        }
        InlineNode::Link(link) => {
            out.push('[');
            for child in &link.content.0 {
                inline_node_to_markdown(child, language, out);
            }
            out.push_str("](");
            out.push_str(&link.href);
            out.push(')');
        }
        InlineNode::Strong(inner) => {
            out.push_str("**");
            inline_node_to_markdown(inner, language, out);
            out.push_str("**");
        }
        InlineNode::Emphasis(inner) => {
            out.push('*');
            inline_node_to_markdown(inner, language, out);
            out.push('*');
        }
    }
}

/// Parse a markdown string into InlineText.
///
/// Supports bold (**text**), italic (*text*), and links [text](url).
/// Returns plain text nodes plus Strong/Emphasis wrappers as appropriate.
pub fn markdown_to_inline_text(markdown: &str) -> InlineText {
    let options = Options::empty();
    let parser = Parser::new_ext(markdown, options);

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
                    if let Some(ctx) = stack.pop() {
                        if ctx.kind == FormattingKind::Strong {
                            let inner = InlineNode::Text(ctx.text);
                            let node = InlineNode::Strong(Box::new(inner));
                            push_to_context_or_nodes(&mut stack, &mut nodes, node);
                        }
                    }
                }
                TagEnd::Emphasis => {
                    if let Some(ctx) = stack.pop() {
                        if ctx.kind == FormattingKind::Emphasis {
                            let inner = InlineNode::Text(ctx.text);
                            let node = InlineNode::Emphasis(Box::new(inner));
                            push_to_context_or_nodes(&mut stack, &mut nodes, node);
                        }
                    }
                }
                TagEnd::Link => {
                    if let Some(ctx) = stack.pop() {
                        if let FormattingKind::Link(href) = ctx.kind {
                            let node = InlineNode::Link(blueprint::structured::LinkNode {
                                href,
                                content: InlineText(vec![InlineNode::Text(ctx.text)]),
                            });
                            push_to_context_or_nodes(&mut stack, &mut nodes, node);
                        }
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
    stack: &mut Vec<FormattingContext>,
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
        InlineNode::TranslatedText(map) => map.values().next().cloned().unwrap_or_default(),
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => node_to_plain_text(inner),
        InlineNode::Link(link) => link.content.as_plain_text(),
    }
}

fn merge_adjacent_text_nodes(nodes: Vec<InlineNode>) -> Vec<InlineNode> {
    let mut result: Vec<InlineNode> = Vec::new();

    for node in nodes {
        if let InlineNode::Text(text) = &node {
            if let Some(InlineNode::Text(last)) = result.last_mut() {
                last.push_str(text);
                continue;
            }
        }
        result.push(node);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_roundtrip() {
        let text = InlineText(vec![InlineNode::Text("Hello world".to_string())]);
        let md = inline_text_to_markdown(&text, None);
        assert_eq!(md, "Hello world");

        let parsed = markdown_to_inline_text(&md);
        assert_eq!(parsed.as_plain_text(), "Hello world");
    }

    #[test]
    fn test_bold_roundtrip() {
        let text = InlineText(vec![
            InlineNode::Text("A tal fine, il Cliente ".to_string()),
            InlineNode::Strong(Box::new(InlineNode::Text("dichiara".to_string()))),
            InlineNode::Text(" di avere".to_string()),
        ]);

        let md = inline_text_to_markdown(&text, None);
        assert_eq!(md, "A tal fine, il Cliente **dichiara** di avere");

        let parsed = markdown_to_inline_text(&md);
        assert_eq!(parsed.0.len(), 3);
        assert!(matches!(&parsed.0[1], InlineNode::Strong(_)));
    }

    #[test]
    fn test_italic_roundtrip() {
        let text = InlineText(vec![
            InlineNode::Text("Some ".to_string()),
            InlineNode::Emphasis(Box::new(InlineNode::Text("italic".to_string()))),
            InlineNode::Text(" text".to_string()),
        ]);

        let md = inline_text_to_markdown(&text, None);
        assert_eq!(md, "Some *italic* text");

        let parsed = markdown_to_inline_text(&md);
        assert_eq!(parsed.0.len(), 3);
        assert!(matches!(&parsed.0[1], InlineNode::Emphasis(_)));
    }

    #[test]
    fn test_mixed_formatting() {
        let text = InlineText(vec![
            InlineNode::Strong(Box::new(InlineNode::Text("bold".to_string()))),
            InlineNode::Text(" and ".to_string()),
            InlineNode::Emphasis(Box::new(InlineNode::Text("italic".to_string()))),
        ]);

        let md = inline_text_to_markdown(&text, None);
        assert_eq!(md, "**bold** and *italic*");
    }
}
