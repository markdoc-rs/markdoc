//! Port of rules_inline/strikethrough.ts: ~~strike through~~
//!
//! The TS default export is `{ tokenize, postProcess }`; they are registered in `ruler` and
//! `ruler2` respectively.

use super::state_inline::{Delimiter, DelimiterList, StateInline};
use crate::markdown_it::{JsError, js};

/// `strikethrough_tokenize(state, silent)` (in `md.inline.ruler`).
///
/// Insert each marker as a separate text token, and add it to delimiter list
#[allow(clippy::manual_is_multiple_of)] // keeps JS's `if (len % 2)`
pub fn tokenize(state: &mut StateInline, silent: bool) -> bool {
    let start = state.pos;
    let marker = js::char_code_at(&state.src, start);

    if silent {
        return false;
    }

    // `None` is JS's NaN (past the end), which is not `~` either.
    if marker != Some(0x7E /* ~ */) {
        return false;
    }
    let marker: u16 = 0x7E;

    let scanned = state.scan_delims(state.pos, true);
    let mut len = scanned.length;
    let ch = js::to_string(&js::from_char_code(&[u32::from(marker)]));

    if len < 2 {
        return false;
    }

    if len % 2 != 0 {
        let token = state.push("text", "", 0);
        token.content = ch.clone();
        len -= 1;
    }

    let mut i = 0;
    while i < len {
        let token = state.push("text", "", 0);
        token.content = format!("{ch}{ch}");

        let token_index = state.tokens.len() - 1;
        state.push_delimiter(Delimiter {
            marker,
            length: 0, // disable "rule of 3" length checks meant for emphasis
            token: token_index,
            end: -1,
            open: scanned.can_open,
            close: scanned.can_close,
        });
        i += 2;
    }

    state.pos += scanned.length;

    true
}

/// `postProcess(state, delimiters)`.
///
/// Where JS reads a property of `undefined` and throws (an `end` or a `token` out of range,
/// which only a plugin's delimiters can hold), the TypeError is recorded on the environment
/// and the post-processing stops.
fn process_delimiters(state: &mut StateInline, delimiters: &DelimiterList) {
    let delimiters = delimiters.borrow();
    let mut lone_markers: Vec<usize> = Vec::new();
    let max = delimiters.len();

    for i in 0..max {
        let start_delim = &delimiters[i];

        // 0x7E is `~`
        if start_delim.marker != 0x7E {
            continue;
        }

        if start_delim.end == -1 {
            continue;
        }

        let end_delim = at(&delimiters, i128::from(start_delim.end));

        let Some(token) = state.tokens.get_mut(start_delim.token) else {
            return throw(state, "Cannot set properties of undefined (setting 'type')");
        };
        token.kind = "s_open".to_owned();
        token.tag = "s".to_owned();
        token.nesting = 1;
        token.markup = "~~".to_owned();
        token.content = String::new();

        let Some(end_delim) = end_delim else {
            return throw(
                state,
                "Cannot read properties of undefined (reading 'token')",
            );
        };
        let Some(token) = state.tokens.get_mut(end_delim.token) else {
            return throw(state, "Cannot set properties of undefined (setting 'type')");
        };
        token.kind = "s_close".to_owned();
        token.tag = "s".to_owned();
        token.nesting = -1;
        token.markup = "~~".to_owned();
        token.content = String::new();

        // `state.tokens[endDelim.token - 1]`: undefined for index -1
        let Some(before) = end_delim.token.checked_sub(1) else {
            return throw(
                state,
                "Cannot read properties of undefined (reading 'type')",
            );
        };
        if state.tokens[before].kind == "text" && state.tokens[before].content == "~" {
            lone_markers.push(before);
        }
    }

    // If a marker sequence has an odd number of characters, it's splitted
    // like this: `~~~~~` -> `~` + `~~` + `~~`, leaving one marker at the
    // start of the sequence.
    //
    // So, we have to move all those markers after subsequent s_close tags.
    //
    while let Some(i) = lone_markers.pop() {
        let mut j = i + 1;

        while j < state.tokens.len() && state.tokens[j].kind == "s_close" {
            j += 1;
        }

        j -= 1;

        if i != j {
            state.tokens.swap(i, j);
        }
    }
}

/// `delimiters[index]` for a JS number index; `None` is `undefined`.
fn at(delimiters: &[Delimiter], index: i128) -> Option<&Delimiter> {
    usize::try_from(index).ok().and_then(|i| delimiters.get(i))
}

fn throw(state: &mut StateInline, message: &str) {
    state.env.throw(JsError::type_error(message));
}

/// `strikethrough_postProcess(state)` (in `md.inline.ruler2`).
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

    /// node (unit_values.cjs `strike_tokenize`): `st = new md.inline.State(src, md, {}, [])`,
    /// `st.pos = pos`, the strikethrough rule `(st, false)`.
    fn tokenize_at(src: &str, pos: usize) -> (bool, usize, Vec<String>, Vec<String>) {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16(src), &md, &mut env, Vec::new());
        st.pos = pos;
        let ok = tokenize(&mut st, false);
        (ok, st.pos, show_all(&st.tokens), delimiters_of(&st))
    }

    #[test]
    fn tokenize_splits_runs_into_pairs() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // an odd run starts with a lone `~` that gets no delimiter; pairs get length 0
        assert_eq!(
            tokenize_at("~~~a", 0),
            (
                true,
                3,
                s(&["text@0\"~\"", "text@0\"~~\""]),
                s(&["126:0:1:-1:o-"])
            )
        );
        assert_eq!(tokenize_at("~a", 0), (false, 0, vec![], vec![]));
        assert_eq!(
            tokenize_at("~~", 0),
            (true, 2, s(&["text@0\"~~\""]), s(&["126:0:0:-1:--"]))
        );
        assert_eq!(
            tokenize_at("a~~~~b", 1),
            (
                true,
                5,
                s(&["text@0\"~~\"", "text@0\"~~\""]),
                s(&["126:0:0:-1:oc", "126:0:1:-1:oc"])
            )
        );
        assert_eq!(tokenize_at("x", 0), (false, 0, vec![], vec![]));

        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("~~a"), &md, &mut env, Vec::new());
        assert!(!tokenize(&mut st, true));
        assert_eq!((st.pos, st.tokens.len()), (0, 0));
    }

    /// node (unit_values.cjs `strikeSeq`): the strikethrough rule at each of `at` (the text
    /// between runs appended to `pending`), `pushPending()`, the `end`s balance_pairs would
    /// set, then the strikethrough post-processing rule.
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
    fn post_process_moves_lone_markers_after_closing_tags() {
        assert_eq!(
            post("~~~a~~~", &[0, 4], &[(0, 1)]),
            [
                "text@0\"~\"",
                "s_open(s)@0/~~^1",
                "text@0\"a\"",
                "s_close(s)@0/~~^-1",
                "text@0\"~\"",
                "text@0"
            ]
        );
        assert_eq!(
            post("~~~~~a~~~~~", &[0, 6], &[(1, 2), (0, 3)]),
            [
                "text@0\"~\"",
                "s_open(s)@0/~~^1",
                "s_open(s)@0/~~^1",
                "text@0\"a\"",
                "s_close(s)@0/~~^-1",
                "s_close(s)@0/~~^-1",
                "text@0\"~\"",
                "text@0"
            ]
        );
        assert_eq!(
            post("~~a~~~", &[0, 3], &[(0, 1)]),
            [
                "s_open(s)@0/~~^1",
                "text@0\"a\"",
                "s_close(s)@0/~~^-1",
                "text@0\"~\"",
                "text@0"
            ]
        );
        assert_eq!(post("~~a", &[0], &[]), ["text@0\"~~\"", "text@0\"a\""]);
    }
}
