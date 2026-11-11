//! Port of rules_inline/escape.ts.
//!
//! Process escaped chars and hardbreaks

use super::state_inline::StateInline;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::js;

/// `ESCAPED`: 1 for each code unit below 256 that a backslash escapes, else 0.
const ESCAPED: [u8; 256] = {
    let mut escaped = [0u8; 256];

    let chars = b"\\!\"#$%&'()*+,./:;<=>?@[]^_`{|}~-";
    let mut i = 0;
    while i < chars.len() {
        escaped[chars[i] as usize] = 1;
        i += 1;
    }
    escaped
};

pub fn escape(state: &mut StateInline, silent: bool) -> bool {
    let mut pos = state.pos;
    let max = state.pos_max;

    if js::char_code_at(&state.src, pos) != Some(0x5C /* \ */) {
        return false;
    }
    pos += 1;

    // '\' at the end of the inline block
    if pos >= max {
        return false;
    }

    // `None` is JS's NaN (only past the end of `src`): it equals nothing.
    let mut ch1 = js::char_code_at(&state.src, pos);

    if ch1 == Some(0x0A) {
        if !silent {
            state.push("hardbreak", "br", 0);
        }

        pos += 1;
        // skip leading whitespaces from next line
        while pos < max {
            ch1 = js::char_code_at(&state.src, pos);
            if !ch1.is_some_and(is_space) {
                break;
            }
            pos += 1;
        }

        state.pos = pos;
        return true;
    }

    // '\' before a space is a literal backslash. Don't consume the space, so a
    // trailing two-space hard line break is still detected by the newline rule.
    if ch1 == Some(0x20) {
        if !silent {
            let token = state.push("text_special", "", 0);
            token.content = "\\".into();
            token.markup = "\\".into();
            token.info = "escape".into();
        }

        state.pos = pos;
        return true;
    }

    // `state.src[pos]`: one code unit (`undefined`, stringified, past the end of `src`).
    let mut escaped_str: Vec<u16> = match state.src.get(pos) {
        Some(&c) => vec![c],
        None => js::to_utf16("undefined"),
    };

    if let Some(c1) = ch1
        && c1 >= 0xD800
        && c1 <= 0xDBFF
        && pos + 1 < max
    {
        let ch2 = js::char_code_at(&state.src, pos + 1);

        if let Some(c2) = ch2
            && c2 >= 0xDC00
            && c2 <= 0xDFFF
        {
            escaped_str.push(c2);
            pos += 1;
        }
    }

    let mut orig_str: Vec<u16> = vec![0x5C /* \ */];
    orig_str.extend_from_slice(&escaped_str);

    if !silent {
        let token = state.push("text_special", "", 0);

        if let Some(c1) = ch1
            && c1 < 256
            && ESCAPED[c1 as usize] != 0
        {
            token.content = js::to_string(&escaped_str);
        } else {
            token.content = js::to_string(&orig_str);
        }

        token.markup = js::to_string(&orig_str);
        token.info = "escape".into();
    }

    state.pos = pos + 1;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    #[test]
    fn escaped_table_matches_escape_ts() {
        // node (tests/markdown_it/inline_rule_values.cjs, `escaped_table`): for each c below
        // 256, whether escape() on '\\' + String.fromCharCode(c) gives the bare c as content
        let want = "0000000000000000000000000000000001111111111111110000000000111111\
                    1000000000000000000000000001111110000000000000000000000000011110\
                    0000000000000000000000000000000000000000000000000000000000000000\
                    0000000000000000000000000000000000000000000000000000000000000000";
        let got: String = ESCAPED
            .iter()
            .map(|&e| if e != 0 { '1' } else { '0' })
            .collect();
        assert_eq!(got, want);
    }

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
    fn escapes_hardbreaks_and_literal_backslashes() {
        // node (`escape`): only text, newline and escape
        let cases: [(&str, &[&str]); 8] = [
            ("\\", &["text|\\||"]),
            ("a\\", &["text|a\\||"]),
            ("\\\n  b", &["hardbreak|||", "text|b||"]),
            // a backslash before a space stays, and leaves the space to the newline rule
            (
                "\\ \nb",
                &["text_special|\\|\\|escape", "softbreak|||", "text|b||"],
            ),
            (
                "a\\  \nb",
                &[
                    "text|a||",
                    "text_special|\\|\\|escape",
                    "hardbreak|||",
                    "text|b||",
                ],
            ),
            // an astral character is escaped whole (and kept with its backslash)
            (
                "\\\u{1F600}x",
                &["text_special|\\\u{1F600}|\\\u{1F600}|escape", "text|x||"],
            ),
            ("\\\u{E9}", &["text_special|\\\u{E9}|\\\u{E9}|escape"]),
            ("\\\\", &["text_special|\\|\\\\|escape"]),
        ];
        for (src, want) in cases {
            assert_eq!(only(&["text", "newline", "escape"], src), want, "{src:?}");
        }
    }

    #[test]
    fn lone_surrogates_are_escaped_one_unit_at_a_time() {
        // node (`escape`): '\\\uD83Dx' => text_special '\\\uD83D' (both content and markup),
        // then text 'x'; '\\\uDE00' => text_special '\\\uDE00'. Token strings are Rust
        // strings, so the lone surrogate reads as U+FFFD there (js::to_string).
        let md = MarkdownIt::default();
        for (src, pos) in [(vec![0x5C, 0xD83D, 0x78], 2), (vec![0x5C, 0xDE00], 2)] {
            let mut env = Env::default();
            let mut st = StateInline::new(src, &md, &mut env, Vec::new());
            assert!(escape(&mut st, false));
            assert_eq!(st.pos, pos);
            let t = &st.tokens[0];
            assert_eq!(
                (t.kind.as_str(), t.content.as_str(), t.markup.as_str()),
                ("text_special", "\\\u{FFFD}", "\\\u{FFFD}")
            );
        }
    }
}
