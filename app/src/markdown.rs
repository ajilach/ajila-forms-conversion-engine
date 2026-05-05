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
                .and_then(|o| o.as_deref())
                .or_else(|| map.values().find_map(|o| o.as_deref()))
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
            let dot_pos = trimmed.find(|c| c == '.' || c == ')').unwrap();
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
        InlineNode::TranslatedText(map) => map.values().find_map(|o| o.clone()).unwrap_or_default(),
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => node_to_plain_text(inner),
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

/// Parse markdown and merge with existing translations.
///
/// This preserves formatting (bold/italic) from the markdown while keeping
/// translations for other languages. The formatting structure from the edited
/// language becomes the canonical structure for all languages.
pub fn markdown_to_inline_text_multilingual(
    markdown: &str,
    language: &str,
    existing: &InlineText,
) -> InlineText {
    // Collect existing translations from the old structure
    let mut existing_translations = std::collections::HashMap::<String, String>::new();
    collect_translations(existing, &mut existing_translations);

    // Parse the new markdown to get the structure
    let parsed = markdown_to_inline_text(markdown);

    // Convert Text nodes to TranslatedText, preserving other languages
    let nodes = convert_to_translated(parsed.0, language, &existing_translations);

    InlineText(nodes)
}

/// Collect all translations from an InlineText, flattening to plain text per language.
fn collect_translations(
    text: &InlineText,
    translations: &mut std::collections::HashMap<String, String>,
) {
    for node in &text.0 {
        collect_translations_from_node(node, translations);
    }
}

fn collect_translations_from_node(
    node: &InlineNode,
    translations: &mut std::collections::HashMap<String, String>,
) {
    match node {
        InlineNode::Text(s) => {
            // Plain text has no language — collect under "default" sentinel key.
            // Filtered out during distribution in `convert_node_to_translated`.
            translations
                .entry("default".to_string())
                .or_default()
                .push_str(s);
        }
        InlineNode::TranslatedText(map) => {
            for (lang, text) in map {
                if let Some(text) = text {
                    translations.entry(lang.clone()).or_default().push_str(text);
                }
            }
        }
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            collect_translations_from_node(inner, translations);
        }
        InlineNode::Link(link) => {
            collect_translations(&link.content, translations);
        }
    }
}

/// Convert Text nodes to TranslatedText nodes, distributing translations across nodes.
fn convert_to_translated(
    nodes: Vec<InlineNode>,
    edited_lang: &str,
    existing_translations: &std::collections::HashMap<String, String>,
) -> Vec<InlineNode> {
    // Count total text length in the new structure to distribute translations proportionally
    let total_len: usize = nodes.iter().map(text_length).sum();

    if total_len == 0 {
        return nodes;
    }

    // Track position for proportional distribution
    let mut position = 0usize;

    nodes
        .into_iter()
        .map(|node| {
            convert_node_to_translated(
                node,
                edited_lang,
                existing_translations,
                total_len,
                &mut position,
            )
        })
        .collect()
}

fn text_length(node: &InlineNode) -> usize {
    match node {
        InlineNode::Text(s) => s.len(),
        InlineNode::TranslatedText(map) => map
            .values()
            .find_map(|o| o.as_ref().map(|s| s.len()))
            .unwrap_or(0),
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => text_length(inner),
        InlineNode::Link(link) => link.content.0.iter().map(text_length).sum(),
    }
}

fn convert_node_to_translated(
    node: InlineNode,
    edited_lang: &str,
    existing_translations: &std::collections::HashMap<String, String>,
    total_len: usize,
    position: &mut usize,
) -> InlineNode {
    match node {
        InlineNode::Text(s) => {
            let node_len = s.len();
            let start_ratio = *position as f64 / total_len as f64;
            let end_ratio = (*position + node_len) as f64 / total_len as f64;
            *position += node_len;

            // Build TranslatedText with the edited language's content
            let mut map = blueprint::structured::TranslationMap::new();
            map.insert(edited_lang.to_string(), Some(s));

            // Distribute other languages' content proportionally
            for (lang, full_text) in existing_translations {
                if lang != edited_lang && lang != "default" {
                    let start_idx = (start_ratio * full_text.len() as f64).round() as usize;
                    let end_idx = (end_ratio * full_text.len() as f64).round() as usize;
                    let start_idx = start_idx.min(full_text.len());
                    let end_idx = end_idx.min(full_text.len());

                    // Extract substring, being careful with UTF-8 boundaries
                    let extracted = extract_substring_safe(full_text, start_idx, end_idx);
                    if !extracted.is_empty() {
                        map.insert(lang.clone(), Some(extracted));
                    }
                }
            }

            InlineNode::TranslatedText(map)
        }
        InlineNode::TranslatedText(mut map) => {
            // Already translated, just update position tracking
            let node_len = map
                .values()
                .find_map(|o| o.as_ref().map(|s| s.len()))
                .unwrap_or(0);
            *position += node_len;

            // Ensure edited language is present
            if !map.contains_key(edited_lang) {
                map.insert(edited_lang.to_string(), Some(String::new()));
            }

            InlineNode::TranslatedText(map)
        }
        InlineNode::Strong(inner) => InlineNode::Strong(Box::new(convert_node_to_translated(
            *inner,
            edited_lang,
            existing_translations,
            total_len,
            position,
        ))),
        InlineNode::Emphasis(inner) => InlineNode::Emphasis(Box::new(convert_node_to_translated(
            *inner,
            edited_lang,
            existing_translations,
            total_len,
            position,
        ))),
        InlineNode::Link(mut link) => {
            link.content.0 = link
                .content
                .0
                .into_iter()
                .map(|n| {
                    convert_node_to_translated(
                        n,
                        edited_lang,
                        existing_translations,
                        total_len,
                        position,
                    )
                })
                .collect();
            InlineNode::Link(link)
        }
    }
}

/// Extract a substring safely respecting UTF-8 char boundaries.
fn extract_substring_safe(s: &str, start: usize, end: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let char_count = chars.len();

    // Convert byte indices to approximate char indices
    let start_char = (start as f64 / s.len() as f64 * char_count as f64).round() as usize;
    let end_char = (end as f64 / s.len() as f64 * char_count as f64).round() as usize;

    let start_char = start_char.min(char_count);
    let end_char = end_char.min(char_count);

    chars[start_char..end_char].iter().collect()
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

    #[test]
    fn test_multilingual_preserves_formatting() {
        // Start with translated text
        let mut translations = std::collections::HashMap::new();
        translations.insert("de".to_string(), Some("Hallo Welt".to_string()));
        translations.insert("en".to_string(), Some("Hello World".to_string()));

        let existing = InlineText(vec![InlineNode::TranslatedText(translations)]);

        // Edit German with bold
        let result = markdown_to_inline_text_multilingual("**Hallo** Welt", "de", &existing);

        // Should have 2 nodes: Strong and Text, both as TranslatedText
        assert_eq!(result.0.len(), 2);

        // First node should be Strong containing TranslatedText with "Hallo" for German
        if let InlineNode::Strong(inner) = &result.0[0] {
            if let InlineNode::TranslatedText(map) = inner.as_ref() {
                assert_eq!(map.get("de"), Some(&Some("Hallo".to_string())));
                // English should have proportional content
                assert!(map.contains_key("en"));
            } else {
                panic!("Expected TranslatedText inside Strong");
            }
        } else {
            panic!("Expected Strong node");
        }

        // Second node should be TranslatedText with " Welt" for German
        if let InlineNode::TranslatedText(map) = &result.0[1] {
            assert_eq!(map.get("de"), Some(&Some(" Welt".to_string())));
        } else {
            panic!("Expected TranslatedText node");
        }
    }

    #[test]
    fn test_multilingual_display_per_language() {
        // Create multilingual formatted text
        let mut map1 = std::collections::HashMap::new();
        map1.insert("de".to_string(), Some("fett".to_string()));
        map1.insert("en".to_string(), Some("bold".to_string()));

        let mut map2 = std::collections::HashMap::new();
        map2.insert("de".to_string(), Some(" Text".to_string()));
        map2.insert("en".to_string(), Some(" text".to_string()));

        let text = InlineText(vec![
            InlineNode::Strong(Box::new(InlineNode::TranslatedText(map1))),
            InlineNode::TranslatedText(map2),
        ]);

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
