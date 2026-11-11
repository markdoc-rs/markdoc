//! JavaScript string semantics the port needs.
//!
//! JS strings are UTF-16 and markdown-it indexes them with `charCodeAt`, so the port keeps
//! text that rules index as `Vec<u16>` / `&[u16]`, and every position, mark and length is a
//! UTF-16 code-unit index, exactly as in JS. These helpers reproduce the parts of
//! `String.prototype` (and `Number`, `parseInt`) that markdown-it and its dependencies use,
//! with JS's semantics rather than Rust's. Each one was checked against node 22.22
//! (V8 12.4, ICU 78, Unicode 17); see the tests at the bottom.
//!
//! Where JS returns `NaN` (`charCodeAt` past the end) or `undefined`, these return `None`;
//! where JS returns `-1` (`indexOf`), they return `None` too.

/// The `RangeError` JS throws (`String.fromCodePoint`, punycode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeError(pub String);

impl std::fmt::Display for RangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RangeError: {}", self.0)
    }
}

impl std::error::Error for RangeError {}

/// A Rust string as the JS string with the same characters.
pub fn to_utf16(s: &str) -> Vec<u16> {
    // UTF-16 never takes more units than UTF-8 takes bytes, so one allocation holds it (and
    // ASCII, the common case, is exactly one unit per byte).
    let mut units = Vec::with_capacity(s.len());
    if s.is_ascii() {
        units.extend(s.bytes().map(u16::from));
    } else {
        units.extend(s.encode_utf16());
    }
    units
}

/// A JS string as a Rust string (lossless for well-formed UTF-16; a lone surrogate becomes U+FFFD).
pub fn to_string(s: &[u16]) -> String {
    String::from_utf16_lossy(s)
}

/// `s.charCodeAt(pos)`; `None` where JS returns `NaN`.
pub fn char_code_at(s: &[u16], pos: usize) -> Option<u16> {
    s.get(pos).copied()
}

/// `s.codePointAt(pos)`; `None` where JS returns `undefined`.
pub fn code_point_at(s: &[u16], pos: usize) -> Option<u32> {
    let first = *s.get(pos)?;
    if (0xD800..=0xDBFF).contains(&first)
        && let Some(&second) = s.get(pos + 1)
        && (0xDC00..=0xDFFF).contains(&second)
    {
        return Some(((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00) + 0x10000);
    }
    Some(u32::from(first))
}

/// `String.fromCharCode(...codes)`: each code is truncated to 16 bits (`ToUint16`).
pub fn from_char_code(codes: &[u32]) -> Vec<u16> {
    codes.iter().map(|&c| c as u16).collect()
}

/// `String.fromCodePoint(...codePoints)`, which throws `RangeError` past U+10FFFF.
/// Surrogate code points are allowed and become lone code units, as in JS.
pub fn from_code_point(code_points: &[u32]) -> Result<Vec<u16>, RangeError> {
    let mut out = Vec::with_capacity(code_points.len());
    for &cp in code_points {
        if cp > 0x10FFFF {
            return Err(RangeError(format!("Invalid code point {cp}")));
        }
        if cp > 0xFFFF {
            let c = cp - 0x10000;
            out.push(0xD800 + (c >> 10) as u16);
            out.push(0xDC00 + (c & 0x3FF) as u16);
        } else {
            out.push(cp as u16);
        }
    }
    Ok(out)
}

fn clamp_relative(index: i64, len: i64) -> i64 {
    if index < 0 {
        (len + index).max(0)
    } else {
        index.min(len)
    }
}

/// `s.slice(start, end)`: negative indices count from the end, both are clamped, and
/// `start >= end` gives the empty string. `end: None` is `undefined` (the end of `s`).
pub fn slice(s: &[u16], start: i64, end: Option<i64>) -> &[u16] {
    let len = s.len() as i64;
    let from = clamp_relative(start, len);
    let to = end.map_or(len, |e| clamp_relative(e, len));
    if from >= to {
        &s[..0]
    } else {
        &s[from as usize..to as usize]
    }
}

/// `s.slice(start, end)` for non-negative indices (the common case): both are clamped to the
/// length and `start >= end` gives the empty string, so it never panics.
pub fn slice_to(s: &[u16], start: usize, end: usize) -> &[u16] {
    let to = end.min(s.len());
    if start >= to { &s[..0] } else { &s[start..to] }
}

/// `s.substring(start, end)`: negative indices become 0, both are clamped, and they are
/// swapped when `start > end`.
pub fn substring(s: &[u16], start: i64, end: Option<i64>) -> &[u16] {
    let len = s.len() as i64;
    let a = start.clamp(0, len);
    let b = end.map_or(len, |e| e.clamp(0, len));
    let (from, to) = if a <= b { (a, b) } else { (b, a) };
    &s[from as usize..to as usize]
}

/// `s.substr(start, length)` (Annex B): a negative `start` counts from the end.
pub fn substr(s: &[u16], start: i64, length: Option<i64>) -> &[u16] {
    let len = s.len() as i64;
    let from = clamp_relative(start, len);
    let count = length.map_or(len - from, |l| l.clamp(0, len - from));
    if count <= 0 {
        &s[..0]
    } else {
        &s[from as usize..(from + count) as usize]
    }
}

/// `s.indexOf(search, from)`; `None` where JS returns `-1`. An empty `search` is found at
/// `min(from, s.length)`, as in JS.
pub fn index_of(s: &[u16], search: &[u16], from: usize) -> Option<usize> {
    let len = s.len();
    let start = from.min(len);
    if search.is_empty() {
        return Some(start);
    }
    if search.len() > len {
        return None;
    }
    (start..=len - search.len()).find(|&k| &s[k..k + search.len()] == search)
}

/// `s.lastIndexOf(search, from)`; `from: None` is `undefined` (+Infinity). `None` where JS
/// returns `-1`.
pub fn last_index_of(s: &[u16], search: &[u16], from: Option<usize>) -> Option<usize> {
    let len = s.len();
    if search.len() > len {
        return None;
    }
    let start = from.unwrap_or(usize::MAX).min(len).min(len - search.len());
    (0..=start)
        .rev()
        .find(|&k| &s[k..k + search.len()] == search)
}

/// `s.startsWith(search, position)`.
pub fn starts_with(s: &[u16], search: &[u16], position: usize) -> bool {
    s[position.min(s.len())..].starts_with(search)
}

/// `s.endsWith(search, endPosition)`; `end_position: None` is `undefined` (the length).
pub fn ends_with(s: &[u16], search: &[u16], end_position: Option<usize>) -> bool {
    let end = end_position.unwrap_or(s.len()).min(s.len());
    end >= search.len() && &s[end - search.len()..end] == search
}

/// `s.includes(search, position)`.
pub fn includes(s: &[u16], search: &[u16], position: usize) -> bool {
    index_of(s, search, position).is_some()
}

/// `s.split(separator)` with a string separator and no limit.
pub fn split<'s>(s: &'s [u16], separator: &[u16]) -> Vec<&'s [u16]> {
    if separator.is_empty() {
        return s.chunks(1).collect();
    }
    let mut out = Vec::new();
    let mut last = 0;
    let mut pos = 0;
    while let Some(k) = index_of(s, separator, pos) {
        if k + separator.len() > s.len() {
            break;
        }
        out.push(&s[last..k]);
        last = k + separator.len();
        pos = last;
        if pos > s.len() {
            break;
        }
    }
    out.push(&s[last..]);
    out
}

/// JS's WhiteSpace and LineTerminator code points: what `trim` removes and what the regex
/// class `\s` matches (all of them are in the BMP, so a code unit is enough).
pub fn is_js_space(c: u32) -> bool {
    matches!(
        c,
        0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x20 | 0xA0 | 0x1680 | 0x2000
            ..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF
    )
}

/// `s.trim()` with JS's whitespace set (not Rust's).
pub fn trim(s: &[u16]) -> &[u16] {
    trim_end(trim_start(s))
}

/// `s.trimStart()`.
pub fn trim_start(s: &[u16]) -> &[u16] {
    let start = s
        .iter()
        .position(|&c| !is_js_space(u32::from(c)))
        .unwrap_or(s.len());
    &s[start..]
}

/// `s.trimEnd()`.
pub fn trim_end(s: &[u16]) -> &[u16] {
    let end = s
        .iter()
        .rposition(|&c| !is_js_space(u32::from(c)))
        .map_or(0, |i| i + 1);
    &s[..end]
}

/// Applies a Rust case conversion to each well-formed run of `s`, leaving lone surrogates
/// as they are (JS does too). A lone surrogate is neither cased nor case-ignorable, so it
/// ends the Final_Sigma context exactly like the start or end of a run does.
fn map_case(s: &[u16], f: fn(&str) -> String) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut run_start = 0;
    let mut i = 0;
    let flush = |out: &mut Vec<u16>, run: &[u16]| {
        if !run.is_empty() {
            let text = String::from_utf16(run).expect("a run without lone surrogates");
            out.extend(f(&text).encode_utf16());
        }
    };
    while i < s.len() {
        let c = s[i];
        if (0xD800..=0xDBFF).contains(&c)
            && i + 1 < s.len()
            && (0xDC00..=0xDFFF).contains(&s[i + 1])
        {
            i += 2;
            continue;
        }
        if (0xD800..=0xDFFF).contains(&c) {
            flush(&mut out, &s[run_start..i]);
            out.push(c);
            run_start = i + 1;
        }
        i += 1;
    }
    flush(&mut out, &s[run_start..]);
    out
}

/// `s.toLowerCase()`: full Unicode mapping (U+0130 becomes two code units) with the
/// Final_Sigma rule, as V8/ICU do in the root locale. Rust's std implements the same
/// mapping; its Unicode version (17) matches node 22's ICU, and every code point plus the
/// sigma contexts were compared against node (see tests).
pub fn to_lower_case(s: &[u16]) -> Vec<u16> {
    map_case(s, str::to_lowercase)
}

/// `s.toUpperCase()`: full Unicode mapping (`ß` becomes `SS`, U+1E9E stays), root locale.
pub fn to_upper_case(s: &[u16]) -> Vec<u16> {
    map_case(s, str::to_uppercase)
}

/// `String(x)` for a number (ECMAScript Number::toString with radix 10): the shortest
/// round-tripping digits, in exponent form below 1e-6 and from 1e21 up.
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    if x.is_infinite() {
        return "Infinity".into();
    }
    // Rust's `{:e}` prints the shortest round-tripping digits, as V8 does.
    let formatted = format!("{x:e}");
    let (mantissa, exponent) = formatted.split_once('e').expect("`{:e}` has an exponent");
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let k = digits.len() as i64;
    let n = exponent.parse::<i64>().expect("integer exponent") + 1;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    }
}

fn digit_value(c: u16) -> Option<u32> {
    char::from_u32(u32::from(c)).and_then(|ch| ch.to_digit(36))
}

/// `parseInt(s, radix)` for `radix` in 2..=36 (or 0, meaning 10 or a `0x` prefix): leading
/// JS whitespace and one sign are skipped, digits are read until the first non-digit, and
/// `NaN` comes back when there are none. Exact up to 2^53 (markdown-it never reads more
/// than eight digits).
pub fn parse_int(s: &[u16], radix: u32) -> f64 {
    let mut s = trim_start(s);
    let mut sign = 1.0;
    if let Some(&c) = s.first()
        && (c == u16::from(b'-') || c == u16::from(b'+'))
    {
        if c == u16::from(b'-') {
            sign = -1.0;
        }
        s = &s[1..];
    }
    let mut r = radix;
    let mut strip_prefix = true;
    if r != 0 {
        if !(2..=36).contains(&r) {
            return f64::NAN;
        }
        if r != 16 {
            strip_prefix = false;
        }
    } else {
        r = 10;
    }
    if strip_prefix && s.len() >= 2 && s[0] == u16::from(b'0') && (s[1] | 0x20) == u16::from(b'x') {
        s = &s[2..];
        r = 16;
    }
    let end = s
        .iter()
        .position(|&c| digit_value(c).is_none_or(|d| d >= r))
        .unwrap_or(s.len());
    let z = &s[..end];
    if z.is_empty() {
        return f64::NAN;
    }
    let value = if r == 10 {
        // Correctly rounded, as V8 does for radix 10.
        to_string(z).parse::<f64>().expect("decimal digits")
    } else {
        z.iter().fold(0.0, |acc, &c| {
            acc * f64::from(r) + f64::from(digit_value(c).expect("digit"))
        })
    };
    sign * value
}

/// `Number(s)` for a string (ECMAScript StringToNumber): JS whitespace is trimmed, the empty
/// string is 0, `0x`/`0o`/`0b` prefixes and `Infinity` are understood, anything else that
/// is not a whole decimal literal is `NaN`.
pub fn string_to_number(s: &[u16]) -> f64 {
    let s = trim(s);
    if s.is_empty() {
        return 0.0;
    }
    let text = to_string(s);
    let b = text.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let digits = &text[2..];
            if digits.chars().all(|c| c.is_digit(radix)) {
                return digits.chars().fold(0.0, |acc, c| {
                    acc * f64::from(radix) + f64::from(c.to_digit(radix).expect("digit"))
                });
            }
            return f64::NAN;
        }
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(&text);
    let negative = text.starts_with('-');
    if unsigned == "Infinity" {
        return if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // StrUnsignedDecimalLiteral: digits [. digits] [exponent] | . digits [exponent]
    let bytes = unsigned.as_bytes();
    let mut i = 0;
    let int_digits = bytes.iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        frac_digits = bytes[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return f64::NAN;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_digits = bytes[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits == 0 {
            return f64::NAN;
        }
        i += exp_digits;
    }
    if i != bytes.len() {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        to_utf16(s)
    }

    fn units(s: &[u16]) -> Vec<u16> {
        s.to_vec()
    }

    #[test]
    fn char_and_code_point_access() {
        // node: 'a😀'.charCodeAt(1) === 0xD83D, .charCodeAt(9) is NaN,
        // .codePointAt(1) === 0x1F600, .codePointAt(2) === 0xDE00, .codePointAt(3) undefined
        let s = u("a\u{1F600}");
        assert_eq!(char_code_at(&s, 1), Some(0xD83D));
        assert_eq!(char_code_at(&s, 9), None);
        assert_eq!(code_point_at(&s, 1), Some(0x1F600));
        assert_eq!(code_point_at(&s, 2), Some(0xDE00));
        assert_eq!(code_point_at(&s, 3), None);
        // a lone high surrogate at the end: '\uD83D'.codePointAt(0) === 0xD83D
        assert_eq!(code_point_at(&[0xD83D], 0), Some(0xD83D));
    }

    #[test]
    fn from_char_code_and_code_point() {
        // String.fromCharCode(0x10041) === 'A' (ToUint16), fromCharCode(0xD800) is a lone unit
        assert_eq!(from_char_code(&[0x10041, 0xD800]), vec![0x41, 0xD800]);
        // String.fromCodePoint(0x1F600, 0xDC00) === '😀\uDC00'
        assert_eq!(
            from_code_point(&[0x1F600, 0xDC00]),
            Ok(vec![0xD83D, 0xDE00, 0xDC00])
        );
        // String.fromCodePoint(0x110000) throws RangeError
        assert!(from_code_point(&[0x110000]).is_err());
    }

    #[test]
    fn slice_substring_substr() {
        let s = u("abcdef");
        // 'abcdef'.slice(-2) === 'ef', .slice(1, -1) === 'bcde', .slice(4, 2) === '', .slice(2, 99) === 'cdef'
        assert_eq!(slice(&s, -2, None), &u("ef")[..]);
        assert_eq!(slice(&s, 1, Some(-1)), &u("bcde")[..]);
        assert_eq!(slice(&s, 4, Some(2)), &u("")[..]);
        assert_eq!(slice(&s, 2, Some(99)), &u("cdef")[..]);
        assert_eq!(slice(&s, -99, Some(2)), &u("ab")[..]);
        assert_eq!(slice_to(&s, 4, 2), &u("")[..]);
        assert_eq!(slice_to(&s, 3, 99), &u("def")[..]);
        // 'abcdef'.substring(4, 1) === 'bcd', .substring(-3, 2) === 'ab'
        assert_eq!(substring(&s, 4, Some(1)), &u("bcd")[..]);
        assert_eq!(substring(&s, -3, Some(2)), &u("ab")[..]);
        // 'abcdef'.substr(-3, 2) === 'de', .substr(2) === 'cdef', .substr(1, -1) === ''
        assert_eq!(substr(&s, -3, Some(2)), &u("de")[..]);
        assert_eq!(substr(&s, 2, None), &u("cdef")[..]);
        assert_eq!(substr(&s, 1, Some(-1)), &u("")[..]);
    }

    #[test]
    fn index_of_and_last_index_of() {
        let s = u("abcabc");
        // 'abcabc'.indexOf('c', 3) === 5, .indexOf('', 99) === 6, .indexOf('x') === -1
        assert_eq!(index_of(&s, &u("c"), 3), Some(5));
        assert_eq!(index_of(&s, &u(""), 99), Some(6));
        assert_eq!(index_of(&s, &u("x"), 0), None);
        assert_eq!(index_of(&s, &u("abcabcd"), 0), None);
        // 'abcabc'.lastIndexOf('a') === 3, .lastIndexOf('a', 2) === 0, .lastIndexOf('bc', 99) === 4
        assert_eq!(last_index_of(&s, &u("a"), None), Some(3));
        assert_eq!(last_index_of(&s, &u("a"), Some(2)), Some(0));
        assert_eq!(last_index_of(&s, &u("bc"), Some(99)), Some(4));
        assert_eq!(last_index_of(&s, &u(""), None), Some(6));
        // 'abc'.startsWith('bc', 1), !'abc'.startsWith('a', 9), 'abc'.endsWith('ab', 2)
        assert!(starts_with(&u("abc"), &u("bc"), 1));
        assert!(!starts_with(&u("abc"), &u("a"), 9));
        assert!(starts_with(&u("abc"), &u(""), 9));
        assert!(ends_with(&u("abc"), &u("ab"), Some(2)));
        assert!(!ends_with(&u("abc"), &u("abcd"), None));
        assert!(includes(&u("abc"), &u("c"), 2));
    }

    #[test]
    fn split_like_js() {
        let parts = |s: &str, sep: &str| -> Vec<String> {
            split(&u(s), &u(sep)).into_iter().map(to_string).collect()
        };
        // 'a\nb\n'.split('\n') => ['a','b',''], ''.split('\n') => [''], 'ab'.split('') => ['a','b'],
        // ''.split('') => [], 'aXXb'.split('XX') => ['a','b'], 'XX'.split('XX') => ['','']
        assert_eq!(parts("a\nb\n", "\n"), ["a", "b", ""]);
        assert_eq!(parts("", "\n"), [""]);
        assert_eq!(parts("ab", ""), ["a", "b"]);
        assert!(parts("", "").is_empty());
        assert_eq!(parts("aXXb", "XX"), ["a", "b"]);
        assert_eq!(parts("XX", "XX"), ["", ""]);
    }

    #[test]
    fn whitespace_set_matches_node() {
        // node: every u in 0..=0xFFFF with String.fromCharCode(u).trim() === '' (and /\s/):
        // 9 a b c d 20 a0 1680 2000-200a 2028 2029 202f 205f 3000 feff
        let expected: Vec<u32> = [0x9, 0xA, 0xB, 0xC, 0xD, 0x20, 0xA0, 0x1680]
            .into_iter()
            .chain(0x2000..=0x200A)
            .chain([0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF])
            .collect();
        let got: Vec<u32> = (0..=0xFFFF).filter(|&c| is_js_space(c)).collect();
        assert_eq!(got, expected);
        // U+180E is not whitespace any more; U+0085 never was
        assert!(!is_js_space(0x180E) && !is_js_space(0x85));
        assert_eq!(trim(&u("\u{FEFF} a b\u{3000}\n")), &u("a b")[..]);
        assert_eq!(trim_start(&u("  a ")), &u("a ")[..]);
        assert_eq!(trim_end(&u("  a ")), &u("  a")[..]);
        assert_eq!(trim(&u(" \t ")), &u("")[..]);
    }

    #[test]
    fn case_mapping_matches_node() {
        // node: 'ẞ'.toLowerCase() === 'ß', 'ß'.toUpperCase() === 'SS',
        // 'İ'.toLowerCase() === 'i̇', 'ΑΣ'.toLowerCase() === 'ας', 'ΑΣ Α' => 'ας α',
        // 'Σ'.toLowerCase() === 'σ', 'ﬀ'.toUpperCase() === 'FF', 'ŉ'.toUpperCase() === 'ʼN'
        assert_eq!(to_lower_case(&u("\u{1E9E}")), u("\u{DF}"));
        assert_eq!(to_upper_case(&u("\u{DF}")), u("SS"));
        assert_eq!(to_lower_case(&u("\u{130}")), u("i\u{307}"));
        assert_eq!(to_lower_case(&u("\u{391}\u{3A3}")), u("\u{3B1}\u{3C2}"));
        assert_eq!(
            to_lower_case(&u("\u{391}\u{3A3} \u{391}")),
            u("\u{3B1}\u{3C2} \u{3B1}")
        );
        assert_eq!(to_lower_case(&u("\u{3A3}")), u("\u{3C3}"));
        assert_eq!(to_upper_case(&u("\u{FB00}")), u("FF"));
        assert_eq!(to_upper_case(&u("\u{149}")), u("\u{2BC}N"));
        // node: 'ÁΣ'.toLowerCase() ends in 'ς' (U+0301 is case-ignorable),
        // 'AΣ́b'.toLowerCase() has 'σ' (a cased letter follows the ignorable)
        assert_eq!(to_lower_case(&u("A\u{301}\u{3A3}")), u("a\u{301}\u{3C2}"));
        assert_eq!(to_lower_case(&u("A\u{3A3}\u{301}b")), u("a\u{3C3}\u{301}b"));
        // lone surrogates pass through and break the sigma context:
        // 'A\uD800Σ'.toLowerCase() === 'a\uD800σ', 'AΣ\uDC00'.toLowerCase() === 'aς\uDC00'
        assert_eq!(
            to_lower_case(&[0x41, 0xD800, 0x3A3]),
            vec![0x61, 0xD800, 0x3C3]
        );
        assert_eq!(
            to_lower_case(&[0x41, 0x3A3, 0xDC00]),
            vec![0x61, 0x3C2, 0xDC00]
        );
        assert_eq!(
            to_upper_case(&[0x61, 0xDBFF, 0x62]),
            vec![0x41, 0xDBFF, 0x42]
        );
        // astral: '\u{10428}'.toUpperCase() === '\u{10400}' (Deseret)
        assert_eq!(to_upper_case(&u("\u{10428}")), u("\u{10400}"));
        assert_eq!(units(&to_lower_case(&u("ABC"))), u("abc"));
    }

    /// FNV-1a over the UTF-16 units of each string (and its length).
    fn fnv(h: u32, s: &[u16]) -> u32 {
        let mut h = h;
        for &c in s {
            h ^= u32::from(c);
            h = h.wrapping_mul(0x0100_0193);
        }
        h ^= s.len() as u32;
        h.wrapping_mul(0x0100_0193)
    }

    #[test]
    fn case_mapping_of_every_code_point_matches_node() {
        // node (tests/markdown_it/unit_test_values.cjs prints these): FNV-1a of
        // s.toLowerCase(), s.toUpperCase() for every code point s (surrogates as lone units),
        // and of the sigma contexts ('A'+s+'Σ', 'AΣ'+s+'A', s+'Σ').toLowerCase()
        let (mut h1, mut h2) = (0x811c_9dc5_u32, 0x811c_9dc5_u32);
        for c in 0u32..=0x10FFFF {
            let s: Vec<u16> = match char::from_u32(c) {
                Some(ch) => ch.to_string().encode_utf16().collect(),
                None => vec![c as u16],
            };
            h1 = fnv(h1, &to_lower_case(&s));
            h1 = fnv(h1, &to_upper_case(&s));
            let ctx = |pre: &[u16], post: &[u16]| [pre, &s, post].concat();
            h2 = fnv(h2, &to_lower_case(&ctx(&[0x41], &[0x3A3])));
            h2 = fnv(h2, &to_lower_case(&ctx(&[0x41, 0x3A3], &[0x41])));
            h2 = fnv(h2, &to_lower_case(&ctx(&[], &[0x3A3])));
        }
        assert_eq!((h1, h2), (0x693e_37c0, 0x5e29_9768));
    }

    #[test]
    fn number_to_string_matches_node() {
        // node: String(x) for each x
        let cases: [(f64, &str); 14] = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (123456789.0, "123456789"),
            (0.1, "0.1"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (1e20, "100000000000000000000"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.25e-7, "1.25e-7"),
            (f64::INFINITY, "Infinity"),
            (f64::NAN, "NaN"),
        ];
        for (x, want) in cases {
            assert_eq!(number_to_string(x), want, "{x}");
        }
        assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(number_to_string(2.0_f64.powi(53)), "9007199254740992");
    }

    #[test]
    fn parse_int_matches_node() {
        // node: parseInt(s, r)
        assert_eq!(parse_int(&u("ff"), 16), 255.0);
        assert_eq!(parse_int(&u("0x1F"), 16), 31.0);
        assert_eq!(parse_int(&u("0x1F"), 10), 0.0);
        assert_eq!(parse_int(&u("  -12abc"), 10), -12.0);
        assert_eq!(parse_int(&u("FFFFFFFF"), 16), 4294967295.0);
        assert_eq!(parse_int(&u("99999999"), 10), 99999999.0);
        assert!(parse_int(&u("g"), 16).is_nan());
        assert!(parse_int(&u(""), 10).is_nan());
        assert!(parse_int(&u("1"), 37).is_nan());
        assert_eq!(parse_int(&u("z"), 36), 35.0);
    }

    #[test]
    fn string_to_number_matches_node() {
        // node: Number(s)
        let cases: [(&str, f64); 12] = [
            ("", 0.0),
            ("  ", 0.0),
            ("007", 7.0),
            ("123456789", 123456789.0),
            (" 1.5 ", 1.5),
            ("5.", 5.0),
            (".5", 0.5),
            ("-2e3", -2000.0),
            ("0x10", 16.0),
            ("0b101", 5.0),
            ("0o17", 15.0),
            ("Infinity", f64::INFINITY),
        ];
        for (s, want) in cases {
            assert_eq!(string_to_number(&u(s)), want, "{s:?}");
        }
        for s in [
            ".", "e5", "1e", "0x", "-0x10", "1_0", "infinity", "1 2", "+-1",
        ] {
            assert!(string_to_number(&u(s)).is_nan(), "{s:?}");
        }
    }
}
