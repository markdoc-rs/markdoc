//! Port of common/utils.ts: utility functions markdown-it's rules (and plugins) use.
//!
//! `callable` (a JS class wrapper) and `lib` (re-exported packages) have no Rust
//! counterpart. `decode_html_strict` is the `entities` package function utils.ts and
//! entity.ts import, ported here over the generated table in [`super::entities`].

use super::{entities, ucmicro};
use crate::markdown_it::js;

/// `arrayReplaceAt(src, pos, newElements)`: a copy of `src` with the element at `pos`
/// replaced by `new_elements` (`pos` past the end appends them, as in JS).
pub fn array_replace_at<T: Clone>(src: &[T], pos: usize, new_elements: Vec<T>) -> Vec<T> {
    let mut out: Vec<T> = src[..pos.min(src.len())].to_vec();
    out.extend(new_elements);
    // `src.slice(pos + 1)`: empty from the end on
    if let Some(rest) = src.get(pos.saturating_add(1)..) {
        out.extend_from_slice(rest);
    }
    out
}

/// `isValidEntityCode(c)`: whether a code point can be decoded from a numeric entity.
pub fn is_valid_entity_code(c: u32) -> bool {
    // broken sequence
    if (0xD800..=0xDFFF).contains(&c) {
        return false;
    }
    // never used
    if (0xFDD0..=0xFDEF).contains(&c) {
        return false;
    }
    if (c & 0xFFFF) == 0xFFFF || (c & 0xFFFF) == 0xFFFE {
        return false;
    }
    // control codes
    if c <= 0x08 {
        return false;
    }
    if c == 0x0B {
        return false;
    }
    if (0x0E..=0x1F).contains(&c) {
        return false;
    }
    if (0x7F..=0x9F).contains(&c) {
        return false;
    }
    // out of range
    if c > 0x10FFFF {
        return false;
    }
    true
}

/// `fromCodePoint(c)`: like `String.fromCodePoint`, but never throws (each half is
/// truncated to 16 bits, as `String.fromCharCode` does).
pub fn from_code_point(c: u32) -> Vec<u16> {
    if c > 0xffff {
        let c = c - 0x10000;
        let surrogate1 = 0xd800 + (c >> 10);
        let surrogate2 = 0xdc00 + (c & 0x3ff);

        return js::from_char_code(&[surrogate1, surrogate2]);
    }
    js::from_char_code(&[c])
}

/// The characters of `UNESCAPE_MD_RE`'s class: `[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]`.
fn is_escapable(c: u16) -> bool {
    matches!(c, 0x21..=0x2F | 0x3A..=0x40 | 0x5B..=0x60 | 0x7B..=0x7E)
}

/// Matches `UNESCAPE_MD_RE` = `/\\([!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~])/` at `i`: the
/// escaped character.
fn match_escape(s: &[u16], i: usize) -> Option<u16> {
    if s[i] == u16::from(b'\\')
        && let Some(&c) = s.get(i + 1)
        && is_escapable(c)
    {
        return Some(c);
    }
    None
}

/// Matches `ENTITY_RE` = `/&([a-z#][a-z0-9]{1,31});/i` at `i`: the length of the match.
/// With the `i` flag (non-unicode) `[a-z]` is exactly `[A-Za-z]`. Greedy `{1,31}` then
/// `;`: backtracking cannot find a `;` inside an alphanumeric run, so the run must be 1..=31
/// long and followed by `;`.
fn match_entity(s: &[u16], i: usize) -> Option<usize> {
    if s[i] != u16::from(b'&') {
        return None;
    }
    let first = *s.get(i + 1)?;
    if !(first == u16::from(b'#') || (first < 0x80 && (first as u8).is_ascii_alphabetic())) {
        return None;
    }
    let run = s[i + 2..]
        .iter()
        .take_while(|&&c| c < 0x80 && (c as u8).is_ascii_alphanumeric())
        .count();
    if (1..=31).contains(&run) && s.get(i + 2 + run) == Some(&u16::from(b';')) {
        return Some(run + 3);
    }
    None
}

/// `DIGITAL_ENTITY_TEST_RE.test(name)` = `/^#((?:x[a-f0-9]{1,8}|[0-9]{1,8}))$/i`.
fn is_digital_entity(name: &[u16]) -> bool {
    let is_hex = |c: &u16| *c < 0x80 && (*c as u8).is_ascii_hexdigit();
    let is_dec = |c: &u16| *c < 0x80 && (*c as u8).is_ascii_digit();
    if name.first() != Some(&u16::from(b'#')) {
        return false;
    }
    let rest = &name[1..];
    if rest
        .first()
        .is_some_and(|&c| c == u16::from(b'x') || c == u16::from(b'X'))
        && (1..=8).contains(&(rest.len() - 1))
        && rest[1..].iter().all(is_hex)
    {
        return true;
    }
    (1..=8).contains(&rest.len()) && rest.iter().all(is_dec)
}

/// `replaceEntityPattern(match, name)`.
fn replace_entity_pattern(m: &[u16], name: &[u16]) -> Vec<u16> {
    if name[0] == 0x23 /* # */ && is_digital_entity(name) {
        let code = if js::to_lower_case(&name[1..2]) == [u16::from(b'x')] {
            js::parse_int(&name[2..], 16)
        } else {
            js::parse_int(&name[1..], 10)
        };
        // at most eight digits: an integer below 2^32
        let code = code as u32;

        if is_valid_entity_code(code) {
            return from_code_point(code);
        }

        return m.to_vec();
    }

    let decoded = decode_html_strict(m);
    if decoded != m {
        return decoded;
    }

    m.to_vec()
}

/// `unescapeMd(str)`: decode Markdown backslash escapes.
pub fn unescape_md(s: &[u16]) -> Vec<u16> {
    if js::index_of(s, &[u16::from(b'\\')], 0).is_none() {
        return s.to_vec();
    }
    // str.replace(UNESCAPE_MD_RE, '$1')
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some(c) = match_escape(s, i) {
            out.push(c);
            i += 2;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// `unescapeAll(str)`: decode Markdown backslash escapes and HTML character references
/// (link destinations, link titles, fenced code info strings).
///
/// `UNESCAPE_ALL_RE` = `UNESCAPE_MD_RE|ENTITY_RE` with the `gi` flags, replaced left to
/// right; the two alternatives start with different characters, so at most one matches.
pub fn unescape_all(s: &[u16]) -> Vec<u16> {
    if js::index_of(s, &[u16::from(b'\\')], 0).is_none()
        && js::index_of(s, &[u16::from(b'&')], 0).is_none()
    {
        return s.to_vec();
    }

    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some(escaped) = match_escape(s, i) {
            out.push(escaped);
            i += 2;
        } else if let Some(len) = match_entity(s, i) {
            let m = &s[i..i + len];
            out.extend(replace_entity_pattern(m, &m[1..len - 1]));
            i += len;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// `escapeHtml(str)`: escape `&`, `<`, `>` and `"`.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// `escapeRE(str)`: escape regular expression metacharacters (`.?*+^$[]\(){}|-`).
pub fn escape_re(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".?*+^$[]\\(){}|-".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `isSpace(code)`: an ASCII space or tab.
pub fn is_space(code: impl Into<u32>) -> bool {
    matches!(code.into(), 0x09 | 0x20)
}

/// `isWhiteSpace(code)`: whitespace recognized by Markdown (Unicode `Zs` and
/// `\t`, `\f`, `\v`, `\r`, `\n`).
pub fn is_white_space(code: impl Into<u32>) -> bool {
    let code = code.into();
    if (0x2000..=0x200A).contains(&code) {
        return true;
    }
    matches!(
        code,
        0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x20 | 0xA0 | 0x1680 | 0x202F | 0x205F | 0x3000
    )
}

/// `isPunctChar(ch)`: `ucmicro.P.test(ch) || ucmicro.S.test(ch)`. The regexes are not
/// anchored, so this is true when any character of `ch` is punctuation or a symbol; they
/// match BMP characters by code unit and astral ones by surrogate pair, never a lone
/// surrogate.
pub fn is_punct_char(ch: &[u16]) -> bool {
    (0..ch.len()).any(|i| {
        let c = u32::from(ch[i]);
        if (0xD800..=0xDBFF).contains(&c)
            && let Some(&low) = ch.get(i + 1)
            && (0xDC00..=0xDFFF).contains(&low)
        {
            let cp = 0x10000 + ((c - 0xD800) << 10) + (u32::from(low) - 0xDC00);
            return ucmicro::is_p(cp) || ucmicro::is_s(cp);
        }
        ucmicro::is_p(c) || ucmicro::is_s(c)
    })
}

/// `isPunctCharCode(code)`: `isPunctChar(fromCodePoint(code))`, for a code point (the
/// generated tables cover 0..=0x10FFFF, surrogates included, exactly as this evaluates).
pub fn is_punct_char_code(code: u32) -> bool {
    is_punct_char(&from_code_point(code))
}

/// `isMdAsciiPunct(ch)`: Markdown ASCII punctuation characters.
///
/// ```text
/// !, ", #, $, %, &, ', (, ), *, +, ,, -, ., /, :, ;, <, =, >, ?, @,
/// [, \, ], ^, _, `, {, |, }, or ~
/// ```
///
/// <http://spec.commonmark.org/0.15/#ascii-punctuation-character>
///
/// Don't confuse with Unicode punctuation. It lacks some characters in the
/// ASCII range.
pub fn is_md_ascii_punct(ch: impl Into<u32>) -> bool {
    matches!(ch.into(), 0x21..=0x2F | 0x3A..=0x40 | 0x5B..=0x60 | 0x7B..=0x7E)
}

/// `normalizeReference(str)`: normalize `[reference labels]` for case-insensitive lookup.
pub fn normalize_reference(s: &[u16]) -> String {
    // Trim and collapse whitespace
    //
    // str = str.trim().replace(/\s+/g, ' ')
    let mut collapsed: Vec<u16> = Vec::with_capacity(s.len());
    let mut in_space = false;
    for &c in js::trim(s) {
        if js::is_js_space(u32::from(c)) {
            if !in_space {
                collapsed.push(0x20);
            }
            in_space = true;
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }

    // .toLowerCase().toUpperCase() should get rid of all differences
    // between letter variants.
    //
    // Simple .toLowerCase() doesn't normalize 125 code points correctly,
    // and .toUpperCase doesn't normalize 6 of them (list of exceptions:
    // İ, ϴ, ẞ, Ω, K, Å - those are already uppercased, but have differently
    // uppercased versions).
    //
    // Here's an example showing how it happens. Lets take greek letter omega:
    // uppercase U+0398 (Θ), U+03f4 (ϴ) and lowercase U+03b8 (θ), U+03d1 (ϑ)
    //
    // Unicode entries:
    // 0398;GREEK CAPITAL LETTER THETA;Lu;0;L;;;;;N;;;;03B8;
    // 03B8;GREEK SMALL LETTER THETA;Ll;0;L;;;;;N;;;0398;;0398
    // 03D1;GREEK THETA SYMBOL;Ll;0;L;<compat> 03B8;;;;N;GREEK SMALL LETTER SCRIPT THETA;;0398;;0398
    // 03F4;GREEK CAPITAL THETA SYMBOL;Lu;0;L;<compat> 0398;;;;N;;;;03B8;
    //
    // Case-insensitive comparison should treat all of them as equivalent.
    //
    // But .toLowerCase() doesn't change ϑ (it's already lowercase),
    // and .toUpperCase() doesn't change ϴ (already uppercase).
    //
    // Applying first lower then upper case normalizes any character:
    // 'Θϴθϑ'.toLowerCase().toUpperCase() === 'ΘΘΘΘ'
    //
    // Note: this is equivalent to unicode case folding; unicode normalization
    // is a different step that is not required here.
    //
    // Final result should be uppercased, because it's later stored in an object
    // (this avoid a conflict with Object.prototype members,
    // most notably, `__proto__`)
    //
    js::to_string(&js::to_upper_case(&js::to_lower_case(&collapsed)))
}

fn is_ascii_trimmable(c: u16) -> bool {
    c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d
}

/// `asciiTrim(str)`: "light" `.trim()` for blocks (headings, paragraphs), where Unicode
/// spaces should be preserved.
pub fn ascii_trim(s: &[u16]) -> &[u16] {
    let mut start = 0;
    while start < s.len() {
        if !is_ascii_trimmable(s[start]) {
            break;
        }
        start += 1;
    }
    let mut end = s.len();
    while end > start {
        if !is_ascii_trimmable(s[end - 1]) {
            break;
        }
        end -= 1;
    }
    &s[start..end]
}

/// `replaceCodePoint` of the `entities` package (decode-codepoint.js).
fn replace_code_point(code_point: u64) -> u32 {
    if (0xD800..=0xDFFF).contains(&code_point) || code_point > 0x10FFFF {
        return 0xFFFD;
    }
    // C1 Unicode control character reference replacements
    match code_point {
        0 => 0xFFFD,
        128 => 8364,
        130 => 8218,
        131 => 402,
        132 => 8222,
        133 => 8230,
        134 => 8224,
        135 => 8225,
        136 => 710,
        137 => 8240,
        138 => 352,
        139 => 8249,
        140 => 338,
        142 => 381,
        145 => 8216,
        146 => 8217,
        147 => 8220,
        148 => 8221,
        149 => 8226,
        150 => 8211,
        151 => 8212,
        152 => 732,
        153 => 8482,
        154 => 353,
        155 => 8250,
        156 => 339,
        158 => 382,
        159 => 376,
        other => other as u32,
    }
}

/// `EntityDecoder.write` in strict mode for the entity whose `&` is at `amp`: the number of
/// characters consumed (0: not an entity), after pushing its decoding to `out`.
///
/// Numeric (`&#` decimal or `&#x` hex digits): needs at least one digit and a `;`; the
/// value goes through `replaceCodePoint` (so huge or surrogate values become U+FFFD).
/// Named: strict mode only emits entities terminated by `;`, so the characters between `&`
/// and `;` must be exactly a name in the table (names are alphanumeric; the trie walk fails
/// on anything else, and a walk that runs off the end of the input decodes nothing).
fn decode_entity_strict(input: &[u16], amp: usize, out: &mut Vec<u16>) -> usize {
    let mut offset = amp + 1;
    if input.get(offset) == Some(&u16::from(b'#')) {
        let mut consumed: usize = 2;
        offset += 1;
        let Some(&c) = input.get(offset) else {
            return 0; // incomplete; end() gives 0
        };
        let (radix, expected) = if (c | 0x20) == u16::from(b'x') {
            consumed += 1;
            offset += 1;
            (16u64, 3usize)
        } else {
            (10u64, 2usize)
        };
        // saturating: any value past 0x10FFFF ends up as U+FFFD, as JS's float does
        let mut result: u64 = 0;
        while let Some(&c) = input.get(offset) {
            let digit = match c {
                0x30..=0x39 => u64::from(c - 0x30),
                0x41..=0x46 | 0x61..=0x66 if radix == 16 => u64::from((c | 0x20) - 0x61 + 10),
                _ => break,
            };
            result = result.saturating_mul(radix).saturating_add(digit);
            consumed += 1;
            offset += 1;
        }
        let Some(&last) = input.get(offset) else {
            return 0; // ran off the end: end() emits nothing in strict mode
        };
        if consumed <= expected {
            return 0; // absence of digits
        }
        if last != u16::from(b';') {
            return 0; // strict mode needs the semicolon
        }
        consumed += 1;
        out.extend(from_code_point(replace_code_point(result)));
        return consumed;
    }

    let name_len = input[offset..]
        .iter()
        .take_while(|&&c| c < 0x80 && (c as u8).is_ascii_alphanumeric())
        .count();
    if name_len == 0 || input.get(offset + name_len) != Some(&u16::from(b';')) {
        return 0;
    }
    let name = js::to_string(&input[offset..offset + name_len]);
    match entities::lookup(&name) {
        Some(value) => {
            out.extend(value.encode_utf16());
            name_len + 2
        }
        None => 0,
    }
}

/// `decodeHTMLStrict(str)` of the `entities` package: decode the character references in
/// `str` that end with a semicolon (named ones from the HTML5 table, and numeric ones).
pub fn decode_html_strict(input: &[u16]) -> Vec<u16> {
    let mut ret = Vec::with_capacity(input.len());
    let mut last_index = 0;
    let mut offset = 0;
    while let Some(amp) = js::index_of(input, &[u16::from(b'&')], offset) {
        ret.extend_from_slice(&input[last_index..amp]);
        let length = decode_entity_strict(input, amp, &mut ret);
        last_index = amp + length;
        offset = if length == 0 {
            last_index + 1
        } else {
            last_index
        };
    }
    ret.extend_from_slice(&input[last_index..]);
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        js::to_utf16(s)
    }

    fn s(v: &[u16]) -> String {
        js::to_string(v)
    }

    #[test]
    fn array_replace_at_like_js() {
        // [1,2,3] at 1 with [7,8] => [1,7,8,3]; at 5 => [1,2,3,7,8]; at 2 => [1,2,7,8]
        assert_eq!(array_replace_at(&[1, 2, 3], 1, vec![7, 8]), [1, 7, 8, 3]);
        assert_eq!(array_replace_at(&[1, 2, 3], 5, vec![7, 8]), [1, 2, 3, 7, 8]);
        assert_eq!(array_replace_at(&[1, 2, 3], 2, vec![7, 8]), [1, 2, 7, 8]);
    }

    #[test]
    fn entity_codes_and_from_code_point() {
        for (c, ok) in [
            (0x41, true),
            (0xD800, false),
            (0xFDD0, false),
            (0x1FFFF, false),
            (0x10FFFE, false),
            (0x08, false),
            (0x09, true),
            (0x0B, false),
            (0x0C, true),
            (0x1F, false),
            (0x7F, false),
            (0x9F, false),
            (0xA0, true),
            (0x10FFFD, true),
            (0x110000, false),
        ] {
            assert_eq!(is_valid_entity_code(c), ok, "{c:#x}");
        }
        assert_eq!(from_code_point(0x1F600), u("\u{1F600}"));
        assert_eq!(from_code_point(0x41), u("A"));
        // never throws: 0x110000 gives '\uDC00\uDC00' in JS
        assert_eq!(from_code_point(0x110000), vec![0xDC00, 0xDC00]);
    }

    #[test]
    fn unescape_md_and_all_match_node() {
        // node: md.utils.unescapeMd / unescapeAll on each input
        assert_eq!(s(&unescape_md(&u(r"\*a\b\\"))), r"*a\b\");
        assert_eq!(s(&unescape_md(&u("plain"))), "plain");
        let cases = [
            (r"a\*b", "a*b"),
            ("&amp;&lt;", "&<"),
            ("&ngE;", "\u{2267}\u{338}"),
            ("&#x41;&#66;", "AB"),
            ("&#X41;", "A"),
            ("&#0;", "&#0;"),
            ("&#xD800;", "&#xD800;"),
            ("&#123456789;", "\u{FFFD}"),
            ("&#x110000;", "&#x110000;"),
            ("&#1a;", "&#1a;"),
            ("&amp", "&amp"),
            ("&ampx;", "&ampx;"),
            ("&AMP;", "&"),
            ("&Amp;", "&Amp;"),
            ("&#x;", "&#x;"),
            ("&#xg;", "&#xg;"),
            ("&#abc;", "&#abc;"),
            (r"\&amp;", "&amp;"),
            ("&#128;", "&#128;"),
            ("&#9;", "\t"),
            ("&#x1F600;", "\u{1F600}"),
            (
                "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
                "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(s(&unescape_all(&u(input))), want, "{input}");
        }
    }

    #[test]
    fn decode_html_strict_matches_entities() {
        // node: decodeHTMLStrict from the bundled entities
        let cases = [
            ("&amp;", "&"),
            ("&amp", "&amp"),
            ("a&lt;b&gt;c", "a<b>c"),
            ("&&amp;", "&&"),
            ("&#128;", "\u{20AC}"),
            ("&#0;", "\u{FFFD}"),
            ("&#xD800;", "\u{FFFD}"),
            ("&#99999999999999999999999;", "\u{FFFD}"),
            ("&#65", "&#65"),
            ("&#x;", "&#x;"),
            ("&NotNestedGreaterGreater;", "\u{2AA2}\u{338}"),
            ("&nosuch;", "&nosuch;"),
        ];
        for (input, want) in cases {
            assert_eq!(s(&decode_html_strict(&u(input))), want, "{input}");
        }
    }

    #[test]
    fn character_classes() {
        assert!(is_space(0x20u16) && is_space(0x09u32) && !is_space(0x0Au16) && !is_space(0xA0u32));
        for c in [
            0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0xA0, 0x1680, 0x2000, 0x200A, 0x202F, 0x205F,
            0x3000,
        ] {
            assert!(is_white_space(c as u32), "{c:#x}");
        }
        for c in [0x2028u32, 0x2029, 0xFEFF, 0x85, 0x180E, 0x200B] {
            assert!(!is_white_space(c), "{c:#x}");
        }
        let md: Vec<u32> = (0..0x80).filter(|&c| is_md_ascii_punct(c)).collect();
        let want: Vec<u32> = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"
            .chars()
            .map(u32::from)
            .collect();
        assert_eq!(md, want);
        // node: md.utils.isPunctCharCode(c) for each c
        for (c, want) in [
            (0x21, true),
            (0x24, true), // $ is S
            (0x41, false),
            (0xA7, true), // § is P
            (0x2019, true),
            (0x20AC, true), // € is S
            (0x1F600, true),
            (0x10100, true),
            (0xD800, false),
            (0x3000, false),
            (0x110000, false),
        ] {
            assert_eq!(is_punct_char_code(c), want, "{c:#x}");
        }
        // node: md.utils.isPunctChar('a!') === true (unanchored), isPunctChar('\uD83D') === false
        assert!(is_punct_char(&u("a!")));
        assert!(!is_punct_char(&[0xD83D]));
        assert!(!is_punct_char(&u("abc")));
    }

    #[test]
    fn normalize_reference_matches_node() {
        // node: md.utils.normalizeReference(x)
        let cases = [
            ("  Foo \t\n bar ", "FOO BAR"),
            ("ẞ", "SS"),
            (
                "\u{398}\u{3F4}\u{3B8}\u{3D1}",
                "\u{398}\u{398}\u{398}\u{398}",
            ),
            ("a\u{A0}\u{3000}b", "A B"),
            ("\u{130}", "I\u{307}"),
            ("\u{2126}", "\u{3A9}"),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_reference(&u(input)), want, "{input:?}");
        }
    }

    #[test]
    fn ascii_trim_keeps_unicode_spaces() {
        assert_eq!(
            ascii_trim(&u(" \t\n\r a b \u{A0}\r\n")),
            &u("a b \u{A0}")[..]
        );
        assert_eq!(ascii_trim(&u("  ")), &u("")[..]);
    }

    #[test]
    fn escapes() {
        assert_eq!(
            escape_html("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;'&lt;/a&gt;"
        );
        assert_eq!(
            escape_re("a.b*c(d)|[e]-{f}^$?+\\"),
            "a\\.b\\*c\\(d\\)\\|\\[e\\]\\-\\{f\\}\\^\\$\\?\\+\\\\"
        );
    }
}
