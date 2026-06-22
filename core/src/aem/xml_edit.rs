//! Structure-aware editing of an AEM `.content.xml` document.
//!
//! These helpers let a caller (the conversion agent's tools) modify the final
//! JCR content XML by addressing **nodes** rather than doing raw string
//! find/replace. A node is addressed by a `/`-separated **path** of element
//! local-names from the document root, e.g.
//! `jcr:root/guideContainer/panel_<uuid>/textbox_<uuid>`. Element tag names are
//! deterministic and unique-by-construction (see [`crate::aem::AemNode::element_name`]),
//! so a plain path is normally unambiguous; for the rare repeated sibling name
//! (`cq:responsive`, `default`, repeatable children) append a 1-based index,
//! e.g. `default[2]`.
//!
//! Each mutation locates the target element's byte span with a streaming
//! `quick-xml` pass and **splices the original string**, so every untouched byte
//! (and its formatting) is preserved exactly. The result is then normalised with
//! [`reformat_attributes`] and checked with [`validate_xml_wellformed`]; if the
//! edit would produce non-well-formed XML it is rejected and the original is left
//! untouched.

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use super::xml_validation::validate_xml_wellformed;
use super::xml_writer::reformat_attributes;

/// Where to insert a new child relative to an existing parent's children.
#[derive(Debug, Clone)]
pub enum InsertPos {
    /// As the parent's first child.
    First,
    /// As the parent's last child.
    Last,
    /// Immediately before the named direct child (a path *segment*, e.g.
    /// `textbox_<uuid>` or `default[2]`).
    Before(String),
    /// Immediately after the named direct child.
    After(String),
}

/// A parsed element with byte spans into the source XML.
struct El {
    /// Qualified tag name as it appears in the source (e.g. `jcr:root`, `panel_ab12`).
    name: String,
    /// Byte offset of the `<` opening the start tag.
    outer_start: usize,
    /// Byte offset just past the start tag's `>` (or `/>`).
    start_tag_end: usize,
    /// Byte offset of the `<` opening the end tag (== `outer_end` for empty elements).
    inner_end: usize,
    /// Byte offset just past the end tag's `>` (or the empty element's `/>`).
    outer_end: usize,
    /// Whether this was a self-closing (`<x/>`) element.
    is_empty: bool,
    children: Vec<El>,
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// Parse `xml` into a tree of [`El`] with byte spans. Returns the top-level
/// elements (normally just `jcr:root`).
fn parse_elements(xml: &str) -> Result<Vec<El>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = false;

    let mut stack: Vec<El> = Vec::new();
    let mut roots: Vec<El> = Vec::new();

    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => {
                let after = reader.buffer_position() as usize;
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                stack.push(El {
                    name,
                    outer_start: before,
                    start_tag_end: after,
                    inner_end: after,
                    outer_end: after,
                    is_empty: false,
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(ref e)) => {
                let after = reader.buffer_position() as usize;
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                push_child(
                    &mut stack,
                    &mut roots,
                    El {
                        name,
                        outer_start: before,
                        start_tag_end: after,
                        inner_end: after,
                        outer_end: after,
                        is_empty: true,
                        children: Vec::new(),
                    },
                );
            }
            Ok(Event::End(_)) => {
                let after = reader.buffer_position() as usize;
                let mut el = stack
                    .pop()
                    .ok_or_else(|| "unbalanced XML: end tag without a matching start".to_string())?;
                el.inner_end = before;
                el.outer_end = after;
                push_child(&mut stack, &mut roots, el);
            }
            Ok(_) => {}
            Err(e) => return Err(format!("XML parse error: {e}")),
        }
    }

    if !stack.is_empty() {
        return Err("unbalanced XML: unclosed element(s)".to_string());
    }
    Ok(roots)
}

fn push_child(stack: &mut Vec<El>, roots: &mut Vec<El>, el: El) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(el),
        None => roots.push(el),
    }
}

// ── Path resolution ──────────────────────────────────────────────────────────

/// Parse a path string into `(name, optional 1-based index)` segments.
fn parse_path(path: &str) -> Result<Vec<(String, Option<usize>)>, String> {
    let trimmed = path.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }
    let mut segs = Vec::new();
    for raw in trimmed.split('/') {
        segs.push(parse_segment(raw)?);
    }
    Ok(segs)
}

/// Parse a single path segment like `panel_ab12` or `default[2]`.
fn parse_segment(raw: &str) -> Result<(String, Option<usize>), String> {
    if raw.is_empty() {
        return Err("path contains an empty segment".to_string());
    }
    let (name, idx) = match raw.find('[') {
        Some(open) => {
            if !raw.ends_with(']') {
                return Err(format!("malformed index in segment '{raw}'"));
            }
            let n = &raw[..open];
            let i: usize = raw[open + 1..raw.len() - 1]
                .parse()
                .map_err(|_| format!("invalid index in segment '{raw}'"))?;
            if i == 0 {
                return Err(format!("index is 1-based; '{raw}' uses [0]"));
            }
            (n.to_string(), Some(i))
        }
        None => (raw.to_string(), None),
    };
    if name.is_empty() {
        return Err(format!("missing element name in segment '{raw}'"));
    }
    Ok((name, idx))
}

/// Resolve `path` against the parsed tree, returning the matched element.
fn resolve<'a>(roots: &'a [El], path: &str) -> Result<&'a El, String> {
    let segs = parse_path(path)?;
    let mut list: &[El] = roots;
    let mut current: Option<&El> = None;

    for (depth, (name, idx)) in segs.iter().enumerate() {
        let matches: Vec<&El> = list.iter().filter(|e| &e.name == name).collect();
        if matches.is_empty() {
            return Err(format!(
                "no element '{name}' at depth {depth}; available children: [{}]",
                available(list)
            ));
        }
        let chosen = match idx {
            Some(i) => *matches.get(i - 1).ok_or_else(|| {
                format!(
                    "'{name}[{i}]' is out of range; only {} sibling(s) named '{name}'",
                    matches.len()
                )
            })?,
            None if matches.len() > 1 => {
                return Err(format!(
                    "'{name}' is ambiguous ({} siblings share this name); add a 1-based index like '{name}[1]'",
                    matches.len()
                ));
            }
            None => matches[0],
        };
        current = Some(chosen);
        list = &chosen.children;
    }
    Ok(current.expect("non-empty path yields a node"))
}

/// A comma-separated list of the distinct child names in `list`, for error text.
fn available(list: &[El]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for e in list {
        if !names.contains(&e.name.as_str()) {
            names.push(&e.name);
        }
    }
    names.join(", ")
}

// ── Read / outline ─────────────────────────────────────────────────────────--

/// Return a compact outline of the document: one line per element, showing its
/// full path plus a few key attributes, so a caller can discover node paths.
pub fn outline_aem_xml(xml: &str) -> Result<String, String> {
    let roots = parse_elements(xml)?;
    let mut out = String::new();
    for (node, suffix) in indexed_children(&roots) {
        write_outline(xml, node, &format!("{}{suffix}", node.name), &mut out);
    }
    if out.is_empty() {
        out.push_str("(no elements)\n");
    }
    Ok(out)
}

fn write_outline(xml: &str, node: &El, path: &str, out: &mut String) {
    out.push_str(path);
    let tag = &xml[node.outer_start..node.start_tag_end];
    for key in ["name", "jcr:title", "jcr:primaryType", "guideNodeClass"] {
        if let Some(v) = attr_value(tag, key) {
            out.push_str(&format!("  {key}=\"{v}\""));
        }
    }
    out.push('\n');

    // Disambiguate repeated sibling names with a 1-based index suffix so the
    // printed paths are directly usable.
    for (child, suffix) in indexed_children(&node.children) {
        let child_path = format!("{path}/{}{suffix}", child.name);
        write_outline(xml, child, &child_path, out);
    }
}

/// Pair each child with its index suffix (`` when unique among same-named
/// siblings, else `[n]`).
fn indexed_children(children: &[El]) -> Vec<(&El, String)> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        let same: Vec<&El> = children.iter().filter(|c| c.name == child.name).collect();
        let suffix = if same.len() > 1 {
            let pos = same
                .iter()
                .position(|c| std::ptr::eq(*c, child))
                .map(|p| p + 1)
                .unwrap_or(1);
            format!("[{pos}]")
        } else {
            String::new()
        };
        out.push((child, suffix));
    }
    out
}

/// Return just the subtree (start tag through end tag) of the node at `path`.
pub fn read_aem_xml_node(xml: &str, path: &str) -> Result<String, String> {
    let roots = parse_elements(xml)?;
    let el = resolve(&roots, path)?;
    Ok(xml[el.outer_start..el.outer_end].to_string())
}

// ── Attribute edits ────────────────────────────────────────────────────────--

/// Set (or add) an attribute on the node at `path`. The value is inserted
/// verbatim — supply JCR-typed values such as `{Boolean}true` directly. The
/// edit is rejected if the result is not well-formed (e.g. an unescaped `&`).
pub fn set_aem_xml_attribute(
    xml: &str,
    path: &str,
    attr: &str,
    value: &str,
) -> Result<String, String> {
    validate_attr_name(attr)?;
    let roots = parse_elements(xml)?;
    let el = resolve(&roots, path)?;
    let (outer_start, start_tag_end) = (el.outer_start, el.start_tag_end);
    drop(roots);

    let tag = &xml[outer_start..start_tag_end];
    let new_tag = match find_attr(tag, attr) {
        Some((s, e)) => {
            // Replace the existing `attr="..."` value, preserving the name.
            format!("{}{attr}=\"{value}\"{}", &tag[..s], &tag[e..])
        }
        None => insert_attr_into_tag(tag, attr, value),
    };

    finalize(format!(
        "{}{new_tag}{}",
        &xml[..outer_start],
        &xml[start_tag_end..]
    ))
}

/// Remove an attribute from the node at `path`. Errors if the node has no such
/// attribute.
pub fn remove_aem_xml_attribute(xml: &str, path: &str, attr: &str) -> Result<String, String> {
    validate_attr_name(attr)?;
    let roots = parse_elements(xml)?;
    let el = resolve(&roots, path)?;
    let (outer_start, start_tag_end) = (el.outer_start, el.start_tag_end);
    drop(roots);

    let tag = &xml[outer_start..start_tag_end];
    let (s, e) = find_attr(tag, attr)
        .ok_or_else(|| format!("node has no attribute '{attr}'"))?;
    // Also consume the whitespace that precedes the attribute so we don't leave a
    // blank line or a double space behind.
    let mut start = s;
    let bytes = tag.as_bytes();
    while start > 0 && (bytes[start - 1] == b' ' || bytes[start - 1] == b'\t') {
        start -= 1;
    }
    if start > 0 && bytes[start - 1] == b'\n' {
        // A whole `\n<indent>attr="..."` line: drop the leading newline too.
        start -= 1;
    }
    let new_tag = format!("{}{}", &tag[..start], &tag[e..]);

    finalize(format!(
        "{}{new_tag}{}",
        &xml[..outer_start],
        &xml[start_tag_end..]
    ))
}

/// Locate the byte range of a complete `attr="..."` occurrence inside a start
/// tag. Matches only a whole attribute (the name must be preceded by whitespace
/// and followed by `=`), so `name` does not match a longer attribute like
/// `dorName`. Returns `(start, end)` byte offsets within `tag`.
fn find_attr(tag: &str, attr: &str) -> Option<(usize, usize)> {
    let bytes = tag.as_bytes();
    let needle = format!("{attr}=\"");
    let mut from = 0;
    while let Some(rel) = tag[from..].find(&needle) {
        let pos = from + rel;
        // The attribute name must start at a token boundary (preceded by
        // whitespace), otherwise it is a suffix of another attribute name.
        let boundary = pos == 0 || bytes[pos - 1].is_ascii_whitespace();
        if boundary {
            // Find the closing quote of the value.
            let value_start = pos + needle.len();
            if let Some(q) = tag[value_start..].find('"') {
                return Some((pos, value_start + q + 1));
            }
            return None;
        }
        from = pos + needle.len();
    }
    None
}

/// Insert a new attribute into `tag` (a start tag, possibly self-closing),
/// placing it as the first attribute after the element name and matching the
/// existing one-attribute-per-line indentation when present.
fn insert_attr_into_tag(tag: &str, attr: &str, value: &str) -> String {
    // End of the element name: first whitespace, `>` or `/` after the leading `<`.
    let name_end = tag[1..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .map(|i| 1 + i)
        .unwrap_or(tag.len());

    let insertion = match attr_indent(tag) {
        Some(indent) => format!("\n{indent}{attr}=\"{value}\""),
        None => format!(" {attr}=\"{value}\""),
    };
    format!("{}{insertion}{}", &tag[..name_end], &tag[name_end..])
}

/// The indentation used for attributes in a multi-line start tag (the
/// whitespace following the first newline inside the tag), if any.
fn attr_indent(tag: &str) -> Option<String> {
    let nl = tag.find('\n')?;
    let rest = &tag[nl + 1..];
    let indent: String = rest
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    Some(indent)
}

/// Reject attribute names that aren't simple XML names (defends the matcher and
/// avoids producing malformed tags).
fn validate_attr_name(attr: &str) -> Result<(), String> {
    let ok = !attr.is_empty()
        && attr
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid attribute name '{attr}'"))
    }
}

// ── Node edits (remove / replace / insert) ───────────────────────────────────

/// Remove the node at `path` (its whole subtree). Refuses to remove the
/// document root.
pub fn remove_aem_xml_node(xml: &str, path: &str) -> Result<String, String> {
    if parse_path(path)?.len() == 1 {
        return Err("refusing to remove the document root element".to_string());
    }
    let roots = parse_elements(xml)?;
    let el = resolve(&roots, path)?;
    let (outer_start, outer_end) = (el.outer_start, el.outer_end);
    drop(roots);

    // Extend the deletion over the node's leading indentation and one trailing
    // newline so we don't leave a blank line behind.
    let bytes = xml.as_bytes();
    let mut start = outer_start;
    while start > 0 && (bytes[start - 1] == b' ' || bytes[start - 1] == b'\t') {
        start -= 1;
    }
    let mut end = outer_end;
    if xml[end..].starts_with("\r\n") {
        end += 2;
    } else if xml[end..].starts_with('\n') {
        end += 1;
    }

    finalize(format!("{}{}", &xml[..start], &xml[end..]))
}

/// Replace the node at `path` (its whole subtree) with `fragment`. The fragment
/// must itself be well-formed XML. Refuses to replace the document root.
pub fn replace_aem_xml_node(xml: &str, path: &str, fragment: &str) -> Result<String, String> {
    validate_fragment(fragment)?;
    let roots = parse_elements(xml)?;
    let el = resolve(&roots, path)?;
    if parse_path(path)?.len() == 1 {
        return Err("refusing to replace the document root element".to_string());
    }
    let (outer_start, outer_end) = (el.outer_start, el.outer_end);
    drop(roots);

    finalize(format!(
        "{}{}{}",
        &xml[..outer_start],
        fragment.trim(),
        &xml[outer_end..]
    ))
}

/// Insert `fragment` as a child of the node at `parent_path`, at `position`. The
/// fragment must be well-formed XML.
pub fn insert_aem_xml_node(
    xml: &str,
    parent_path: &str,
    fragment: &str,
    position: InsertPos,
) -> Result<String, String> {
    validate_fragment(fragment)?;
    let roots = parse_elements(xml)?;
    let parent = resolve(&roots, parent_path)?;
    let frag = fragment.trim().to_string();

    // Determine the anchor child (if any) and whether we go before/after it.
    let (anchor, after): (Option<&El>, bool) = match &position {
        InsertPos::First => (parent.children.first(), false),
        InsertPos::Last => (parent.children.last(), true),
        InsertPos::Before(seg) => (Some(find_direct_child(parent, seg)?), false),
        InsertPos::After(seg) => (Some(find_direct_child(parent, seg)?), true),
    };

    let edited = match anchor {
        Some(child) => {
            if after {
                let indent = indent_before(xml, child.outer_start);
                let at = child.outer_end;
                format!("{}\n{indent}{frag}{}", &xml[..at], &xml[at..])
            } else {
                // Before the child: insert at the start of the child's line.
                let line_start = line_start_before(xml, child.outer_start);
                let indent = &xml[line_start..child.outer_start];
                format!(
                    "{}{indent}{frag}\n{}",
                    &xml[..line_start],
                    &xml[line_start..]
                )
            }
        }
        None => {
            // Parent has no children yet.
            if parent.is_empty {
                return Err(
                    "node is self-closing (no element body); use replace_aem_xml_node to give it content"
                        .to_string(),
                );
            }
            let parent_indent = indent_before(xml, parent.outer_start);
            let child_indent = format!("{parent_indent}    ");
            let at = parent.start_tag_end;
            format!("{}\n{child_indent}{frag}{}", &xml[..at], &xml[at..])
        }
    };

    finalize(edited)
}

/// Find a direct child of `parent` by a path segment (`name` or `name[n]`).
fn find_direct_child<'a>(parent: &'a El, segment: &str) -> Result<&'a El, String> {
    let (name, idx) = parse_segment(segment)?;
    let matches: Vec<&El> = parent.children.iter().filter(|c| c.name == name).collect();
    if matches.is_empty() {
        return Err(format!(
            "no direct child '{name}'; available children: [{}]",
            available(&parent.children)
        ));
    }
    match idx {
        Some(i) => matches
            .get(i - 1)
            .copied()
            .ok_or_else(|| format!("'{name}[{i}]' out of range ({} sibling(s))", matches.len())),
        None if matches.len() > 1 => Err(format!(
            "'{name}' is ambiguous ({} siblings); add an index like '{name}[1]'",
            matches.len()
        )),
        None => Ok(matches[0]),
    }
}

/// The whitespace indentation immediately preceding `pos` on its line.
fn indent_before(xml: &str, pos: usize) -> String {
    xml[line_start_before(xml, pos)..pos].to_string()
}

/// The byte offset of the start of the line containing `pos` (just after the
/// preceding newline), skipping back only over the run of spaces/tabs.
fn line_start_before(xml: &str, pos: usize) -> usize {
    let bytes = xml.as_bytes();
    let mut s = pos;
    while s > 0 && (bytes[s - 1] == b' ' || bytes[s - 1] == b'\t') {
        s -= 1;
    }
    s
}

/// Reject a fragment that is not well-formed XML on its own.
fn validate_fragment(fragment: &str) -> Result<(), String> {
    let trimmed = fragment.trim();
    if trimmed.is_empty() {
        return Err("fragment is empty".to_string());
    }
    validate_xml_wellformed(trimmed).map_err(|e| format!("fragment is not well-formed XML: {e}"))
}

// ── Finalisation ─────────────────────────────────────────────────────────────

/// Normalise attribute layout and reject the edit if the result is not
/// well-formed.
fn finalize(xml: String) -> Result<String, String> {
    let formatted = reformat_attributes(&xml);
    validate_xml_wellformed(&formatted)?;
    Ok(formatted)
}

/// Extract the value of `attr` from a start-tag substring (used by the outline).
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let (s, e) = find_attr(tag, attr)?;
    let value_start = s + attr.len() + 2; // past `attr="`
    Some(tag[value_start..e - 1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:jcr="http://www.jcp.org/jcr/1.0" jcr:primaryType="cq:Page">
    <guideContainer
        jcr:primaryType="nt:unstructured"
        name="container">
        <panel_aaa
            jcr:primaryType="nt:unstructured"
            name="p1"
            jcr:title="Panel 1">
            <textbox_bbb
                jcr:primaryType="nt:unstructured"
                name="field1"
                jcr:title="Field 1"/>
        </panel_aaa>
    </guideContainer>
</jcr:root>
"#;

    const PANEL: &str = "jcr:root/guideContainer/panel_aaa";
    const FIELD: &str = "jcr:root/guideContainer/panel_aaa/textbox_bbb";

    fn assert_wellformed(xml: &str) {
        validate_xml_wellformed(xml).expect("output must be well-formed");
    }

    #[test]
    fn set_existing_attribute_replaces_value() {
        let out = set_aem_xml_attribute(SAMPLE, FIELD, "jcr:title", "Renamed").unwrap();
        assert!(out.contains("jcr:title=\"Renamed\""));
        assert!(!out.contains("jcr:title=\"Field 1\""));
        // The panel's title is untouched.
        assert!(out.contains("jcr:title=\"Panel 1\""));
        assert_wellformed(&out);
    }

    #[test]
    fn set_new_attribute_adds_it() {
        let out = set_aem_xml_attribute(SAMPLE, FIELD, "mandatory", "{Boolean}true").unwrap();
        assert!(out.contains("mandatory=\"{Boolean}true\""));
        assert_wellformed(&out);
        // Re-reading the node shows the new attribute.
        let node = read_aem_xml_node(&out, FIELD).unwrap();
        assert!(node.contains("mandatory=\"{Boolean}true\""));
    }

    #[test]
    fn set_attribute_does_not_match_attribute_suffix() {
        // Add `dorName` then set `name`; the `name` edit must not touch `dorName`.
        let with_dor = set_aem_xml_attribute(SAMPLE, FIELD, "dorName", "DOR").unwrap();
        let out = set_aem_xml_attribute(&with_dor, FIELD, "name", "field1_renamed").unwrap();
        assert!(out.contains("dorName=\"DOR\""));
        assert!(out.contains("name=\"field1_renamed\""));
        assert_wellformed(&out);
    }

    #[test]
    fn remove_attribute_drops_it() {
        let out = remove_aem_xml_attribute(SAMPLE, FIELD, "jcr:title").unwrap();
        let node = read_aem_xml_node(&out, FIELD).unwrap();
        assert!(!node.contains("jcr:title"));
        assert!(node.contains("name=\"field1\""));
        assert_wellformed(&out);
    }

    #[test]
    fn remove_missing_attribute_errors() {
        let err = remove_aem_xml_attribute(SAMPLE, FIELD, "nonexistent").unwrap_err();
        assert!(err.contains("no attribute"));
    }

    #[test]
    fn remove_node_deletes_subtree() {
        let out = remove_aem_xml_node(SAMPLE, FIELD).unwrap();
        assert!(!out.contains("textbox_bbb"));
        assert!(out.contains("panel_aaa"));
        assert_wellformed(&out);
    }

    #[test]
    fn remove_root_is_refused() {
        let err = remove_aem_xml_node(SAMPLE, "jcr:root").unwrap_err();
        assert!(err.contains("root"));
    }

    #[test]
    fn replace_node_swaps_subtree() {
        let frag = "<textbox_ccc jcr:primaryType=\"nt:unstructured\" name=\"replaced\"/>";
        let out = replace_aem_xml_node(SAMPLE, FIELD, frag).unwrap();
        assert!(out.contains("textbox_ccc"));
        assert!(out.contains("name=\"replaced\""));
        assert!(!out.contains("textbox_bbb"));
        assert_wellformed(&out);
    }

    #[test]
    fn replace_with_malformed_fragment_errors() {
        let err = replace_aem_xml_node(SAMPLE, FIELD, "<broken").unwrap_err();
        assert!(err.contains("not well-formed"));
    }

    #[test]
    fn insert_first_and_last() {
        let frag_first = "<textbox_first jcr:primaryType=\"nt:unstructured\" name=\"first\"/>";
        let frag_last = "<textbox_last jcr:primaryType=\"nt:unstructured\" name=\"last\"/>";
        let out1 = insert_aem_xml_node(SAMPLE, PANEL, frag_first, InsertPos::First).unwrap();
        let out2 = insert_aem_xml_node(&out1, PANEL, frag_last, InsertPos::Last).unwrap();
        let panel = read_aem_xml_node(&out2, PANEL).unwrap();
        let first = panel.find("textbox_first").unwrap();
        let mid = panel.find("textbox_bbb").unwrap();
        let last = panel.find("textbox_last").unwrap();
        assert!(first < mid && mid < last, "order: first < bbb < last");
        assert_wellformed(&out2);
    }

    #[test]
    fn insert_before_and_after() {
        let frag = "<textbox_x jcr:primaryType=\"nt:unstructured\" name=\"x\"/>";
        let before =
            insert_aem_xml_node(SAMPLE, PANEL, frag, InsertPos::Before("textbox_bbb".into()))
                .unwrap();
        let p = read_aem_xml_node(&before, PANEL).unwrap();
        assert!(p.find("textbox_x").unwrap() < p.find("textbox_bbb").unwrap());
        assert_wellformed(&before);

        let after =
            insert_aem_xml_node(SAMPLE, PANEL, frag, InsertPos::After("textbox_bbb".into()))
                .unwrap();
        let p2 = read_aem_xml_node(&after, PANEL).unwrap();
        assert!(p2.find("textbox_bbb").unwrap() < p2.find("textbox_x").unwrap());
        assert_wellformed(&after);
    }

    #[test]
    fn ambiguous_path_errors() {
        let xml = "<jcr:root><a><default v=\"1\"/><default v=\"2\"/></a></jcr:root>";
        let err = read_aem_xml_node(xml, "jcr:root/a/default").unwrap_err();
        assert!(err.contains("ambiguous"));
        // With an index it resolves.
        let node = read_aem_xml_node(xml, "jcr:root/a/default[2]").unwrap();
        assert!(node.contains("v=\"2\""));
    }

    #[test]
    fn unknown_path_lists_candidates() {
        let err = read_aem_xml_node(SAMPLE, "jcr:root/guideContainer/nope").unwrap_err();
        assert!(err.contains("panel_aaa"), "error should list available children: {err}");
    }

    #[test]
    fn outline_lists_paths_with_attrs() {
        let outline = outline_aem_xml(SAMPLE).unwrap();
        assert!(outline.contains("jcr:root/guideContainer/panel_aaa"));
        assert!(outline.contains(FIELD));
        assert!(outline.contains("jcr:title=\"Panel 1\""));
    }

    #[test]
    fn untouched_region_is_byte_preserved() {
        // Editing the field's title must not alter the container's start tag bytes.
        let out = set_aem_xml_attribute(SAMPLE, FIELD, "jcr:title", "Renamed").unwrap();
        let marker = "<guideContainer\n        jcr:primaryType=\"nt:unstructured\"\n        name=\"container\">";
        assert!(out.contains(marker), "container start tag should be byte-preserved");
    }

    #[test]
    fn unescaped_ampersand_is_rejected() {
        let err = set_aem_xml_attribute(SAMPLE, FIELD, "jcr:title", "Tom & Jerry").unwrap_err();
        assert!(err.contains("&") || err.to_lowercase().contains("escap") || !err.is_empty());
    }
}
