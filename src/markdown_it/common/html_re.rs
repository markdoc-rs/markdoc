//! Port of common/html_re.ts: regexps to match html elements.
//!
//! ```text
//! attr_name     = '[a-zA-Z_:][a-zA-Z0-9:._-]*'
//! unquoted      = '[^"\'=<>`\\x00-\\x20]+'
//! single_quoted = "'[^']*'"
//! double_quoted = '"[^"]*"'
//! attr_value    = `(?:${unquoted}|${single_quoted}|${double_quoted})`
//! attribute     = `(?:\\s+${attr_name}(?:\\s*=\\s*${attr_value})?)`
//! open_tag      = `<[A-Za-z][A-Za-z0-9\\-]*${attribute}*\\s*\\/?>`
//! close_tag     = '<\\/[A-Za-z][A-Za-z0-9\\-]*\\s*>'
//! comment       = '<!---?>|<!--(?:[^-]|-[^-]|--[^>])*-->'
//! processing    = '<[?][\\s\\S]*?[?]>'
//! declaration   = '<![A-Za-z][^>]*>'
//! cdata         = '<!\\[CDATA\\[[\\s\\S]*?\\]\\]>'
//!
//! HTML_TAG_RE            = new RegExp(`^(?:${open_tag}|${close_tag}|${comment}|${processing}|${declaration}|${cdata})`)
//! HTML_OPEN_CLOSE_TAG_RE = new RegExp(`^(?:${open_tag}|${close_tag})`)
//! ```
//!
//! The regexes become hand-written matchers over UTF-16 code units (the regexes have no `u`
//! flag, so JS matches code units too): `\s` is JS's whitespace set (`js::is_js_space`),
//! `[\s\S]` and the negated classes match any code unit (newlines and lone surrogates
//! included), `$` is the end of the input (no `m` flag). Each matcher returns the end of
//! the match JS finds, i.e. `match[0].length`, as argued per part below. The alternatives
//! of `HTML_TAG_RE` start differently (`<` then a letter, `/`, `!--`, `?`, `!` and a letter,
//! `![`), so at most one of them can match and their order cannot change the result.

use crate::markdown_it::js;

/// `[A-Za-z]`
fn is_alpha(c: u16) -> bool {
    matches!(c, 0x41..=0x5A | 0x61..=0x7A)
}

/// `[A-Za-z0-9\-]`
fn is_tag_name_char(c: u16) -> bool {
    matches!(c, 0x41..=0x5A | 0x61..=0x7A | 0x30..=0x39 | 0x2D)
}

/// `[a-zA-Z_:]`
fn is_attr_name_start(c: u16) -> bool {
    matches!(c, 0x41..=0x5A | 0x61..=0x7A | 0x5F | 0x3A)
}

/// `[a-zA-Z0-9:._-]`
fn is_attr_name_char(c: u16) -> bool {
    matches!(c, 0x41..=0x5A | 0x61..=0x7A | 0x30..=0x39 | 0x3A | 0x2E | 0x5F | 0x2D)
}

/// `[^"'=<>`\x00-\x20]`
fn is_unquoted_char(c: u16) -> bool {
    c > 0x20 && !matches!(c, 0x22 | 0x27 | 0x3D | 0x3C | 0x3E | 0x60)
}

/// `\s`
fn is_ws(c: u16) -> bool {
    js::is_js_space(u32::from(c))
}

/// `open_tag` at the start of `s`: the end of the match.
///
/// The pattern is ambiguous: JS's `\s` includes non-ASCII spaces (U+00A0, U+3000, ...)
/// that `unquoted` also accepts, so `<a b=c\u{A0}d>` matches with one attribute or with two,
/// and a backtracking engine explores such splits. Yet every successful match ends at the
/// same place: a match ends at a `>`, and a `>`, a quote or a `=` can only be consumed
/// inside a quoted value or as itself (the closing `>`, a value's quotes, the `=` before a
/// value). Take two matches and the first position consumed by both where they disagree
/// about being inside a quoted value: it cannot be inside a value both entered (same
/// opening quote, same closing quote; different openings would put a `=` outside one value
/// and inside the other, earlier), so it opens a value in one match only; the other cannot
/// consume a quote outside a value, so it ended before it, at a `>` that the first match
/// must then have consumed inside a value, earlier still. So the matches agree on quoted
/// values, hence on which `>` closes the tag. JS's first match therefore ends where any
/// match ends, which this finds by running the pattern as an NFA (a set of positions in
/// the pattern) and stopping at the first `>` that completes it.
fn open_tag(s: &[u16]) -> Option<usize> {
    // Places in the pattern, before the next code unit:
    const TAG_NAME: u16 = 1 << 0; // in `[A-Za-z0-9\-]*`
    const ATTR_START: u16 = 1 << 1; // before an attribute's `\s+`
    const ATTR_SPACE: u16 = 1 << 2; // in an attribute's `\s+` (one or more read)
    const ATTR_NAME: u16 = 1 << 3; // in `[a-zA-Z0-9:._-]*`
    const EQ_SPACE: u16 = 1 << 4; // in the `\s*` before `=`
    const VALUE_SPACE: u16 = 1 << 5; // in the `\s*` after `=`
    const UNQUOTED: u16 = 1 << 6; // in `unquoted` (one or more read)
    const SINGLE: u16 = 1 << 7; // in `'[^']*`
    const DOUBLE: u16 = 1 << 8; // in `"[^"]*`
    const END_SPACE: u16 = 1 << 9; // in the final `\s*`
    const SLASH: u16 = 1 << 10; // after the final `\/`
    // where `${attribute}*` can take another attribute or stop
    const LOOP: u16 = ATTR_START | END_SPACE;

    if s.first() != Some(&0x3C) || !s.get(1).is_some_and(|&c| is_alpha(c)) {
        return None;
    }
    let mut states = TAG_NAME | LOOP;
    let mut pos = 2;
    while states != 0 {
        let c = *s.get(pos)?;
        pos += 1;
        let mut next = 0;
        if states & TAG_NAME != 0 && is_tag_name_char(c) {
            next |= TAG_NAME | LOOP;
        }
        if states & (ATTR_START | ATTR_SPACE) != 0 && is_ws(c) {
            next |= ATTR_SPACE;
        }
        if states & ATTR_SPACE != 0 && is_attr_name_start(c) {
            // the value is optional: `=` may follow, or the next attribute, or the end
            next |= ATTR_NAME | EQ_SPACE | LOOP;
        }
        if states & ATTR_NAME != 0 && is_attr_name_char(c) {
            next |= ATTR_NAME | EQ_SPACE | LOOP;
        }
        if states & EQ_SPACE != 0 {
            if is_ws(c) {
                next |= EQ_SPACE;
            }
            if c == 0x3D {
                next |= VALUE_SPACE;
            }
        }
        if states & VALUE_SPACE != 0 {
            if is_ws(c) {
                next |= VALUE_SPACE;
            }
            if is_unquoted_char(c) {
                next |= UNQUOTED | LOOP;
            }
            if c == 0x27 {
                next |= SINGLE;
            }
            if c == 0x22 {
                next |= DOUBLE;
            }
        }
        if states & UNQUOTED != 0 && is_unquoted_char(c) {
            next |= UNQUOTED | LOOP;
        }
        if states & SINGLE != 0 {
            next |= if c == 0x27 { LOOP } else { SINGLE };
        }
        if states & DOUBLE != 0 {
            next |= if c == 0x22 { LOOP } else { DOUBLE };
        }
        if states & END_SPACE != 0 {
            if is_ws(c) {
                next |= END_SPACE;
            }
            if c == 0x2F {
                next |= SLASH;
            }
            if c == 0x3E {
                return Some(pos);
            }
        }
        if states & SLASH != 0 && c == 0x3E {
            return Some(pos);
        }
        states = next;
    }
    None
}

/// `close_tag` at the start of `s`: the end of the match. Deterministic: the name
/// characters, `\s` and `>` are disjoint classes, so each greedy run is all there is.
fn close_tag(s: &[u16]) -> Option<usize> {
    if s.first() != Some(&0x3C)
        || s.get(1) != Some(&0x2F)
        || !s.get(2).is_some_and(|&c| is_alpha(c))
    {
        return None;
    }
    let mut pos = 3;
    while s.get(pos).is_some_and(|&c| is_tag_name_char(c)) {
        pos += 1;
    }
    while s.get(pos).is_some_and(|&c| is_ws(c)) {
        pos += 1;
    }
    if s.get(pos) == Some(&0x3E) {
        Some(pos + 1)
    } else {
        None
    }
}

/// `comment` at the start of `s`: the end of the match.
///
/// `<!---?>` is tried first (`-?` with the `-` first). In the second alternative the three
/// loop alternatives exclude each other (`[^-]`, `-` then not `-`, `--` then not `>`), so
/// the greedy loop has one path; and `-->` can only match where the loop stops (at a `-->`
/// no loop alternative matches), so giving iterations back cannot help.
fn comment(s: &[u16]) -> Option<usize> {
    const OPEN: [u16; 4] = [0x3C, 0x21, 0x2D, 0x2D]; // <!--
    if !s.starts_with(&OPEN) {
        return None;
    }
    // <!---?>
    if s.get(4) == Some(&0x2D) && s.get(5) == Some(&0x3E) {
        return Some(6);
    }
    if s.get(4) == Some(&0x3E) {
        return Some(5);
    }
    // <!--(?:[^-]|-[^-]|--[^>])*-->
    let mut pos = 4;
    loop {
        match (s.get(pos), s.get(pos + 1), s.get(pos + 2)) {
            (Some(&c), _, _) if c != 0x2D => pos += 1,
            (Some(_), Some(&c1), _) if c1 != 0x2D => pos += 2,
            (Some(_), Some(_), Some(&c2)) if c2 != 0x3E => pos += 3,
            _ => break,
        }
    }
    if js::starts_with(s, &[0x2D, 0x2D, 0x3E], pos) {
        Some(pos + 3)
    } else {
        None
    }
}

/// `processing` at the start of `s`: `<?`, then the first `?>` (the lazy `[\s\S]*?` takes
/// the shortest match).
fn processing(s: &[u16]) -> Option<usize> {
    if !s.starts_with(&[0x3C, 0x3F]) {
        return None;
    }
    js::index_of(s, &[0x3F, 0x3E], 2).map(|i| i + 2)
}

/// `declaration` at the start of `s`: `<!`, a letter, then up to the first `>` (`[^>]*`
/// cannot give back anything that `>` would match).
fn declaration(s: &[u16]) -> Option<usize> {
    if !s.starts_with(&[0x3C, 0x21]) || !s.get(2).is_some_and(|&c| is_alpha(c)) {
        return None;
    }
    js::index_of(s, &[0x3E], 3).map(|i| i + 1)
}

/// `cdata` at the start of `s`: `<![CDATA[`, then the first `]]>` (lazy).
fn cdata(s: &[u16]) -> Option<usize> {
    const OPEN: [u16; 9] = [0x3C, 0x21, 0x5B, 0x43, 0x44, 0x41, 0x54, 0x41, 0x5B]; // <![CDATA[
    if !s.starts_with(&OPEN) {
        return None;
    }
    js::index_of(s, &[0x5D, 0x5D, 0x3E], 9).map(|i| i + 3)
}

/// `s.match(HTML_TAG_RE)` (anchored at the start): the length of `match[0]`, if any.
pub fn html_tag_re(s: &[u16]) -> Option<usize> {
    open_tag(s)
        .or_else(|| close_tag(s))
        .or_else(|| comment(s))
        .or_else(|| processing(s))
        .or_else(|| declaration(s))
        .or_else(|| cdata(s))
}

/// `s.match(HTML_OPEN_CLOSE_TAG_RE)` (anchored at the start): the length of `match[0]`.
pub fn html_open_close_tag_re(s: &[u16]) -> Option<usize> {
    open_tag(s).or_else(|| close_tag(s))
}

/// `new RegExp(HTML_OPEN_CLOSE_TAG_RE.source + '\\s*$').test(s)`, which html_block.ts
/// builds: an open or close tag at the start of `s` with only `\s` after it. Backtracking
/// into the tag cannot help `\s*$`, as every match of a tag ends at the same place (see
/// `open_tag`; `close_tag` is deterministic).
pub fn html_open_close_tag_re_to_end(s: &[u16]) -> bool {
    html_open_close_tag_re(s).is_some_and(|end| s[end..].iter().all(|&c| is_ws(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_regexes_match_node() {
        // node: [s.match(HTML_TAG_RE), s.match(HTML_OPEN_CLOSE_TAG_RE)] as match[0].length, and
        // HTML_SEQUENCES[6][0].test(s) (`HTML_OPEN_CLOSE_TAG_RE.source + '\\s*$'`), with the
        // regexes lifted from the bundle's source
        let cases: &[(&str, Option<usize>, Option<usize>, bool)] = &[
            ("<a>", Some(3), Some(3), true),
            ("<a", None, None, false),
            ("<a/>", Some(4), Some(4), true),
            ("<a />", Some(5), Some(5), true),
            ("<a//>", None, None, false),
            ("<A-1 b>", Some(7), Some(7), true),
            ("<1a>", None, None, false),
            ("<a b>", Some(5), Some(5), true),
            ("<a b=c>", Some(7), Some(7), true),
            ("<a b = c >", Some(10), Some(10), true),
            ("<a b='>'>", Some(9), Some(9), true),
            ("<a b=\"x'y\">", Some(11), Some(11), true),
            ("<a b=c/>", Some(8), Some(8), true),
            ("<a b=c\u{a0}d>", Some(9), Some(9), true),
            ("<a b=c\u{a0}d=e>", Some(11), Some(11), true),
            ("<a b=c\u{a0}d='>'>", Some(13), Some(13), true),
            ("<a b=\u{a0}>", Some(7), Some(7), true),
            ("<a b=>", None, None, false),
            ("<a b=c=d>", None, None, false),
            ("<a\u{3000}b>", Some(5), Some(5), true),
            ("<a\nb\n=\n\"c\"\n>", Some(12), Some(12), true),
            ("<a b=c`>", None, None, false),
            ("<a :x._-y=1>", Some(12), Some(12), true),
            ("<a b><c>", Some(5), Some(5), false),
            ("<a b='c' d>x", Some(11), Some(11), false),
            ("</a>", Some(4), Some(4), true),
            ("</a >", Some(5), Some(5), true),
            ("</a b>", None, None, false),
            ("</>", None, None, false),
            ("</1>", None, None, false),
            ("<!---->", Some(7), None, false),
            ("<!-->", Some(5), None, false),
            ("<!--->", Some(6), None, false),
            ("<!-- -->", Some(8), None, false),
            ("<!-- --->", None, None, false),
            ("<!-- a ---->", None, None, false),
            ("<!-- a -- b -->", Some(15), None, false),
            ("<!---x-->", Some(9), None, false),
            ("<!-->-->", Some(5), None, false),
            ("<?>", None, None, false),
            ("<??>", Some(4), None, false),
            ("<?x?>y?>", Some(5), None, false),
            ("<!A>", Some(4), None, false),
            ("<!a b>", Some(6), None, false),
            ("<!>", None, None, false),
            ("<![CDATA[]]>", Some(12), None, false),
            ("<![CDATA[x]]]>", Some(14), None, false),
            ("<![CDATA[", None, None, false),
            ("a<b>", None, None, false),
            ("<a b='' c=\"\">", Some(13), Some(13), true),
            ("<a b='c'd>", None, None, false),
            ("<a\u{2028}b=c>", Some(7), Some(7), true),
        ];
        for &(s, tag, open_close, to_end) in cases {
            let s16 = js::to_utf16(s);
            assert_eq!(
                (
                    html_tag_re(&s16),
                    html_open_close_tag_re(&s16),
                    html_open_close_tag_re_to_end(&s16)
                ),
                (tag, open_close, to_end),
                "{s:?}"
            );
        }
    }
}
