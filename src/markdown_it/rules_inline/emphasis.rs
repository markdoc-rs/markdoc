//! Port of rules_inline/emphasis.ts: Process *this* and _that_
//!
//! The TS default export is `{ tokenize, postProcess }`; they are registered in `ruler` and
//! `ruler2` respectively.

use super::state_inline::{Delimiter, DelimiterList, StateInline};
use crate::markdown_it::{JsError, js};

/// `emphasis_tokenize(state, silent)` (in `md.inline.ruler`).
///
/// Insert each marker as a separate text token, and add it to delimiter list
pub fn tokenize(state: &mut StateInline, silent: bool) -> bool {
    let start = state.pos;
    let marker = js::char_code_at(&state.src, start);

    if silent {
        return false;
    }

    // 0x5F is `_`, 0x2A is `*`; `None` (JS's NaN, past the end) is neither.
    let marker: u16 = match marker {
        Some(m) if m == 0x5F || m == 0x2A => m,
        _ => return false,
    };

    let scanned = state.scan_delims(state.pos, marker == 0x2A);

    for _ in 0..scanned.length {
        let token = state.push("text", "", 0);
        token.content = js::to_string(&js::from_char_code(&[u32::from(marker)]));

        let token_index = state.tokens.len() - 1;
        state.push_delimiter(Delimiter {
            // Char code of the starting marker (number).
            //
            marker,

            // Total length of these series of delimiters.
            //
            length: scanned.length,

            // A position of the token this delimiter corresponds to.
            //
            token: token_index,

            // If this delimiter is matched as a valid opener, `end` will be
            // equal to its position, otherwise it's `-1`.
            //
            end: -1,

            // Boolean flags that determine if this delimiter could open or close
            // an emphasis.
            //
            open: scanned.can_open,
            close: scanned.can_close,
        });
    }

    state.pos += scanned.length;

    true
}

/// `delimiters[index]` for a JS number index; `None` is `undefined`.
fn at(delimiters: &[Delimiter], index: i128) -> Option<&Delimiter> {
    usize::try_from(index).ok().and_then(|i| delimiters.get(i))
}

fn throw(state: &mut StateInline, message: &str) {
    state.env.throw(JsError::type_error(message));
}

/// `postProcess(state, delimiters)`.
///
/// Where JS reads a property of `undefined` and throws (an `end` or a `token` out of range,
/// which only a plugin's delimiters can hold), the TypeError is recorded on the environment
/// and the post-processing stops.
fn process_delimiters(state: &mut StateInline, delimiters: &DelimiterList) {
    let delimiters = delimiters.borrow();
    let max = delimiters.len() as i64;

    // `for (let i = max - 1; i >= 0; i--)`, whose body also steps `i` back once more when it
    // merges two delimiters into a strong one.
    let mut i: i64 = max - 1;
    while i >= 0 {
        let start_delim = &delimiters[i as usize];

        // 0x5F is `_`, 0x2A is `*`
        if start_delim.marker != 0x5F && start_delim.marker != 0x2A {
            i -= 1;
            continue;
        }

        // Process only opening markers
        if start_delim.end == -1 {
            i -= 1;
            continue;
        }

        let end = i128::from(start_delim.end);
        let end_delim = at(&delimiters, end);

        // If the previous delimiter has the same marker and is adjacent to this one,
        // merge those into one strong delimiter.
        //
        // `<em><em>whatever</em></em>` -> `<strong>whatever</strong>`
        //
        // (Token indexes compare as JS numbers: `startDelim.token - 1` is -1 for token 0.)
        let previous = (i > 0).then(|| &delimiters[(i - 1) as usize]);
        let is_strong = match previous {
            Some(previous)
                if i128::from(previous.end) == end + 1
                    // check that first two markers match and adjacent
                    && previous.marker == start_delim.marker
                    && previous.token as i128 == start_delim.token as i128 - 1 =>
            {
                // check that last two markers are adjacent (we can safely assume they match)
                // `delimiters[startDelim.end + 1].token === endDelim.token + 1`
                match (at(&delimiters, end + 1), end_delim) {
                    (Some(after), Some(end_delim)) => {
                        after.token as i128 == end_delim.token as i128 + 1
                    }
                    _ => {
                        return throw(
                            state,
                            "Cannot read properties of undefined (reading 'token')",
                        );
                    }
                }
            }
            _ => false,
        };

        let ch = js::to_string(&js::from_char_code(&[u32::from(start_delim.marker)]));

        let Some(token_o) = state.tokens.get_mut(start_delim.token) else {
            return throw(state, "Cannot set properties of undefined (setting 'type')");
        };
        token_o.kind = if is_strong { "strong_open" } else { "em_open" }.to_owned();
        token_o.tag = if is_strong { "strong" } else { "em" }.to_owned();
        token_o.nesting = 1;
        token_o.markup = if is_strong {
            format!("{ch}{ch}")
        } else {
            ch.clone()
        };
        token_o.content = String::new();

        let Some(end_delim) = end_delim else {
            return throw(
                state,
                "Cannot read properties of undefined (reading 'token')",
            );
        };
        let Some(token_c) = state.tokens.get_mut(end_delim.token) else {
            return throw(state, "Cannot set properties of undefined (setting 'type')");
        };
        token_c.kind = if is_strong {
            "strong_close"
        } else {
            "em_close"
        }
        .to_owned();
        token_c.tag = if is_strong { "strong" } else { "em" }.to_owned();
        token_c.nesting = -1;
        token_c.markup = if is_strong {
            format!("{ch}{ch}")
        } else {
            ch.clone()
        };
        token_c.content = String::new();

        if is_strong {
            // both delimiters were read for `isStrong`
            let outer_open = delimiters[(i - 1) as usize].token;
            let outer_close = at(&delimiters, end + 1).map_or(usize::MAX, |d| d.token);
            for token in [outer_open, outer_close] {
                let Some(token) = state.tokens.get_mut(token) else {
                    return throw(
                        state,
                        "Cannot set properties of undefined (setting 'content')",
                    );
                };
                token.content = String::new();
            }
            i -= 1;
        }
        i -= 1;
    }
}

/// `emphasis_post_process(state)` (in `md.inline.ruler2`).
///
/// Walk through delimiter list and replace text tokens with tags
pub fn post_process(state: &mut StateInline) {
    let max = state.tokens_meta.len();

    let delimiters = state.current_delimiters();
    process_delimiters(state, &delimiters);

    for curr in 0..max {
        let delimiters = state.tokens_meta[curr]
            .as_ref()
            .and_then(|meta| meta.delimiters.clone());
        if let Some(delimiters) = delimiters {
            process_delimiters(state, &delimiters);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::link::test_support::{delimiters_of, show_all};
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    /// node (unit_values.cjs `emph_tokenize`): `st = new md.inline.State(src, md, {}, [])`,
    /// `st.pos = pos`, the emphasis rule `(st, false)`.
    fn tokenize_at(src: &str, pos: usize) -> (bool, usize, Vec<String>, Vec<String>) {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16(src), &md, &mut env, Vec::new());
        st.pos = pos;
        let ok = tokenize(&mut st, false);
        (ok, st.pos, show_all(&st.tokens), delimiters_of(&st))
    }

    #[test]
    fn tokenize_pushes_a_text_token_and_a_delimiter_per_marker() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let star = "text@0\"*\"";
        assert_eq!(
            tokenize_at("***b", 0),
            (
                true,
                3,
                s(&[star, star, star]),
                s(&["42:3:0:-1:o-", "42:3:1:-1:o-", "42:3:2:-1:o-"])
            )
        );
        // `_` cannot split a word
        assert_eq!(
            tokenize_at("a_b", 1),
            (true, 2, s(&["text@0\"_\""]), s(&["95:1:0:-1:--"]))
        );
        assert_eq!(
            tokenize_at("a **", 2),
            (
                true,
                4,
                s(&[star, star]),
                s(&["42:2:0:-1:--", "42:2:1:-1:--"])
            )
        );
        // an astral symbol before the run counts as punctuation
        assert_eq!(
            tokenize_at("\u{1F600}_a", 2),
            (true, 3, s(&["text@0\"_\""]), s(&["95:1:0:-1:o-"]))
        );
        assert_eq!(tokenize_at("x", 0), (false, 0, vec![], vec![]));
        // past the end (charCodeAt is NaN)
        assert_eq!(tokenize_at("*", 1), (false, 1, vec![], vec![]));

        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("*a"), &md, &mut env, Vec::new());
        assert!(!tokenize(&mut st, true));
        assert_eq!((st.pos, st.tokens.len()), (0, 0));
    }

    /// node (unit_values.cjs `emphSeq`): the emphasis rule at each of `at` (the text between
    /// runs appended to `pending`), `pushPending()`, the `end`s balance_pairs would set, then
    /// the emphasis post-processing rule.
    fn post(src: &str, at: &[usize], ends: &[(usize, i64)]) -> Vec<String> {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let src = js::to_utf16(src);
        let mut st = StateInline::new(src.clone(), &md, &mut env, Vec::new());
        let mut p = 0;
        for &a in at {
            while p < a {
                st.pending.push(src[p]);
                p += 1;
            }
            st.pos = a;
            assert!(tokenize(&mut st, false));
            p = st.pos;
        }
        st.pending.extend_from_slice(&src[p..]);
        st.push_pending();
        for &(i, end) in ends {
            st.current_delimiters().borrow_mut()[i].end = end;
        }
        post_process(&mut st);
        show_all(&st.tokens)
    }

    #[test]
    fn post_process_turns_matched_delimiters_into_tags() {
        assert_eq!(
            post("*a*", &[0, 2], &[(0, 1)]),
            [
                "em_open(em)@0/*^1",
                "text@0\"a\"",
                "em_close(em)@0/*^-1",
                "text@0"
            ]
        );
        // adjacent pairs of the same marker merge into strong; their outer text is emptied
        assert_eq!(
            post("**a**", &[0, 3], &[(1, 2), (0, 3)]),
            [
                "text@0",
                "strong_open(strong)@0/**^1",
                "text@0\"a\"",
                "strong_close(strong)@0/**^-1",
                "text@0",
                "text@0"
            ]
        );
        // `i--` after a merge: the outermost delimiter is handled on its own
        assert_eq!(
            post("***a***", &[0, 4], &[(2, 3), (1, 4), (0, 5)]),
            [
                "em_open(em)@0/*^1",
                "text@0",
                "strong_open(strong)@0/**^1",
                "text@0\"a\"",
                "strong_close(strong)@0/**^-1",
                "text@0",
                "em_close(em)@0/*^-1",
                "text@0"
            ]
        );
        // different markers never merge
        assert_eq!(
            post("*_a_*", &[0, 1, 3, 4], &[(1, 2), (0, 3)]),
            [
                "em_open(em)@0/*^1",
                "em_open(em)@0/_^1",
                "text@0\"a\"",
                "em_close(em)@0/_^-1",
                "em_close(em)@0/*^-1",
                "text@0"
            ]
        );
        assert_eq!(
            post("*a_b*", &[0, 2, 4], &[(0, 2)]),
            [
                "em_open(em)@0/*^1",
                "text@0\"a\"",
                "text@0\"_\"",
                "text@0\"b\"",
                "em_close(em)@0/*^-1",
                "text@0"
            ]
        );
    }

    #[test]
    fn post_process_walks_the_lists_in_tokens_meta() {
        // node (unit_values.cjs `emph_post nested`): '*x*y*z*' with a link_open pushed after
        // the first run and a link_close after the third
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("*x*y*z*"), &md, &mut env, Vec::new());
        tokenize(&mut st, false);
        st.push("link_open", "a", 1);
        for (text, at) in [("x", 2), ("y", 4)] {
            st.pending.extend(js::to_utf16(text));
            st.pos = at;
            tokenize(&mut st, false);
        }
        st.push("link_close", "a", -1);
        st.pending.extend(js::to_utf16("z"));
        st.pos = 6;
        tokenize(&mut st, false);
        st.push_pending();
        let inner = st.tokens_meta[1]
            .as_ref()
            .unwrap()
            .delimiters
            .clone()
            .unwrap();
        inner.borrow_mut()[0].end = 1;
        st.current_delimiters().borrow_mut()[0].end = 1;
        post_process(&mut st);
        assert_eq!(
            show_all(&st.tokens),
            [
                "em_open(em)@0/*^1",
                "link_open(a)@0^1",
                "text@1\"x\"",
                "em_open(em)@1/*^1",
                "text@1\"y\"",
                "em_close(em)@1/*^-1",
                "link_close(a)@0^-1",
                "text@0\"z\"",
                "em_close(em)@0/*^-1",
                "text@0"
            ]
        );
        let lists: Vec<Option<usize>> = st
            .tokens_meta
            .iter()
            .map(|m| {
                m.as_ref()
                    .map(|m| m.delimiters.as_ref().unwrap().borrow().len())
            })
            .collect();
        assert_eq!(lists, [None, Some(2), None, None, None, None]);
    }
}
