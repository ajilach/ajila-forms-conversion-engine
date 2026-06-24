//! Shared utility functions used across multiple modules.

/// Escape HTML special characters in a string.
///
/// Replaces `&`, `<`, `>`, `"`, and `'` with their HTML entity equivalents.
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Encode binary data as a base64 string using the standard alphabet.
pub fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Repair "mojibake": text that was correct UTF-8 but got decoded as Latin-1
/// (or Windows-1252) and re-encoded as UTF-8, turning e.g. `ä` into `Ã¤`, `ü`
/// into `Ã¼` and `€` into `â¬`.
///
/// This is a defensive pass for source documents (e.g. XFA packets) that were
/// themselves exported with a double-encoding bug: the parser reproduces the
/// garbage faithfully, so we repair it here. It is a **no-op on clean text** —
/// it only acts when the unmistakable double-encoding signature is present and
/// the round-trip yields valid UTF-8 — so running it on correct input (the
/// common case) changes nothing.
pub fn fix_double_encoded_utf8(s: &str) -> String {
    if !looks_double_encoded(s) {
        return s.to_string();
    }
    // Reconstruct the original bytes: each char came from one source byte under
    // a Latin-1/Windows-1252 mis-decode. Latin-1 maps bytes 1:1 to U+0000..=U+00FF;
    // Windows-1252 remaps 0x80..=0x9F to assorted higher code points, so reverse
    // those too. Any char that fits neither means this isn't a clean mis-read —
    // bail and leave the text untouched.
    let mut bytes = Vec::with_capacity(s.len());
    for c in s.chars() {
        match cp1252_byte(c) {
            Some(b) => bytes.push(b),
            None => return s.to_string(),
        }
    }
    match std::str::from_utf8(&bytes) {
        // Reinterpreting as UTF-8 succeeded → this is the repaired text.
        Ok(fixed) => fixed.to_string(),
        // Not actually valid double-encoded UTF-8; leave it alone.
        Err(_) => s.to_string(),
    }
}

/// The source byte that would decode to `c` under Windows-1252 (a superset of
/// Latin-1). Returns `None` for code points that no single 1252 byte produces.
fn cp1252_byte(c: char) -> Option<u8> {
    let u = c as u32;
    if u <= 0xFF {
        // Latin-1 range maps 1:1 (the 0x80..=0x9F C1 controls included — they
        // occur when the original mis-decode used ISO-8859-1 rather than 1252).
        return Some(u as u8);
    }
    Some(match u {
        0x20AC => 0x80,
        0x201A => 0x82,
        0x0192 => 0x83,
        0x201E => 0x84,
        0x2026 => 0x85,
        0x2020 => 0x86,
        0x2021 => 0x87,
        0x02C6 => 0x88,
        0x2030 => 0x89,
        0x0160 => 0x8A,
        0x2039 => 0x8B,
        0x0152 => 0x8C,
        0x017D => 0x8E,
        0x2018 => 0x91,
        0x2019 => 0x92,
        0x201C => 0x93,
        0x201D => 0x94,
        0x2022 => 0x95,
        0x2013 => 0x96,
        0x2014 => 0x97,
        0x02DC => 0x98,
        0x2122 => 0x99,
        0x0161 => 0x9A,
        0x203A => 0x9B,
        0x0153 => 0x9C,
        0x017E => 0x9E,
        0x0178 => 0x9F,
        _ => return None,
    })
}

/// True when `s` contains the tell-tale signature of UTF-8-decoded-as-Latin-1:
/// a UTF-8 lead byte (`Ã` = U+00C3, `Â` = U+00C2, `â` = U+00E2, …) immediately
/// followed by a continuation byte (U+0080..=U+00BF). Plain correct text — even
/// German with `ä`/`ö`/`ü`/`ß` or a literal `€` — does not match this pattern.
fn looks_double_encoded(s: &str) -> bool {
    // Reason in reconstructed-byte space so Windows-1252 continuation bytes
    // (e.g. 0x82 → U+201A, well above U+00FF) are still recognised.
    let mut prev: Option<u8> = None;
    for c in s.chars() {
        let Some(b) = cp1252_byte(c) else {
            prev = None;
            continue;
        };
        if let Some(p) = prev {
            let lead = matches!(p, 0xC2..=0xF4);
            let cont = matches!(b, 0x80..=0xBF);
            if lead && cont {
                return true;
            }
        }
        prev = Some(b);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate the bug: correct UTF-8 bytes decoded as ISO-8859-1 (one char
    /// per byte), as a byte stream would be if mislabeled Latin-1.
    fn corrupt_latin1(s: &str) -> String {
        s.bytes().map(|b| b as char).collect()
    }

    #[test]
    fn repairs_latin1_double_encoded() {
        for original in ["Sie müssen", "Europäische", "€ 100.000", "§ 288", "Straße"] {
            let corrupted = corrupt_latin1(original);
            assert_ne!(corrupted, original, "test setup: {original:?} not corrupted");
            assert_eq!(fix_double_encoded_utf8(&corrupted), original);
        }
    }

    #[test]
    fn repairs_windows1252_euro() {
        // € (E2 82 AC) decoded as Windows-1252 → "â‚¬" (the 0x82 → U+201A).
        let corrupted = "â\u{201a}¬ 100.000";
        assert_eq!(fix_double_encoded_utf8(corrupted), "€ 100.000");
    }

    #[test]
    fn leaves_clean_text_untouched() {
        // Correct German (incl. a real €) must pass through verbatim.
        for s in [
            "Sie müssen",
            "Europäische",
            "€ 100.000",
            "§ 288",
            "Straße",
            "plain ascii",
        ] {
            assert_eq!(fix_double_encoded_utf8(s), s, "changed clean input {s:?}");
        }
    }
}
