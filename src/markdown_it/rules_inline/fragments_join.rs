//! Port of rules_inline/fragments_join.ts.
//!
//! Clean up tokens after emphasis and strikethrough postprocessing:
//! merge adjacent text nodes into one and re-calculate all token levels
//!
//! This is necessary because initially emphasis delimiter markers (*, _, ~)
//! are treated as their own separate text tokens. Then emphasis rule either
//! leaves them as text (needed to merge with adjacent text) or turns them
//! into opening/closing tags (which messes up levels inside).
//!

use super::state_inline::StateInline;
use crate::markdown_it::token::Token;

pub fn fragments_join(state: &mut StateInline) {
    let mut level: i32 = 0;
    let tokens = &mut state.tokens;
    let max = tokens.len();

    let mut curr = 0;
    let mut last = 0;
    while curr < max {
        // re-calculate levels after emphasis/strikethrough turns some text nodes
        // into opening/closing tags
        if tokens[curr].nesting < 0 {
            level -= 1; // closing tag
        }
        tokens[curr].level = level;
        if tokens[curr].nesting > 0 {
            level += 1; // opening tag
        }

        if tokens[curr].kind == "text" && curr + 1 < max && tokens[curr + 1].kind == "text" {
            // collapse two adjacent text nodes
            // (`tokens[curr + 1].content = tokens[curr].content + tokens[curr + 1].content`:
            // the token at `curr` is never read again, so its content is moved and the next
            // one appended, which keeps a run of n text tokens linear, as JS's (rope)
            // string concatenation is, where copying the run so far each time was quadratic)
            let (prev, next) = tokens.split_at_mut(curr + 1);
            if !Token::join_exact_into(&mut prev[curr], &mut next[0]) {
                let mut joined = std::mem::take(&mut prev[curr].content);
                joined.push_str(&next[0].content);
                next[0].content = joined;
            }
        } else {
            // `tokens[last] = tokens[curr]`: a swap, since slot `curr` is never read again
            // and every slot from `last` on is cut off below.
            if curr != last {
                tokens.swap(last, curr);
            }

            last += 1;
        }
        curr += 1;
    }

    if curr != last {
        tokens.truncate(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::token::Token;
    use crate::markdown_it::{Env, MarkdownIt, js};

    fn tok(kind: &str, nesting: i8, content: &str) -> Token {
        let mut t = Token::new(kind, "", nesting);
        t.content = content.into();
        t.level = 7;
        t
    }

    #[test]
    fn merges_text_runs_and_recomputes_levels() {
        // node (tests/markdown_it/inline_rule_values.cjs, `fragments_join`): s.tokens = the
        // tokens below (type, nesting, content, level 7 each); fragments_join(s); then
        // (type, level, content) of each
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let tokens = vec![
            tok("text", 0, "a"),
            tok("text", 0, "b"),
            tok("em_open", 1, ""),
            tok("text", 0, "c"),
            tok("text", 0, "d"),
            tok("text", 0, "e"),
            tok("em_close", -1, ""),
            tok("text_special", 0, "&"),
            tok("text", 0, "f"),
            tok("s_close", -1, ""),
            tok("text", 0, "g"),
            tok("text", 0, "h"),
        ];
        let mut st = StateInline::new(js::to_utf16(""), &md, &mut env, tokens);
        fragments_join(&mut st);
        let got: Vec<(&str, i32, &str)> = st
            .tokens
            .iter()
            .map(|t| (t.kind.as_str(), t.level, t.content.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("text", 0, "ab"),
                ("em_open", 0, ""),
                ("text", 1, "cde"),
                ("em_close", 0, ""),
                ("text_special", 0, "&"),
                ("text", 0, "f"),
                ("s_close", -1, ""),
                ("text", -1, "gh"),
            ]
        );
    }
}
