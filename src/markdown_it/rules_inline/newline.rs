//! Port of rules_inline/newline.ts.
//!
//! Proceess '\n'

use super::state_inline::StateInline;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::js;

pub fn newline(state: &mut StateInline, silent: bool) -> bool {
    let mut pos = state.pos;

    if js::char_code_at(&state.src, pos) != Some(0x0A /* \n */) {
        return false;
    }

    // `state.pending.length - 1`: -1 when nothing is pending.
    let pmax = state.pending.len() as i64 - 1;
    let max = state.pos_max;

    // '  \n' -> hardbreak
    // Lookup in pending chars is bad practice! Don't copy to other rules!
    // Pending string is stored in concat mode, indexed lookups will cause
    // convertion to flat mode.
    if !silent {
        if pmax >= 0 && state.pending[pmax as usize] == 0x20 {
            if pmax >= 1 && state.pending[pmax as usize - 1] == 0x20 {
                // Find whitespaces tail of pending chars.
                let mut ws = pmax as usize - 1;
                while ws >= 1 && state.pending[ws - 1] == 0x20 {
                    ws -= 1;
                }

                // state.pending = state.pending.slice(0, ws)
                state.pending.truncate(ws);
                state.push("hardbreak", "br", 0);
            } else {
                // state.pending = state.pending.slice(0, -1)
                state.pending.truncate(pmax as usize);
                state.push("softbreak", "br", 0);
            }
        } else {
            state.push("softbreak", "br", 0);
        }
    }

    pos += 1;

    // skip heading spaces for next line
    while pos < max && js::char_code_at(&state.src, pos).is_some_and(is_space) {
        pos += 1;
    }

    state.pos = pos;
    true
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
    fn spaces_before_a_newline_make_a_hardbreak() {
        // node (tests/markdown_it/inline_rule_values.cjs, `newline`): only text and newline
        let cases: [(&str, &[&str]); 10] = [
            ("a\nb", &["text|a||", "softbreak|||", "text|b||"]),
            ("a \nb", &["text|a||", "softbreak|||", "text|b||"]),
            ("a  \nb", &["text|a||", "hardbreak|||", "text|b||"]),
            ("a   \n  b", &["text|a||", "hardbreak|||", "text|b||"]),
            ("   \nb", &["hardbreak|||", "text|b||"]),
            (" \nb", &["softbreak|||", "text|b||"]),
            // only U+0020 counts before the newline; tabs and spaces are skipped after it
            ("a\t\nb", &["text|a\t||", "softbreak|||", "text|b||"]),
            ("a \t \nb", &["text|a \t||", "softbreak|||", "text|b||"]),
            ("a\n \t b", &["text|a||", "softbreak|||", "text|b||"]),
            (
                "a\n\u{A0}b",
                &["text|a||", "softbreak|||", "text|\u{A0}b||"],
            ),
        ];
        for (src, want) in cases {
            assert_eq!(only(&["text", "newline"], src), want, "{src:?}");
        }
    }
}
