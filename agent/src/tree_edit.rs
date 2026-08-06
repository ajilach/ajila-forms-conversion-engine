//! Pieces shared by the two path-addressed tree editors
//! ([`crate::structured_edit`] and [`crate::aem_translated_edit`]).
//!
//! Both expose the same insert-position vocabulary and the same one-line node
//! summaries to the model, so the agent addresses either tree the same way.
//! Only the traversal differs, because the trees differ: `AemNodeTranslated`
//! has three container variants addressed by bare child index, while
//! `StructuredNode` has heterogeneous containers (table cells, grid elements,
//! a repeatable's item) that need typed path segments.

/// Where to put a node when inserting into a child list.
pub enum InsertPos {
    First,
    Last,
    Before(usize),
    After(usize),
}

/// Parse the `position` argument of the `insert_*_node` tools.
/// Accepts `"first"`, `"last"`, `{"before": <i>}` or `{"after": <i>}`.
pub fn parse_insert_pos(v: &serde_json::Value) -> Result<InsertPos, String> {
    if let Some(s) = v.as_str() {
        return match s {
            "first" => Ok(InsertPos::First),
            "last" => Ok(InsertPos::Last),
            other => Err(format!(
                "invalid position '{other}'; expected \"first\", \"last\", {{\"before\":<i>}} or {{\"after\":<i>}}"
            )),
        };
    }
    if let Some(obj) = v.as_object() {
        if let Some(i) = obj.get("before").and_then(|x| x.as_u64()) {
            return Ok(InsertPos::Before(i as usize));
        }
        if let Some(i) = obj.get("after").and_then(|x| x.as_u64()) {
            return Ok(InsertPos::After(i as usize));
        }
    }
    Err("invalid position; expected \"first\", \"last\", {\"before\":<i>} or {\"after\":<i>}".into())
}

/// Resolve an [`InsertPos`] to an index in a list of `len` items, clamped.
pub fn insert_index(pos: &InsertPos, len: usize) -> usize {
    match pos {
        InsertPos::First => 0,
        InsertPos::Last => len,
        InsertPos::Before(i) => (*i).min(len),
        InsertPos::After(i) => (*i + 1).min(len),
    }
}

/// Longest excerpt of `s` shown in an outline line, in characters.
const EXCERPT_LEN: usize = 60;

/// A single-line, length-capped excerpt of `s` for outline summaries.
pub fn excerpt(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > EXCERPT_LEN {
        let head: String = flat.chars().take(EXCERPT_LEN).collect();
        format!("{head}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_pos_parses() {
        assert!(matches!(
            parse_insert_pos(&serde_json::json!("first")),
            Ok(InsertPos::First)
        ));
        assert!(matches!(
            parse_insert_pos(&serde_json::json!("last")),
            Ok(InsertPos::Last)
        ));
        assert!(matches!(
            parse_insert_pos(&serde_json::json!({"before": 2})),
            Ok(InsertPos::Before(2))
        ));
        assert!(matches!(
            parse_insert_pos(&serde_json::json!({"after": 0})),
            Ok(InsertPos::After(0))
        ));
        assert!(parse_insert_pos(&serde_json::json!("middle")).is_err());
        assert!(parse_insert_pos(&serde_json::json!(3)).is_err());
    }

    #[test]
    fn insert_index_clamps_out_of_range_positions() {
        assert_eq!(insert_index(&InsertPos::First, 3), 0);
        assert_eq!(insert_index(&InsertPos::Last, 3), 3);
        assert_eq!(insert_index(&InsertPos::Before(99), 3), 3);
        assert_eq!(insert_index(&InsertPos::After(99), 3), 3);
        assert_eq!(insert_index(&InsertPos::After(1), 3), 2);
    }

    #[test]
    fn excerpt_collapses_whitespace_and_caps_length() {
        assert_eq!(excerpt("  a\n  b\tc "), "a b c");
        let long = "x".repeat(EXCERPT_LEN + 10);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_LEN + 1, "capped plus the ellipsis");
        assert!(cut.ends_with('…'));
    }
}
