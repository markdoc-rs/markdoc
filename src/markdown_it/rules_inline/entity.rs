//! Port of rules_inline/entity.ts.
//!
//! Process html entity - &#123;, &#xAF;, &quot;, ...

use super::state_inline::StateInline;
use crate::markdown_it::common::utils::{
    decode_html_strict, from_code_point, is_valid_entity_code,
};
use crate::markdown_it::js;

fn is_ascii_alpha(c: u16) -> bool {
    (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c)
}

fn is_ascii_digit(c: u16) -> bool {
    (0x30..=0x39).contains(&c)
}

fn is_ascii_hex_digit(c: u16) -> bool {
    is_ascii_digit(c) || (0x41..=0x46).contains(&c) || (0x61..=0x66).contains(&c)
}

/// The length of the run of `pred` code units at the start of `s`.
fn run(s: &[u16], pred: fn(u16) -> bool) -> usize {
    s.iter().take_while(|&&c| pred(c)).count()
}

// const DIGITAL_RE = /^&#((?:x[a-f0-9]{1,6}|[0-9]{1,7}));/i
//
// Hand-written: `s.match(DIGITAL_RE)` as the length of `match[0]` (`match[1]` is
// `s[2..len - 1]`). With the `i` flag (non-unicode) the classes are exactly their ASCII
// letters in both cases, and no non-ASCII code unit matches. Neither class has `;`, so a
// greedy run that backtracks only leaves a digit where `;` must be: the run after `x` must be
// 1 to 6 hex digits, or the run of decimal digits 1 to 7 long, followed by `;`. The first
// alternative needs `x` and the second a digit, so at most one can match.
fn digital_re_match(s: &[u16]) -> Option<usize> {
    if s.len() < 2 || s[0] != u16::from(b'&') || s[1] != u16::from(b'#') {
        return None;
    }
    let body = &s[2..];
    let group = match body.first() {
        Some(&c) if c == 0x78 /* x */ || c == 0x58 /* X */ => {
            let digits = run(&body[1..], is_ascii_hex_digit);
            if !(1..=6).contains(&digits) {
                return None;
            }
            1 + digits
        }
        _ => {
            let digits = run(body, is_ascii_digit);
            if !(1..=7).contains(&digits) {
                return None;
            }
            digits
        }
    };
    if body.get(group) != Some(&0x3B /* ; */) {
        return None;
    }
    Some(2 + group + 1)
}

// const NAMED_RE = /^&([a-z][a-z0-9]{1,31});/i
//
// Hand-written, as `digital_re_match`: a letter, then a run of 1 to 31 letters or digits,
// then `;`.
fn named_re_match(s: &[u16]) -> Option<usize> {
    if s.len() < 2 || s[0] != 0x26 /* & */ || !is_ascii_alpha(s[1]) {
        return None;
    }
    let rest = run(&s[2..], |c| is_ascii_alpha(c) || is_ascii_digit(c));
    if !(1..=31).contains(&rest) || s.get(2 + rest) != Some(&0x3B /* ; */) {
        return None;
    }
    Some(2 + rest + 1)
}

pub fn entity(state: &mut StateInline, silent: bool) -> bool {
    let pos = state.pos;
    let max = state.pos_max;

    if js::char_code_at(&state.src, pos) != Some(0x26 /* & */) {
        return false;
    }

    if pos + 1 >= max {
        return false;
    }

    let ch = js::char_code_at(&state.src, pos + 1);

    if ch == Some(0x23 /* # */) {
        // state.src.slice(pos).match(DIGITAL_RE)
        let rest = js::slice_to(&state.src, pos, state.src.len());
        if let Some(match_len) = digital_re_match(rest) {
            if !silent {
                let match0 = rest[..match_len].to_vec();
                let match1 = &match0[2..match_len - 1];
                // match[1][0].toLowerCase() === 'x' ? parseInt(match[1].slice(1), 16) : parseInt(match[1], 10)
                let code = if js::to_lower_case(&match1[..1]) == [0x78 /* x */] {
                    js::parse_int(&match1[1..], 16)
                } else {
                    js::parse_int(match1, 10)
                };
                // at most 0xFFFFFF or 9999999: an exact integer that fits in u32
                let code = code as u32;

                let token = state.push("text_special", "", 0);
                token.content = js::to_string(&if is_valid_entity_code(code) {
                    from_code_point(code)
                } else {
                    from_code_point(0xFFFD)
                });
                token.markup = js::to_string(&match0);
                token.info = "entity".into();
            }
            state.pos += match_len;
            return true;
        }
    } else {
        // state.src.slice(pos).match(NAMED_RE)
        let rest = js::slice_to(&state.src, pos, state.src.len());
        if let Some(match_len) = named_re_match(rest) {
            let match0 = rest[..match_len].to_vec();
            let decoded = decode_html_strict(&match0);
            if decoded != match0 {
                if !silent {
                    let token = state.push("text_special", "", 0);
                    token.content = js::to_string(&decoded);
                    token.markup = js::to_string(&match0);
                    token.info = "entity".into();
                }
                state.pos += match_len;
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::{Env, MarkdownIt};

    /// `parseInline` with only `rules` in the inline ruler and `text_join` off: the children
    /// as `type|content|markup|info`.
    fn only(rules: &[&str], src: &str) -> Vec<String> {
        let mut md = MarkdownIt::default();
        md.inline.ruler.enable_only(rules, false).unwrap();
        md.core.ruler.disable(&["text_join"], false).unwrap();
        let tokens = md.parse_inline(src, &mut Env::default());
        let children = tokens[0].children.as_ref().unwrap();
        children
            .iter()
            .map(|t| format!("{}|{}|{}|{}", t.kind, t.content, t.markup, t.info))
            .collect()
    }

    #[test]
    fn numeric_and_named_entities_match_entity_ts() {
        // node (tests/markdown_it/inline_rule_values.cjs, `entity`): only text and entity
        let a32 = format!("&{};", "a".repeat(32));
        let a32_text = format!("text|{a32}||");
        let cases: [(&str, &str); 14] = [
            ("&#0;", "text_special|\u{FFFD}|&#0;|entity"),
            // ...FFFF is not a valid entity code
            ("&#x10FFFF;", "text_special|\u{FFFD}|&#x10FFFF;|entity"),
            ("&#x10FFFD;", "text_special|\u{10FFFD}|&#x10FFFD;|entity"),
            ("&#9999999;", "text_special|\u{FFFD}|&#9999999;|entity"),
            ("&#x1F600;", "text_special|\u{1F600}|&#x1F600;|entity"),
            // eight digits are too many
            ("&#12345678;", "text|&#12345678;||"),
            ("&#xD800;", "text_special|\u{FFFD}|&#xD800;|entity"),
            ("&#X41;", "text_special|A|&#X41;|entity"),
            ("&NotAnEntity;", "text|&NotAnEntity;||"),
            ("&ngE;", "text_special|\u{2267}\u{338}|&ngE;|entity"),
            ("&amp", "text|&amp||"),
            ("&a;", "text|&a;||"),
            ("&#x;", "text|&#x;||"),
            (&a32, &a32_text),
        ];
        for (src, want) in cases {
            assert_eq!(only(&["text", "entity"], src), [want], "{src:?}");
        }
    }
}
