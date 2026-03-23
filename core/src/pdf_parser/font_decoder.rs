//! Font decoder for PDF text extraction.
//!
//! Maps glyph codes from PDF content streams to Unicode characters.
//! Handles ToUnicode CMaps, predefined encodings (WinAnsi, MacRoman, Standard),
//! and `/Differences` arrays.

use lopdf::{Document, Object, ObjectId};
use std::collections::HashMap;

/// A decoded font entry, holding enough information to map glyph codes → Unicode
/// and to compute approximate glyph widths.
#[derive(Debug, Clone)]
pub struct FontEntry {
    /// Maps character codes to Unicode strings.
    decoder: CharDecoder,
    /// Glyph widths indexed by character code. Missing entries fall back to `default_width`.
    widths: HashMap<u32, f64>,
    /// Default glyph width (from `/DW` or a sensible fallback).
    default_width: f64,
    /// The base font name (e.g. "Helvetica", "ArialMT").
    pub base_font: String,
}

/// Strategy for mapping character codes to Unicode.
#[derive(Debug, Clone)]
enum CharDecoder {
    /// Parsed `/ToUnicode` CMap.
    ToUnicode(ToUnicodeCMap),
    /// A 256-entry lookup table (for single-byte encodings + Differences).
    SingleByte(Box<[Option<char>; 256]>),
    /// Identity mapping — character code IS the Unicode code point (common for CIDFonts).
    Identity,
}

// ============================================================================
// ToUnicode CMap parser
// ============================================================================

/// Parsed ToUnicode CMap (maps character codes to Unicode strings).
#[derive(Debug, Clone, Default)]
struct ToUnicodeCMap {
    /// Direct single-char mappings: code → Unicode string.
    char_map: HashMap<u32, String>,
    /// Range mappings: (start_code, end_code, start_unicode).
    ranges: Vec<(u32, u32, u32)>,
}

impl ToUnicodeCMap {
    fn decode(&self, code: u32) -> Option<String> {
        if let Some(s) = self.char_map.get(&code) {
            return Some(s.clone());
        }
        for &(start, end, base_unicode) in &self.ranges {
            if code >= start && code <= end {
                let offset = code - start;
                if let Some(ch) = char::from_u32(base_unicode + offset) {
                    return Some(ch.to_string());
                }
            }
        }
        None
    }
}

/// Parse a ToUnicode CMap stream into our representation.
fn parse_to_unicode_cmap(data: &[u8]) -> ToUnicodeCMap {
    let text = String::from_utf8_lossy(data);
    let mut cmap = ToUnicodeCMap::default();

    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let line = line.trim();

        if line.ends_with("beginbfchar") {
            // Single character mappings
            for mapping_line in lines.by_ref() {
                let mapping_line = mapping_line.trim();
                if mapping_line.contains("endbfchar") {
                    break;
                }
                if let Some((src, dst)) = parse_bfchar_line(mapping_line) {
                    cmap.char_map.insert(src, dst);
                }
            }
        } else if line.ends_with("beginbfrange") {
            // Range mappings
            for range_line in lines.by_ref() {
                let range_line = range_line.trim();
                if range_line.contains("endbfrange") {
                    break;
                }
                parse_bfrange_line(range_line, &mut cmap);
            }
        }
    }

    cmap
}

/// Parse a single line from a `beginbfchar` section.
/// Format: `<XXXX> <YYYY>` where XXXX is the source code and YYYY is the Unicode code point.
fn parse_bfchar_line(line: &str) -> Option<(u32, String)> {
    let tokens: Vec<&str> = line.split('<').filter(|s| s.contains('>')).collect();
    if tokens.len() < 2 {
        return None;
    }
    let src = u32::from_str_radix(tokens[0].split('>').next()?, 16).ok()?;
    let dst_hex = tokens[1].split('>').next()?;
    let dst = hex_to_unicode_string(dst_hex)?;
    Some((src, dst))
}

/// Parse a single line from a `beginbfrange` section.
/// Format: `<START> <END> <BASE_UNICODE>` or `<START> <END> [<U1> <U2> ...]`
fn parse_bfrange_line(line: &str, cmap: &mut ToUnicodeCMap) {
    let tokens: Vec<&str> = line.split('<').filter(|s| s.contains('>')).collect();
    if tokens.len() < 3 {
        return;
    }
    let start = match u32::from_str_radix(tokens[0].split('>').next().unwrap_or(""), 16) {
        Ok(v) => v,
        Err(_) => return,
    };
    let end = match u32::from_str_radix(tokens[1].split('>').next().unwrap_or(""), 16) {
        Ok(v) => v,
        Err(_) => return,
    };

    if line.contains('[') {
        // Array form: each code in [start, end] maps to the corresponding array element
        let array_tokens: Vec<&str> = line
            .split('[')
            .nth(1)
            .unwrap_or("")
            .split(']')
            .next()
            .unwrap_or("")
            .split('<')
            .filter(|s| s.contains('>'))
            .collect();
        for (i, tok) in array_tokens.iter().enumerate() {
            let hex = tok.split('>').next().unwrap_or("");
            if let Some(s) = hex_to_unicode_string(hex) {
                cmap.char_map.insert(start + i as u32, s);
            }
        }
    } else {
        // Simple range: <start> <end> <base_unicode>
        let base_hex = tokens[2].split('>').next().unwrap_or("");
        if let Ok(base_unicode) = u32::from_str_radix(base_hex, 16) {
            cmap.ranges.push((start, end, base_unicode));
        }
    }
}

/// Convert a hex string like "0041" or "00410042" to a Unicode string.
fn hex_to_unicode_string(hex: &str) -> Option<String> {
    let hex = hex.trim();
    if hex.is_empty() {
        return None;
    }
    // Each Unicode code point is 4 hex digits (2 bytes in UTF-16)
    let mut result = String::new();
    let mut i = 0;
    while i + 4 <= hex.len() {
        let code = u32::from_str_radix(&hex[i..i + 4], 16).ok()?;
        result.push(char::from_u32(code)?);
        i += 4;
    }
    // Handle remaining 2-digit codes (single byte)
    if i + 2 <= hex.len() && i < hex.len() {
        let code = u32::from_str_radix(&hex[i..i + 2], 16).ok()?;
        result.push(char::from_u32(code)?);
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

// ============================================================================
// Predefined PDF encodings
// ============================================================================

/// WinAnsiEncoding lookup table (Windows-1252 superset).
/// Maps byte values 0–255 to Unicode code points.
fn win_ansi_decode(code: u8) -> Option<char> {
    // 0x00–0x1F: control characters (most don't map)
    // 0x20–0x7E: ASCII
    // 0x80–0x9F: Windows-1252 specials
    // 0xA0–0xFF: Latin-1 Supplement
    match code {
        0x80 => Some('\u{20AC}'),          // Euro sign
        0x82 => Some('\u{201A}'),          // Single low-9 quotation mark
        0x83 => Some('\u{0192}'),          // Latin small letter f with hook
        0x84 => Some('\u{201E}'),          // Double low-9 quotation mark
        0x85 => Some('\u{2026}'),          // Horizontal ellipsis
        0x86 => Some('\u{2020}'),          // Dagger
        0x87 => Some('\u{2021}'),          // Double dagger
        0x88 => Some('\u{02C6}'),          // Modifier letter circumflex accent
        0x89 => Some('\u{2030}'),          // Per mille sign
        0x8A => Some('\u{0160}'),          // Latin capital letter S with caron
        0x8B => Some('\u{2039}'),          // Single left-pointing angle quotation mark
        0x8C => Some('\u{0152}'),          // Latin capital ligature OE
        0x8E => Some('\u{017D}'),          // Latin capital letter Z with caron
        0x91 => Some('\u{2018}'),          // Left single quotation mark
        0x92 => Some('\u{2019}'),          // Right single quotation mark
        0x93 => Some('\u{201C}'),          // Left double quotation mark
        0x94 => Some('\u{201D}'),          // Right double quotation mark
        0x95 => Some('\u{2022}'),          // Bullet
        0x96 => Some('\u{2013}'),          // En dash
        0x97 => Some('\u{2014}'),          // Em dash
        0x98 => Some('\u{02DC}'),          // Small tilde
        0x99 => Some('\u{2122}'),          // Trade mark sign
        0x9A => Some('\u{0161}'),          // Latin small letter s with caron
        0x9B => Some('\u{203A}'),          // Single right-pointing angle quotation mark
        0x9C => Some('\u{0153}'),          // Latin small ligature oe
        0x9E => Some('\u{017E}'),          // Latin small letter z with caron
        0x9F => Some('\u{0178}'),          // Latin capital letter Y with diaeresis
        0x81 | 0x8D | 0x8F | 0x90 => None, // Undefined in Windows-1252
        0xAD => Some('\u{00AD}'),          // Soft hyphen
        c if c >= 0x20 => char::from_u32(c as u32),
        _ => None,
    }
}

/// MacRomanEncoding lookup table.
fn mac_roman_decode(code: u8) -> Option<char> {
    // 0x00–0x7F: mostly ASCII
    if code < 0x80 {
        return char::from_u32(code as u32);
    }
    // 0x80–0xFF: Mac-specific characters
    const MAC_HIGH: [u16; 128] = [
        0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, 0x00E0, 0x00E2, 0x00E4,
        0x00E3, 0x00E5, 0x00E7, 0x00E9, 0x00E8, 0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF,
        0x00F1, 0x00F3, 0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC, 0x2020,
        0x00B0, 0x00A2, 0x00A3, 0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9, 0x2122, 0x00B4,
        0x00A8, 0x2260, 0x00C6, 0x00D8, 0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202,
        0x2211, 0x220F, 0x03C0, 0x222B, 0x00AA, 0x00BA, 0x2126, 0x00E6, 0x00F8, 0x00BF, 0x00A1,
        0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB, 0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3,
        0x00D5, 0x0152, 0x0153, 0x2013, 0x2014, 0x201C, 0x201D, 0x2018, 0x2019, 0x00F7, 0x25CA,
        0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02, 0x2021, 0x00B7, 0x201A,
        0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, 0x00CB, 0x00C8, 0x00CD, 0x00CE, 0x00CF, 0x00CC,
        0x00D3, 0x00D4, 0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC, 0x00AF,
        0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7,
    ];
    char::from_u32(MAC_HIGH[(code - 0x80) as usize] as u32)
}

/// Standard PDF encoding (Adobe Standard Encoding).
fn standard_encoding_decode(code: u8) -> Option<char> {
    // Most of ASCII is the same
    if (0x20..=0x7E).contains(&code) {
        return char::from_u32(code as u32);
    }
    // Notable differences from ASCII / Latin-1 for codes >= 0x80
    match code {
        0xA1 => Some('\u{00A1}'), // exclamdown
        0xA2 => Some('\u{00A2}'), // cent
        0xA3 => Some('\u{00A3}'), // sterling
        0xA4 => Some('\u{2044}'), // fraction
        0xA5 => Some('\u{00A5}'), // yen
        0xA6 => Some('\u{0192}'), // florin
        0xA7 => Some('\u{00A7}'), // section
        0xA8 => Some('\u{00A4}'), // currency
        0xA9 => Some('\u{0027}'), // quotesingle
        0xAA => Some('\u{201C}'), // quotedblleft
        0xAB => Some('\u{00AB}'), // guillemotleft
        0xAC => Some('\u{2039}'), // guilsinglleft
        0xAD => Some('\u{203A}'), // guilsinglright
        0xAE => Some('\u{FB01}'), // fi ligature
        0xAF => Some('\u{FB02}'), // fl ligature
        0xB1 => Some('\u{2013}'), // endash
        0xB2 => Some('\u{2020}'), // dagger
        0xB3 => Some('\u{2021}'), // daggerdbl
        0xB4 => Some('\u{00B7}'), // periodcentered
        0xB7 => Some('\u{2022}'), // bullet
        0xB8 => Some('\u{201A}'), // quotesinglbase
        0xB9 => Some('\u{201E}'), // quotedblbase
        0xBA => Some('\u{201D}'), // quotedblright
        0xBB => Some('\u{00BB}'), // guillemotright
        0xBC => Some('\u{2026}'), // ellipsis
        0xBD => Some('\u{2030}'), // perthousand
        0xC1 => Some('\u{0060}'), // grave
        0xC2 => Some('\u{00B4}'), // acute
        0xC3 => Some('\u{02C6}'), // circumflex
        0xC4 => Some('\u{02DC}'), // tilde
        0xC5 => Some('\u{00AF}'), // macron
        0xC6 => Some('\u{02D8}'), // breve
        0xC7 => Some('\u{02D9}'), // dotaccent
        0xC8 => Some('\u{00A8}'), // dieresis
        0xCA => Some('\u{02DA}'), // ring
        0xCB => Some('\u{00B8}'), // cedilla
        0xCD => Some('\u{02DD}'), // hungarumlaut
        0xCE => Some('\u{02DB}'), // ogonek
        0xCF => Some('\u{02C7}'), // caron
        0xD0 => Some('\u{2014}'), // emdash
        0xE1 => Some('\u{00C6}'), // AE
        0xE3 => Some('\u{00AA}'), // ordfeminine
        0xE8 => Some('\u{0141}'), // Lslash
        0xE9 => Some('\u{00D8}'), // Oslash
        0xEA => Some('\u{0152}'), // OE
        0xEB => Some('\u{00BA}'), // ordmasculine
        0xF1 => Some('\u{00E6}'), // ae
        0xF5 => Some('\u{0131}'), // dotlessi
        0xF8 => Some('\u{0142}'), // lslash
        0xF9 => Some('\u{00F8}'), // oslash
        0xFA => Some('\u{0153}'), // oe
        0xFB => Some('\u{00DF}'), // germandbls
        _ => None,
    }
}

/// Build a 256-entry decode table from a named encoding.
fn encoding_table(name: &str) -> Box<[Option<char>; 256]> {
    let decode_fn: fn(u8) -> Option<char> = match name {
        "MacRomanEncoding" => mac_roman_decode,
        "StandardEncoding" => standard_encoding_decode,
        _ => win_ansi_decode, // WinAnsiEncoding is the default
    };
    let mut table = Box::new([None; 256]);
    for i in 0u16..256 {
        table[i as usize] = decode_fn(i as u8);
    }
    table
}

/// Adobe glyph name → Unicode code point mapping (subset covering the most common names).
fn glyph_name_to_unicode(name: &str) -> Option<char> {
    // This is a curated subset. A full mapping would use the Adobe Glyph List.
    match name {
        "space" => Some(' '),
        "exclam" => Some('!'),
        "quotedbl" => Some('"'),
        "numbersign" => Some('#'),
        "dollar" => Some('$'),
        "percent" => Some('%'),
        "ampersand" => Some('&'),
        "quotesingle" => Some('\''),
        "parenleft" => Some('('),
        "parenright" => Some(')'),
        "asterisk" => Some('*'),
        "plus" => Some('+'),
        "comma" => Some(','),
        "hyphen" | "minus" => Some('-'),
        "period" => Some('.'),
        "slash" => Some('/'),
        "zero" => Some('0'),
        "one" => Some('1'),
        "two" => Some('2'),
        "three" => Some('3'),
        "four" => Some('4'),
        "five" => Some('5'),
        "six" => Some('6'),
        "seven" => Some('7'),
        "eight" => Some('8'),
        "nine" => Some('9'),
        "colon" => Some(':'),
        "semicolon" => Some(';'),
        "less" => Some('<'),
        "equal" => Some('='),
        "greater" => Some('>'),
        "question" => Some('?'),
        "at" => Some('@'),
        "A" => Some('A'),
        "B" => Some('B'),
        "C" => Some('C'),
        "D" => Some('D'),
        "E" => Some('E'),
        "F" => Some('F'),
        "G" => Some('G'),
        "H" => Some('H'),
        "I" => Some('I'),
        "J" => Some('J'),
        "K" => Some('K'),
        "L" => Some('L'),
        "M" => Some('M'),
        "N" => Some('N'),
        "O" => Some('O'),
        "P" => Some('P'),
        "Q" => Some('Q'),
        "R" => Some('R'),
        "S" => Some('S'),
        "T" => Some('T'),
        "U" => Some('U'),
        "V" => Some('V'),
        "W" => Some('W'),
        "X" => Some('X'),
        "Y" => Some('Y'),
        "Z" => Some('Z'),
        "bracketleft" => Some('['),
        "backslash" => Some('\\'),
        "bracketright" => Some(']'),
        "asciicircum" => Some('^'),
        "underscore" => Some('_'),
        "grave" => Some('`'),
        "a" => Some('a'),
        "b" => Some('b'),
        "c" => Some('c'),
        "d" => Some('d'),
        "e" => Some('e'),
        "f" => Some('f'),
        "g" => Some('g'),
        "h" => Some('h'),
        "i" => Some('i'),
        "j" => Some('j'),
        "k" => Some('k'),
        "l" => Some('l'),
        "m" => Some('m'),
        "n" => Some('n'),
        "o" => Some('o'),
        "p" => Some('p'),
        "q" => Some('q'),
        "r" => Some('r'),
        "s" => Some('s'),
        "t" => Some('t'),
        "u" => Some('u'),
        "v" => Some('v'),
        "w" => Some('w'),
        "x" => Some('x'),
        "y" => Some('y'),
        "z" => Some('z'),
        "braceleft" => Some('{'),
        "bar" => Some('|'),
        "braceright" => Some('}'),
        "asciitilde" => Some('~'),
        "bullet" => Some('\u{2022}'),
        "endash" => Some('\u{2013}'),
        "emdash" => Some('\u{2014}'),
        "quotedblleft" => Some('\u{201C}'),
        "quotedblright" => Some('\u{201D}'),
        "quoteleft" => Some('\u{2018}'),
        "quoteright" => Some('\u{2019}'),
        "fi" => Some('\u{FB01}'),
        "fl" => Some('\u{FB02}'),
        "ellipsis" => Some('\u{2026}'),
        "Euro" => Some('\u{20AC}'),
        "copyright" => Some('\u{00A9}'),
        "registered" => Some('\u{00AE}'),
        "trademark" => Some('\u{2122}'),
        "degree" => Some('\u{00B0}'),
        "Adieresis" => Some('\u{00C4}'),
        "Odieresis" => Some('\u{00D6}'),
        "Udieresis" => Some('\u{00DC}'),
        "adieresis" => Some('\u{00E4}'),
        "odieresis" => Some('\u{00F6}'),
        "udieresis" => Some('\u{00FC}'),
        "germandbls" | "szlig" => Some('\u{00DF}'),
        "Agrave" => Some('\u{00C0}'),
        "Aacute" => Some('\u{00C1}'),
        "Acircumflex" => Some('\u{00C2}'),
        "Atilde" => Some('\u{00C3}'),
        "Aring" => Some('\u{00C5}'),
        "AE" => Some('\u{00C6}'),
        "Ccedilla" => Some('\u{00C7}'),
        "Egrave" => Some('\u{00C8}'),
        "Eacute" => Some('\u{00C9}'),
        "Ecircumflex" => Some('\u{00CA}'),
        "Edieresis" => Some('\u{00CB}'),
        "Igrave" => Some('\u{00CC}'),
        "Iacute" => Some('\u{00CD}'),
        "Icircumflex" => Some('\u{00CE}'),
        "Idieresis" => Some('\u{00CF}'),
        "Ntilde" => Some('\u{00D1}'),
        "Ograve" => Some('\u{00D2}'),
        "Oacute" => Some('\u{00D3}'),
        "Ocircumflex" => Some('\u{00D4}'),
        "Otilde" => Some('\u{00D5}'),
        "Oslash" => Some('\u{00D8}'),
        "Ugrave" => Some('\u{00D9}'),
        "Uacute" => Some('\u{00DA}'),
        "Ucircumflex" => Some('\u{00DB}'),
        "Yacute" => Some('\u{00DD}'),
        "agrave" => Some('\u{00E0}'),
        "aacute" => Some('\u{00E1}'),
        "acircumflex" => Some('\u{00E2}'),
        "atilde" => Some('\u{00E3}'),
        "aring" => Some('\u{00E5}'),
        "ae" => Some('\u{00E6}'),
        "ccedilla" => Some('\u{00E7}'),
        "egrave" => Some('\u{00E8}'),
        "eacute" => Some('\u{00E9}'),
        "ecircumflex" => Some('\u{00EA}'),
        "edieresis" => Some('\u{00EB}'),
        "igrave" => Some('\u{00EC}'),
        "iacute" => Some('\u{00ED}'),
        "icircumflex" => Some('\u{00EE}'),
        "idieresis" => Some('\u{00EF}'),
        "ntilde" => Some('\u{00F1}'),
        "ograve" => Some('\u{00F2}'),
        "oacute" => Some('\u{00F3}'),
        "ocircumflex" => Some('\u{00F4}'),
        "otilde" => Some('\u{00F5}'),
        "oslash" => Some('\u{00F8}'),
        "ugrave" => Some('\u{00F9}'),
        "uacute" => Some('\u{00FA}'),
        "ucircumflex" => Some('\u{00FB}'),
        "ydieresis" => Some('\u{00FF}'),
        "section" => Some('\u{00A7}'),
        "paragraph" => Some('\u{00B6}'),
        // If the name looks like "uniXXXX" or "uXXXX", parse the hex
        _ => {
            if let Some(hex) = name.strip_prefix("uni") {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(hex) = name.strip_prefix("u") {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else {
                None
            }
        }
    }
}

// ============================================================================
// FontMap — per-page font registry
// ============================================================================

/// A collection of font decoders for a page, keyed by the font resource name
/// (e.g. `/F1`, `/TT0`, `/C2_0`).
pub type FontMap = HashMap<String, FontEntry>;

impl FontEntry {
    /// Decode a single character code to a Unicode string.
    pub fn decode_char(&self, code: u32) -> String {
        match &self.decoder {
            CharDecoder::ToUnicode(cmap) => cmap.decode(code).unwrap_or_else(|| replacement(code)),
            CharDecoder::SingleByte(table) => {
                if code < 256 {
                    table[code as usize]
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| replacement(code))
                } else {
                    replacement(code)
                }
            }
            CharDecoder::Identity => char::from_u32(code)
                .map(|c| c.to_string())
                .unwrap_or_else(|| replacement(code)),
        }
    }

    /// Decode a byte slice according to this font's encoding.
    /// For single-byte fonts each byte is one character code.
    /// For CIDFonts (Identity / ToUnicode with 2-byte codes) we try 2-byte big-endian.
    pub fn decode_bytes(&self, bytes: &[u8]) -> String {
        match &self.decoder {
            CharDecoder::Identity | CharDecoder::ToUnicode(_) if self.is_two_byte() => {
                // 2-byte CIDFont
                let mut result = String::new();
                let mut i = 0;
                while i + 1 < bytes.len() {
                    let code = u32::from(bytes[i]) << 8 | u32::from(bytes[i + 1]);
                    result.push_str(&self.decode_char(code));
                    i += 2;
                }
                // Trailing single byte (shouldn't happen but handle gracefully)
                if i < bytes.len() {
                    result.push_str(&self.decode_char(u32::from(bytes[i])));
                }
                result
            }
            _ => {
                // Single-byte encoding
                let mut result = String::new();
                for &b in bytes {
                    result.push_str(&self.decode_char(u32::from(b)));
                }
                result
            }
        }
    }

    /// Get the width of a character code in text-space units (typically 1/1000 of text space).
    pub fn char_width(&self, code: u32) -> f64 {
        self.widths
            .get(&code)
            .copied()
            .unwrap_or(self.default_width)
    }

    /// Whether this font uses 2-byte character codes (CIDFont).
    pub fn is_two_byte_public(&self) -> bool {
        self.is_two_byte()
    }

    /// Whether this font uses 2-byte character codes (CIDFont).
    fn is_two_byte(&self) -> bool {
        // Heuristic: if all width entries have codes > 255, it's likely CID
        // Also Identity decoder is always 2-byte
        matches!(&self.decoder, CharDecoder::Identity) || self.widths.keys().any(|&k| k > 255)
    }
}

fn replacement(code: u32) -> String {
    // Try as raw Unicode code point first; fall back to replacement character
    char::from_u32(code)
        .map(|c| c.to_string())
        .unwrap_or_else(|| '\u{FFFD}'.to_string())
}

// ============================================================================
// Embedded font extraction
// ============================================================================

/// Raw embedded font data extracted from a PDF.
pub struct RawEmbeddedFont {
    /// Font name from the PDF's /BaseFont entry (e.g. "Helvetica-Bold", "ArialMT")
    pub base_font: String,
    /// Raw TrueType font bytes from /FontFile2
    pub data: Vec<u8>,
}

/// Extract all embedded TrueType fonts from a PDF document.
///
/// Iterates every page's `/Resources/Font` dictionary, resolves each font's
/// `/FontDescriptor` → `/FontFile2` stream (for simple fonts) or
/// `/DescendantFonts[0]` → `/FontDescriptor` → `/FontFile2` (for Type0/CID fonts),
/// decompresses the stream, and returns the raw TTF bytes along with the base font name.
///
/// Only TrueType fonts (`/FontFile2`) are extracted; Type1 (`/FontFile`) and
/// CFF/OpenType (`/FontFile3`) are skipped.
pub fn extract_embedded_fonts(doc: &Document) -> Vec<RawEmbeddedFont> {
    let mut result = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    let pages = doc.get_pages();

    for page_id in pages.values() {
        let page_obj = match doc.get_object(*page_id) {
            Ok(obj) => obj,
            Err(_) => continue,
        };

        let Some(fonts) = get_fonts_dict(doc, page_obj) else {
            continue;
        };

        for (_resource_name, obj) in fonts {
            let font_obj = match resolve_object(doc, obj) {
                Some(o) => o,
                None => continue,
            };

            let font_dict = match font_obj.as_dict() {
                Ok(d) => d,
                Err(_) => continue,
            };

            // Get base font name
            let base_font = font_dict
                .get(b"BaseFont")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .unwrap_or_default();

            if base_font.is_empty() || seen_names.contains(&base_font) {
                continue;
            }

            // Try to extract TrueType font data via FontDescriptor (/FontFile2)
            if let Some(data) = extract_font_file2(doc, font_dict) {
                seen_names.insert(base_font.clone());
                result.push(RawEmbeddedFont { base_font, data });
                continue;
            }

            // Try to extract OpenType CFF font data via FontDescriptor (/FontFile3)
            if let Some(data) = extract_font_file3(doc, font_dict) {
                seen_names.insert(base_font.clone());
                result.push(RawEmbeddedFont { base_font, data });
                continue;
            }

            // For Type0 (CID) fonts, look through DescendantFonts
            if let Ok(descendants) = font_dict.get(b"DescendantFonts") {
                let desc_array = match descendants {
                    Object::Array(arr) => Some(arr.as_slice()),
                    Object::Reference(r) => doc.get_object(*r).ok().and_then(|o| {
                        if let Object::Array(arr) = o {
                            Some(arr.as_slice())
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };

                if let Some(arr) = desc_array {
                    for item in arr {
                        let cid_obj = match item {
                            Object::Reference(r) => doc.get_object(*r).ok(),
                            other => Some(other),
                        };
                        if let Some(cid_obj) = cid_obj {
                            if let Ok(cid_dict) = cid_obj.as_dict() {
                                if let Some(data) = extract_font_file2(doc, cid_dict) {
                                    seen_names.insert(base_font.clone());
                                    result.push(RawEmbeddedFont {
                                        base_font: base_font.clone(),
                                        data,
                                    });
                                    break;
                                }
                                // Also try /FontFile3 for CID fonts
                                if !seen_names.contains(&base_font) {
                                    if let Some(data) = extract_font_file3(doc, cid_dict) {
                                        seen_names.insert(base_font.clone());
                                        result.push(RawEmbeddedFont {
                                            base_font: base_font.clone(),
                                            data,
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

/// Try to extract /FontFile2 (TrueType) data from a font dictionary's /FontDescriptor.
fn extract_font_file2(doc: &Document, font_dict: &lopdf::Dictionary) -> Option<Vec<u8>> {
    let fd_obj = font_dict.get(b"FontDescriptor").ok()?;
    let fd_resolved = resolve_object(doc, fd_obj)?;
    let fd_dict = fd_resolved.as_dict().ok()?;
    let ff2_obj = fd_dict.get(b"FontFile2").ok()?;
    resolve_stream_data(doc, ff2_obj)
}

/// Try to extract /FontFile3 (CFF/OpenType CFF) data from a font dictionary's /FontDescriptor.
///
/// `/FontFile3` entries can have Subtype `CIDFontType0C`, `Type1C`, or `OpenType`.
/// Only the `OpenType` subtype can be loaded by ab_glyph (it wraps a full
/// OpenType/CFF font program). The Type1C and CIDFontType0C subtypes are
/// bare CFF data that ab_glyph cannot parse, so we skip those.
fn extract_font_file3(doc: &Document, font_dict: &lopdf::Dictionary) -> Option<Vec<u8>> {
    let fd_obj = font_dict.get(b"FontDescriptor").ok()?;
    let fd_resolved = resolve_object(doc, fd_obj)?;
    let fd_dict = fd_resolved.as_dict().ok()?;
    let ff3_obj = fd_dict.get(b"FontFile3").ok()?;

    // Resolve the stream and check its Subtype
    let ff3_resolved = resolve_object(doc, ff3_obj)?;
    let ff3_stream = ff3_resolved.as_stream().ok()?;
    let subtype = ff3_stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).to_string())
        .unwrap_or_default();

    // Only accept OpenType subtype — Type1C and CIDFontType0C are bare CFF
    // data that ab_glyph cannot load
    if subtype != "OpenType" {
        return None;
    }

    resolve_stream_data(doc, ff3_obj)
}

// ============================================================================
// Build FontMap from a page's /Resources /Font dictionary
// ============================================================================

/// Build a [`FontMap`] from a PDF page's resource dictionary.
///
/// For each font referenced in `/Resources` → `/Font`, this extracts the
/// encoding information (ToUnicode CMap, predefined encoding, Differences)
/// and glyph widths.
pub fn build_font_map(doc: &Document, page_id: ObjectId) -> FontMap {
    let mut map = FontMap::new();

    // Get the page object
    let page_obj = match doc.get_object(page_id) {
        Ok(obj) => obj,
        Err(_) => return map,
    };

    // Navigate to /Resources -> /Font
    let fonts_dict = get_fonts_dict(doc, page_obj);
    let Some(fonts_dict) = fonts_dict else {
        return map;
    };

    for (name, obj) in fonts_dict {
        let font_obj = match resolve_object(doc, obj) {
            Some(o) => o,
            None => continue,
        };

        if let Some(entry) = build_font_entry(doc, font_obj) {
            map.insert(name.clone(), entry);
        }
    }

    map
}

/// Navigate from a page object to its /Resources /Font dictionary.
/// Returns a Vec of (font_name, &Object) pairs.
fn get_fonts_dict<'a>(
    doc: &'a Document,
    page_obj: &'a Object,
) -> Option<Vec<(String, &'a Object)>> {
    let page_dict = page_obj.as_dict().ok()?;

    // Try /Resources directly on the page, or resolve a reference
    let resources = match page_dict.get(b"Resources") {
        Ok(res) => match res {
            Object::Reference(r) => doc.get_object(*r).ok()?,
            other => other,
        },
        Err(_) => return None,
    };

    let resources_dict = resources.as_dict().ok()?;

    let font_obj = match resources_dict.get(b"Font") {
        Ok(f) => match f {
            Object::Reference(r) => doc.get_object(*r).ok()?,
            other => other,
        },
        Err(_) => return None,
    };

    let font_dict = font_obj.as_dict().ok()?;

    let mut result = Vec::new();
    for (key, val) in font_dict.iter() {
        let name = String::from_utf8_lossy(key).to_string();
        result.push((name, val));
    }
    Some(result)
}

/// Build a single FontEntry from a font dictionary object.
fn build_font_entry(doc: &Document, font_obj: &Object) -> Option<FontEntry> {
    let font_dict = font_obj.as_dict().ok()?;

    // Get base font name
    let base_font = font_dict
        .get(b"BaseFont")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    // Determine the decoder
    let decoder = build_decoder(doc, font_dict);

    // Extract widths
    let (widths, default_width) = extract_widths(doc, font_dict);

    Some(FontEntry {
        decoder,
        widths,
        default_width,
        base_font,
    })
}

/// Build a CharDecoder from a font dictionary.
/// Priority: ToUnicode > Encoding with Differences > Named Encoding > Identity
fn build_decoder(doc: &Document, font_dict: &lopdf::Dictionary) -> CharDecoder {
    // 1. Check for /ToUnicode CMap
    if let Ok(to_unicode) = font_dict.get(b"ToUnicode") {
        if let Some(cmap_data) = resolve_stream_data(doc, to_unicode) {
            let cmap = parse_to_unicode_cmap(&cmap_data);
            if !cmap.char_map.is_empty() || !cmap.ranges.is_empty() {
                return CharDecoder::ToUnicode(cmap);
            }
        }
    }

    // 2. Check for /Encoding
    if let Ok(encoding) = font_dict.get(b"Encoding") {
        match encoding {
            Object::Name(name) => {
                let name_str = String::from_utf8_lossy(name);
                if name_str == "Identity-H" || name_str == "Identity-V" {
                    return CharDecoder::Identity;
                }
                return CharDecoder::SingleByte(encoding_table(&name_str));
            }
            Object::Dictionary(_) | Object::Reference(_) => {
                let enc_obj = if let Object::Reference(r) = encoding {
                    doc.get_object(*r).ok()
                } else {
                    Some(encoding)
                };
                if let Some(enc_obj) = enc_obj {
                    if let Ok(enc_dict) = enc_obj.as_dict() {
                        // Get base encoding
                        let base_name = enc_dict
                            .get(b"BaseEncoding")
                            .ok()
                            .and_then(|o| o.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).to_string())
                            .unwrap_or_else(|| "WinAnsiEncoding".to_string());

                        let mut table = encoding_table(&base_name);

                        // Apply /Differences
                        if let Ok(Object::Array(diffs)) = enc_dict.get(b"Differences") {
                            apply_differences(&mut table, diffs);
                        }

                        return CharDecoder::SingleByte(table);
                    }
                }
            }
            _ => {}
        }
    }

    // 3. Check for CIDFont subtype (Type0 composite font)
    if let Ok(subtype) = font_dict.get(b"Subtype") {
        if let Ok(name) = subtype.as_name() {
            if name == b"Type0" {
                // Check /Encoding on the Type0 font itself
                if let Ok(enc) = font_dict.get(b"Encoding") {
                    if let Ok(name) = enc.as_name() {
                        let name_str = String::from_utf8_lossy(name);
                        if name_str == "Identity-H" || name_str == "Identity-V" {
                            return CharDecoder::Identity;
                        }
                    }
                }
                return CharDecoder::Identity;
            }
        }
    }

    // 4. Fallback: WinAnsiEncoding (most common for simple fonts)
    CharDecoder::SingleByte(encoding_table("WinAnsiEncoding"))
}

/// Apply a `/Differences` array to an encoding table.
/// Format: [code1 /name1 /name2 ... code2 /name3 ...]
fn apply_differences(table: &mut Box<[Option<char>; 256]>, diffs: &[Object]) {
    let mut current_code: Option<u32> = None;

    for obj in diffs {
        match obj {
            Object::Integer(n) => {
                current_code = Some(*n as u32);
            }
            Object::Name(name) => {
                if let Some(code) = current_code {
                    if code < 256 {
                        let name_str = String::from_utf8_lossy(name);
                        if let Some(ch) = glyph_name_to_unicode(&name_str) {
                            table[code as usize] = Some(ch);
                        }
                    }
                    current_code = Some(code + 1);
                }
            }
            _ => {}
        }
    }
}

/// Extract glyph widths from a font dictionary.
/// Returns (widths_map, default_width).
fn extract_widths(doc: &Document, font_dict: &lopdf::Dictionary) -> (HashMap<u32, f64>, f64) {
    let mut widths = HashMap::new();
    let default_width = 600.0; // Reasonable default for monospaced-like fonts

    // Try /Widths array (for simple fonts)
    if let (Ok(first_char), Ok(widths_obj)) =
        (font_dict.get(b"FirstChar"), font_dict.get(b"Widths"))
    {
        let first = match first_char {
            Object::Integer(n) => *n as u32,
            Object::Reference(r) => doc
                .get_object(*r)
                .ok()
                .and_then(|o| {
                    if let Object::Integer(n) = o {
                        Some(*n as u32)
                    } else {
                        None
                    }
                })
                .unwrap_or(0),
            _ => 0,
        };

        let width_array = match widths_obj {
            Object::Array(arr) => Some(arr),
            Object::Reference(r) => doc.get_object(*r).ok().and_then(|o| {
                if let Object::Array(arr) = o {
                    Some(arr)
                } else {
                    None
                }
            }),
            _ => None,
        };

        if let Some(arr) = width_array {
            for (i, obj) in arr.iter().enumerate() {
                let w = match obj {
                    Object::Integer(n) => *n as f64,
                    Object::Real(f) => *f as f64,
                    _ => continue,
                };
                widths.insert(first + i as u32, w);
            }
        }
    }

    // Try /DescendantFonts for Type0 (CID) fonts
    if let Ok(descendants) = font_dict.get(b"DescendantFonts") {
        let desc_array = match descendants {
            Object::Array(arr) => Some(arr),
            Object::Reference(r) => doc.get_object(*r).ok().and_then(|o| {
                if let Object::Array(arr) = o {
                    Some(arr)
                } else {
                    None
                }
            }),
            _ => None,
        };

        if let Some(arr) = desc_array {
            for item in arr {
                let cid_font = match item {
                    Object::Reference(r) => doc.get_object(*r).ok(),
                    other => Some(other),
                };
                if let Some(cid_obj) = cid_font {
                    if let Ok(cid_dict) = cid_obj.as_dict() {
                        // /DW (default width)
                        if let Ok(dw) = cid_dict.get(b"DW") {
                            if let Object::Integer(n) = dw {
                                return (widths, *n as f64);
                            }
                        }

                        // /W array (CID widths): [cid_start [w1 w2 ...] cid_start cid_end w ...]
                        if let Ok(w_obj) = cid_dict.get(b"W") {
                            let w_array = match w_obj {
                                Object::Array(arr) => Some(arr),
                                Object::Reference(r) => doc.get_object(*r).ok().and_then(|o| {
                                    if let Object::Array(arr) = o {
                                        Some(arr)
                                    } else {
                                        None
                                    }
                                }),
                                _ => None,
                            };
                            if let Some(w_arr) = w_array {
                                parse_cid_widths(&mut widths, w_arr, doc);
                            }
                        }
                    }
                }
            }
        }
    }

    (widths, default_width)
}

/// Parse a CID /W array.
/// Format: [cid [w1 w2 ...]] or [cid_first cid_last w]
fn parse_cid_widths(widths: &mut HashMap<u32, f64>, arr: &[Object], doc: &Document) {
    let mut i = 0;
    while i < arr.len() {
        let start_cid = match &arr[i] {
            Object::Integer(n) => *n as u32,
            _ => {
                i += 1;
                continue;
            }
        };
        i += 1;
        if i >= arr.len() {
            break;
        }

        match &arr[i] {
            Object::Array(w_list) => {
                // [start_cid [w1 w2 w3 ...]]
                for (j, w_obj) in w_list.iter().enumerate() {
                    let w = match w_obj {
                        Object::Integer(n) => *n as f64,
                        Object::Real(f) => *f as f64,
                        _ => continue,
                    };
                    widths.insert(start_cid + j as u32, w);
                }
                i += 1;
            }
            Object::Reference(r) => {
                // Could be a reference to an array
                if let Ok(Object::Array(w_list)) = doc.get_object(*r) {
                    for (j, w_obj) in w_list.iter().enumerate() {
                        let w = match w_obj {
                            Object::Integer(n) => *n as f64,
                            Object::Real(f) => *f as f64,
                            _ => continue,
                        };
                        widths.insert(start_cid + j as u32, w);
                    }
                }
                i += 1;
            }
            Object::Integer(_) => {
                // [start_cid end_cid width]
                let end_cid = match &arr[i] {
                    Object::Integer(n) => *n as u32,
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                i += 1;
                if i >= arr.len() {
                    break;
                }
                let w = match &arr[i] {
                    Object::Integer(n) => *n as f64,
                    Object::Real(f) => *f as f64,
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                for cid in start_cid..=end_cid {
                    widths.insert(cid, w);
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
}

/// Resolve an object, following references.
fn resolve_object<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(r) => doc.get_object(*r).ok(),
        other => Some(other),
    }
}

/// Get the raw bytes from a stream object (follows references, decompresses).
fn resolve_stream_data(doc: &Document, obj: &Object) -> Option<Vec<u8>> {
    let stream_obj = match obj {
        Object::Reference(r) => doc.get_object(*r).ok()?,
        other => other,
    };

    match stream_obj {
        Object::Stream(stream) => {
            // Try to get decompressed content
            let mut stream_clone = stream.clone();
            stream_clone.decompress();
            Some(stream_clone.content.clone())
        }
        _ => None,
    }
}
