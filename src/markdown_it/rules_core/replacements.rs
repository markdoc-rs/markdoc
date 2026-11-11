//! Port of rules_core/replacements.ts.
//!
//! The regexes are matched by hand over UTF-16 (each matcher quotes its regex). They are
//! ASCII patterns; the `i` flag (without `u`) folds only ASCII letters, `\s` is JS's
//! whitespace class, and with the `m` flag `^`/`$` also match after/before a JS line
//! terminator (\n \r U+2028 U+2029).

// Simple typographic replacements
//
// (c) (C) → ©
// (tm) (TM) → ™
// (r) (R) → ®
// +- → ±
// ... → … (also ?.... → ?.., !.... → !..)
// ???????? → ???, !!!!! → !!!, `,,` → `,`
// -- → &ndash;, --- → &mdash;
//

// TODO:
// - fractionals 1/2, 1/4, 3/4 -> ½, ¼, ¾
// - multiplications 2 x 4 -> 2 × 4

use super::state_core::StateCore;
use crate::markdown_it::JsError;
use crate::markdown_it::js;
use crate::markdown_it::token::Token;

fn is(c: u16, ascii: u8) -> bool {
    c == u16::from(ascii)
}

/// `RARE_RE.test(s)`: `/\+-|\.\.|\?\?\?\?|!!!!|,,|--/`.
fn rare_re_test(s: &[u16]) -> bool {
    ["+-", "..", "????", "!!!!", ",,", "--"]
        .iter()
        .any(|needle| js::includes(s, &js::to_utf16(needle), 0))
}

// Workaround for phantomjs - need regex without /g flag,
// or root check will fail every second time
/// `SCOPED_ABBR_TEST_RE.test(s)`: `/\((c|tm|r)\)/i`.
fn scoped_abbr_test_re(s: &[u16]) -> bool {
    (0..s.len()).any(|i| scoped_abbr_re_at(s, i).is_some())
}

/// A match of `SCOPED_ABBR_RE` (`/\((c|tm|r)\)/ig`) starting at `i`: its length and the
/// captured name.
fn scoped_abbr_re_at(s: &[u16], i: usize) -> Option<(usize, &[u16])> {
    let lower = |k: usize| {
        s.get(k)
            .map(|&c| c | if (0x41..=0x5A).contains(&c) { 0x20 } else { 0 })
    };
    if !s.get(i).is_some_and(|&c| is(c, b'(')) {
        return None;
    }
    let close = |k: usize| s.get(k).is_some_and(|&c| is(c, b')'));
    if lower(i + 1) == Some(u16::from(b'c')) && close(i + 2) {
        return Some((3, &s[i + 1..i + 2]));
    }
    if lower(i + 1) == Some(u16::from(b't'))
        && lower(i + 2) == Some(u16::from(b'm'))
        && close(i + 3)
    {
        return Some((4, &s[i + 1..i + 3]));
    }
    if lower(i + 1) == Some(u16::from(b'r')) && close(i + 2) {
        return Some((3, &s[i + 1..i + 2]));
    }
    None
}

/// `SCOPED_ABBR[name]`.
fn scoped_abbr(name: &[u16]) -> &'static str {
    match js::to_string(name).as_str() {
        "c" => "©",
        "r" => "®",
        "tm" => "™",
        _ => unreachable!("SCOPED_ABBR_RE only captures c, tm and r"),
    }
}

/// `replaceFn(match, name)`.
fn replace_fn(name: &[u16]) -> Vec<u16> {
    js::to_utf16(scoped_abbr(&js::to_lower_case(name)))
}

/// `s.replace(SCOPED_ABBR_RE, replaceFn)`.
fn replace_scoped_abbr(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some((len, name)) = scoped_abbr_re_at(s, i) {
            out.extend(replace_fn(name));
            i += len;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

fn replace_scoped(inline_tokens: &mut [Token]) {
    let mut inside_autolink: i32 = 0;

    for i in (0..inline_tokens.len()).rev() {
        let token = &mut inline_tokens[i];

        if token.kind == "text" && inside_autolink == 0 {
            let content = token.content_utf16();
            token.set_content_utf16(&replace_scoped_abbr(&content));
        }

        if token.kind == "link_open" && token.info == "auto" {
            inside_autolink -= 1;
        }

        if token.kind == "link_close" && token.info == "auto" {
            inside_autolink += 1;
        }
    }
}

/// `s.replace(/<c>{min,}/g, rep)` for a single character `c`: each run of at least `min`
/// becomes `rep`.
fn replace_runs(s: &[u16], c: u8, min: usize, rep: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let run = s[i..].iter().take_while(|&&x| is(x, c)).count();
        if run >= min {
            out.extend_from_slice(rep);
            i += run;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// `s.replace(/\+-/g, '±')`.
fn replace_plus_minus(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if is(s[i], b'+') && s.get(i + 1).is_some_and(|&c| is(c, b'-')) {
            out.push(0xB1);
            i += 2;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

fn is_q_or_bang(c: u16) -> bool {
    is(c, b'?') || is(c, b'!')
}

/// `s.replace(/([?!])…/g, '$1..')`.
fn replace_ellipsis_after_q_or_bang(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len() + 2);
    let mut i = 0;
    while i < s.len() {
        if is_q_or_bang(s[i]) && s.get(i + 1) == Some(&0x2026) {
            out.extend([s[i], u16::from(b'.'), u16::from(b'.')]);
            i += 2;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// `s.replace(/([?!]){4,}/g, '$1$1$1')`: a run of four or more `?`/`!` (greedy, so the
/// whole run) becomes three of its last character, the group's last capture.
fn replace_q_or_bang_runs(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let run = s[i..].iter().take_while(|&&c| is_q_or_bang(c)).count();
        if run >= 4 {
            let last = s[i + run - 1];
            out.extend([last, last, last]);
            i += run;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// JS's LineTerminator code points, where multiline `^` and `$` match.
fn is_line_terminator(c: u16) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

fn is_s(c: u16) -> bool {
    js::is_js_space(u32::from(c))
}

/// `s.replace(/(^|[before])-{n}(?=[after]|$)/mg, '$1' + dash)`, where `before` and `after`
/// are the classes as predicates. At each position the regex tries `^` first, then the
/// class (which consumes a character, given back as `$1`); after a match the scan goes on
/// where it ended.
fn replace_dashes(
    s: &[u16],
    n: usize,
    before: fn(u16) -> bool,
    after: fn(u16) -> bool,
    dash: u16,
) -> Vec<u16> {
    let len = s.len();
    let line_start = |p: usize| p == 0 || is_line_terminator(s[p - 1]);
    let dashes_at = |p: usize| p + n <= len && s[p..p + n].iter().all(|&c| is(c, b'-'));
    let lookahead = |p: usize| p == len || is_line_terminator(s[p]) || after(s[p]);

    let mut out = Vec::with_capacity(len);
    let mut p = 0;
    while p < len {
        if line_start(p) && dashes_at(p) && lookahead(p + n) {
            out.push(dash);
            p += n;
        } else if before(s[p]) && dashes_at(p + 1) && lookahead(p + 1 + n) {
            out.push(s[p]);
            out.push(dash);
            p += 1 + n;
        } else {
            out.push(s[p]);
            p += 1;
        }
    }
    out
}

/// The `.replace` chain of `replace_rare`, in order.
fn replace_rare_text(s: &[u16]) -> Vec<u16> {
    let s = replace_plus_minus(s);
    // .., ..., ....... -> …
    // but ?..... & !..... -> ?.. & !..
    let s = replace_runs(&s, b'.', 2, &[0x2026]);
    let s = replace_ellipsis_after_q_or_bang(&s);
    let s = replace_q_or_bang_runs(&s);
    let s = replace_runs(&s, b',', 2, &[u16::from(b',')]);
    // em-dash
    // .replace(/(^|[^-])---(?=[^-]|$)/mg, '$1—')
    let s = replace_dashes(&s, 3, |c| !is(c, b'-'), |c| !is(c, b'-'), 0x2014);
    // en-dash
    // .replace(/(^|\s)--(?=\s|$)/mg, '$1–')
    let s = replace_dashes(&s, 2, is_s, is_s, 0x2013);
    // .replace(/(^|[^-\s])--(?=[^-\s]|$)/mg, '$1–')
    let not_dash_or_s = |c: u16| !is(c, b'-') && !is_s(c);
    replace_dashes(&s, 2, not_dash_or_s, not_dash_or_s, 0x2013)
}

fn replace_rare(inline_tokens: &mut [Token]) {
    let mut inside_autolink: i32 = 0;

    for i in (0..inline_tokens.len()).rev() {
        let token = &mut inline_tokens[i];

        if token.kind == "text" && inside_autolink == 0 {
            let content = token.content_utf16();
            if rare_re_test(&content) {
                token.set_content_utf16(&replace_rare_text(&content));
            }
        }

        if token.kind == "link_open" && token.info == "auto" {
            inside_autolink -= 1;
        }

        if token.kind == "link_close" && token.info == "auto" {
            inside_autolink += 1;
        }
    }
}

pub fn replace(state: &mut StateCore) {
    if !state.md.options.typographer {
        return;
    }

    for blk_idx in (0..state.tokens.len()).rev() {
        if state.tokens[blk_idx].kind != "inline" {
            continue;
        }

        let content = js::to_utf16(&state.tokens[blk_idx].content);

        for (test, rule) in [
            (
                scoped_abbr_test_re as fn(&[u16]) -> bool,
                replace_scoped as fn(&mut [Token]),
            ),
            (rare_re_test, replace_rare),
        ] {
            if !test(&content) {
                continue;
            }
            // `state.tokens[blkIdx].children`: iterating null (a plugin's inline token) throws
            let Some(children) = state.tokens[blk_idx].children.as_mut() else {
                state.env.throw(JsError::type_error(
                    "Cannot read properties of null (reading 'length')",
                ));
                return;
            };
            rule(children);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    fn tok(kind: &str, content: &str, info: &str) -> Token {
        let mut t = Token::new(kind, "", 0);
        t.content = content.into();
        t.info = info.into();
        t
    }

    /// The rule on one inline token with these children, as core_values.cjs runs it.
    fn run(content: &str, children: Vec<Token>) -> Vec<String> {
        let mut opts = serde_json::Map::new();
        opts.insert("typographer".into(), true.into());
        let md = MarkdownIt::with_options("default", &opts).unwrap();
        let mut env = Env::default();
        let mut state = StateCore::new(Vec::new(), &md, &mut env);
        let mut inline = tok("inline", content, "");
        inline.children = Some(children);
        state.tokens = vec![inline];
        replace(&mut state);
        let children = state.tokens[0].children.as_ref().unwrap();
        children.iter().map(|t| t.content.clone()).collect()
    }

    #[test]
    fn replacements_match_node() {
        // node: replacements.ts on one text token (tests/markdown_it/core_values.cjs --examples)
        for (input, expected) in [
            ("a+-b +-+- ++-", "a\u{b1}b \u{b1}\u{b1} +\u{b1}"),
            ("x.. y... z.....", "x\u{2026} y\u{2026} z\u{2026}"),
            ("?... !.. ?. ?..", "?.. !.. ?. ?.."),
            ("?!?!? !!!! ??? a!!!!!b", "??? !!! ??? a!!!b"),
            ("a,,b,,,,c,d", "a,b,c,d"),
            ("a---b ---- c --- d", "a\u{2014}b ---- c \u{2014} d"),
            ("---\n---x", "\u{2014}\n\u{2014}x"),
            ("a -- b --c-- d--e", "a \u{2013} b --c-- d\u{2013}e"),
            ("--\u{2028}--", "\u{2013}\u{2028}\u{2013}"),
            ("x---y---z", "x\u{2014}y\u{2014}z"),
            ("a--b--c", "a\u{2013}b\u{2013}c"),
            ("-- -- --", "\u{2013} \u{2013} \u{2013}"),
            ("a\u{3000}--\u{3000}b", "a\u{3000}\u{2013}\u{3000}b"),
            (
                "(c) (C) (tm) (TM) (tM) (r) (R) (x) ((c)) (c",
                "\u{a9} \u{a9} \u{2122} \u{2122} \u{2122} \u{ae} \u{ae} (x) (\u{a9}) (c",
            ),
            // Kelvin sign: without the `u` flag, `i` folds no non-ASCII letter
            ("(\u{212a})", "(\u{212a})"),
        ] {
            assert_eq!(
                run(input, vec![tok("text", input, "")]),
                [expected],
                "{input:?}"
            );
        }
    }

    #[test]
    fn autolink_text_is_skipped_by_counting_from_the_end() {
        // an extra closing autolink leaves the counter at 1 for the first text token
        let children = vec![
            tok("text", "a--b (c)", ""),
            tok("link_open", "", "auto"),
            tok("text", "x--y (c)", ""),
            tok("link_close", "", "auto"),
            tok("link_close", "", "auto"),
            tok("text", "p--q (r)", ""),
        ];
        assert_eq!(
            run("a--b (c)", children),
            ["a--b (c)", "", "x--y (c)", "", "", "p\u{2013}q \u{ae}"]
        );
    }

    #[test]
    fn regex_tests() {
        assert!(scoped_abbr_test_re(&js::to_utf16("a(Tm)")));
        assert!(!scoped_abbr_test_re(&js::to_utf16("(t m)")));
        assert!(rare_re_test(&js::to_utf16("a????")) && !rare_re_test(&js::to_utf16("a???")));
    }

    #[test]
    fn generated_token_sequences_match_node() {
        use crate::markdown_it::rules_core::smartquotes::gen_tokens::{
            Function, check, run, tok_syms,
        };
        let functions: [Function; 1] = [("replacements", |o, m, c| {
            tok_syms(o, &run(&m[0], &[replace], c))
        })];
        check(&functions);
    }
}
