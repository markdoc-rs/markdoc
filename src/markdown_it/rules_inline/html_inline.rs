//! Port of rules_inline/html_inline.ts.
//!
//! Process html tags

use super::state_inline::StateInline;
use crate::markdown_it::common::html_re::html_tag_re;
use crate::markdown_it::js;

/// `/^<a[>\s]/i.test(str)`: hand-written; with the `i` flag (non-unicode) only `a` and `A`
/// match `a`, and `\s` is JS's whitespace set.
fn is_link_open(s: &[u16]) -> bool {
    s.len() >= 3
        && s[0] == 0x3C /* < */
        && (s[1] | 0x20) == 0x61 /* a */
        && (s[2] == 0x3E /* > */ || js::is_js_space(u32::from(s[2])))
}

/// `/^<\/a\s*>/i.test(str)`: hand-written, as [`is_link_open`].
fn is_link_close(s: &[u16]) -> bool {
    if s.len() < 3 || s[0] != 0x3C /* < */ || s[1] != 0x2F /* / */ || (s[2] | 0x20) != 0x61 {
        return false;
    }
    let mut i = 3;
    while i < s.len() && js::is_js_space(u32::from(s[i])) {
        i += 1;
    }
    s.get(i) == Some(&0x3E /* > */)
}

fn is_letter(ch: u16) -> bool {
    /* eslint no-bitwise:0 */
    let lc = ch | 0x20; // to lower case
    (lc >= 0x61/* a */) && (lc <= 0x7a/* z */)
}

pub fn html_inline(state: &mut StateInline, silent: bool) -> bool {
    if !state.md.options.html {
        return false;
    }

    // Check start
    let max = state.pos_max;
    let pos = state.pos;
    if js::char_code_at(&state.src, pos) != Some(0x3C /* < */) || pos + 2 >= max {
        return false;
    }

    // Quick fail on second char
    // (`isLetter(NaN)` is false: `NaN | 0x20` is 0x20)
    let ch = js::char_code_at(&state.src, pos + 1);
    if ch != Some(0x21 /* ! */)
        && ch != Some(0x3F /* ? */)
        && ch != Some(0x2F /* / */)
        && !ch.is_some_and(is_letter)
    {
        return false;
    }

    // state.src.slice(pos).match(HTML_TAG_RE): the match may run past `posMax`, as in JS.
    let Some(match_len) = html_tag_re(js::slice_to(&state.src, pos, state.src.len())) else {
        return false;
    };

    if !silent {
        let matched = state.src[pos..pos + match_len].to_vec();
        let token = state.push("html_inline", "", 0);
        token.content = js::to_string(&matched);

        if is_link_open(&matched) {
            state.link_level += 1;
        }
        if is_link_close(&matched) {
            state.link_level -= 1;
        }
    }
    state.pos += match_len;
    true
}

#[cfg(test)]
#[allow(clippy::type_complexity)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    #[test]
    fn link_open_and_close_tests_match_the_regexes() {
        // node (tests/markdown_it/inline_rule_values.cjs, `link_open_close`):
        // [/^<a[>\s]/i.test(s), /^<\/a\s*>/i.test(s)]
        let cases: [(&str, (bool, bool)); 11] = [
            ("<a>", (true, false)),
            ("<A>", (true, false)),
            ("<a b>", (true, false)),
            ("<a\u{A0}b>", (true, false)),
            ("<a\u{85}b>", (false, false)),
            ("<ab>", (false, false)),
            ("</a>", (false, true)),
            ("</A  >", (false, true)),
            ("</a\u{3000}>", (false, true)),
            ("</ab>", (false, false)),
            ("<a/>", (false, false)),
        ];
        for (s, want) in cases {
            let u = js::to_utf16(s);
            assert_eq!((is_link_open(&u), is_link_close(&u)), want, "{s:?}");
        }
    }

    #[test]
    fn html_inline_counts_link_levels() {
        // node (`html_inline_level`): html_inline(new md.inline.State(src, md, {}, []), false)
        // with html on: [result, state.pos, state.linkLevel, tokens as type|content|level]
        let md = MarkdownIt::with_options(
            "default",
            serde_json::json!({ "html": true }).as_object().unwrap(),
        )
        .unwrap();
        let cases: [(&str, (bool, usize, i32, &[&str])); 7] = [
            (
                "<a href=\"x\">",
                (true, 12, 1, &["html_inline|<a href=\"x\">|0"]),
            ),
            ("</a>", (true, 4, -1, &["html_inline|</a>|0"])),
            ("<A\tb>", (true, 5, 1, &["html_inline|<A\tb>|0"])),
            ("</a >", (true, 5, -1, &["html_inline|</a >|0"])),
            ("<span>", (true, 6, 0, &["html_inline|<span>|0"])),
            ("<a", (false, 0, 0, &[])),
            ("x", (false, 0, 0, &[])),
        ];
        for (src, (ok, pos, link_level, tokens)) in cases {
            let mut env = Env::default();
            let mut st = StateInline::new(js::to_utf16(src), &md, &mut env, Vec::new());
            let got_ok = html_inline(&mut st, false);
            let got_tokens: Vec<String> = st
                .tokens
                .iter()
                .map(|t| format!("{}|{}|{}", t.kind, t.content, t.level))
                .collect();
            assert_eq!(
                (got_ok, st.pos, st.link_level, got_tokens),
                (
                    ok,
                    pos,
                    link_level,
                    tokens.iter().map(|t| t.to_string()).collect()
                ),
                "{src:?}"
            );
        }
    }
}
