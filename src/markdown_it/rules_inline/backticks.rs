//! Port of rules_inline/backticks.ts.
//!
//! Parse backticks

use std::collections::HashMap;

use super::state_inline::StateInline;
use crate::markdown_it::js;

const BACKTICK: [u16; 1] = [0x60 /* ` */];

/// Position of the last backtick run of each length. Derived from the string in
/// one pass, so unlike an incremental cache it never depends on how the parser
/// walks it, and stays valid across lookaheads and narrowed ranges.
fn build_last_runs(src: &[u16]) -> HashMap<usize, usize> {
    let mut last_runs: HashMap<usize, usize> = HashMap::new();
    let mut pos = 0;

    // while ((pos = src.indexOf('`', pos)) !== -1)
    while let Some(found) = js::index_of(src, &BACKTICK, pos) {
        pos = found;
        let start = pos;
        // while (src.charCodeAt(++pos) === 0x60/* ` */) { /* scan run length */ }
        loop {
            pos += 1;
            if js::char_code_at(src, pos) != Some(0x60 /* ` */) {
                break;
            }
        }
        last_runs.insert(pos - start, start);
    }

    last_runs
}

pub fn backtick(state: &mut StateInline, silent: bool) -> bool {
    let start = state.pos;

    if js::char_code_at(&state.src, start) != Some(0x60 /* ` */) {
        return false;
    }

    let max = state.pos_max;
    let mut pos = start + 1;

    // scan marker length
    while pos < max && js::char_code_at(&state.src, pos) == Some(0x60 /* ` */) {
        pos += 1;
    }

    let marker = js::slice_to(&state.src, start, pos).to_vec();
    let opener_length = marker.len();

    if !state.backticks_scanned {
        state.backticks = build_last_runs(&state.src);
        state.backticks_scanned = true;
    }

    // Nothing of the same length left in the string, so no closer can exist
    // (`(state.backticks[openerLength] ?? -1) >= pos`)
    if state
        .backticks
        .get(&opener_length)
        .is_some_and(|&last| last >= pos)
    {
        let mut match_end = pos;

        // while ((matchStart = state.src.indexOf('`', matchEnd)) !== -1 && matchStart < max)
        while let Some(match_start) = js::index_of(&state.src, &BACKTICK, match_end)
            && match_start < max
        {
            // Measure the whole run, not just its part inside the current range,
            // otherwise a run straddling `max` would be mistaken for a shorter one
            match_end = match_start + 1;
            while js::char_code_at(&state.src, match_end) == Some(0x60 /* ` */) {
                match_end += 1;
            }

            if match_end > max {
                break;
            }

            if match_end - match_start == opener_length {
                if !silent {
                    // .replace(/\n/g, ' ')
                    let mut content: Vec<u16> = js::slice_to(&state.src, pos, match_start)
                        .iter()
                        .map(|&c| if c == 0x0A { 0x20 } else { c })
                        .collect();

                    // Strip one space from each side, unless the content is all spaces
                    // (`/[^ ]/.test(content)`)
                    if js::starts_with(&content, &[0x20], 0)
                        && js::ends_with(&content, &[0x20], None)
                        && content.iter().any(|&c| c != 0x20)
                    {
                        content = js::slice(&content, 1, Some(-1)).to_vec();
                    }

                    let token = state.push("code_inline", "code", 0);
                    token.markup = js::to_string(&marker);
                    token.content = js::to_string(&content);
                }
                state.pos = match_end;
                return true;
            }
        }
    }

    if !silent {
        state.pending.extend_from_slice(&marker);
    }
    state.pos = pos;
    true
}

#[cfg(test)]
#[allow(clippy::type_complexity)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    /// backtick() on a fresh state at `pos` (and `pos_max`): its result, then `state.pos`,
    /// `state.pending`, the run table and the tokens as `type|content|markup|level`.
    fn run(
        src: &str,
        pos: usize,
        pos_max: Option<usize>,
        silent: bool,
    ) -> (bool, usize, String, Vec<(usize, usize)>, Vec<String>) {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16(src), &md, &mut env, Vec::new());
        st.pos = pos;
        if let Some(m) = pos_max {
            st.pos_max = m;
        }
        let ok = backtick(&mut st, silent);
        let mut runs: Vec<(usize, usize)> = st.backticks.iter().map(|(&k, &v)| (k, v)).collect();
        runs.sort_unstable();
        let tokens = st
            .tokens
            .iter()
            .map(|t| format!("{}|{}|{}|{}", t.kind, t.content, t.markup, t.level))
            .collect();
        (ok, st.pos, js::to_string(&st.pending), runs, tokens)
    }

    #[test]
    fn code_spans_and_run_lengths_match_backticks_ts() {
        // node (tests/markdown_it/inline_rule_values.cjs, `backticks`): s = new
        // md.inline.State(src, md, {}, []), s.pos = pos (s.posMax = posMax), backtick(s, silent)
        let cases: [(
            &str,
            usize,
            Option<usize>,
            bool,
            (bool, usize, &str, &[(usize, usize)], &[&str]),
        ); 10] = [
            (
                "a`b``c`d```e``",
                1,
                None,
                false,
                (
                    true,
                    7,
                    "",
                    &[(1, 6), (2, 12), (3, 8)],
                    &["code_inline|b``c|`|0"],
                ),
            ),
            // the closing run straddles posMax: it is measured whole, so it is too long
            ("``a``x", 0, Some(4), false, (true, 2, "``", &[(2, 3)], &[])),
            (
                "``a``x",
                0,
                None,
                false,
                (true, 5, "", &[(2, 3)], &["code_inline|a|``|0"]),
            ),
            // the last run of length 1 starts before pos: no closer is looked for
            (
                "`a``",
                0,
                Some(3),
                false,
                (true, 1, "`", &[(1, 0), (2, 2)], &[]),
            ),
            (
                "` a `",
                0,
                None,
                false,
                (true, 5, "", &[(1, 4)], &["code_inline|a|`|0"]),
            ),
            (
                "`  `",
                0,
                None,
                false,
                (true, 4, "", &[(1, 3)], &["code_inline|  |`|0"]),
            ),
            (
                "`\na\n`",
                0,
                None,
                false,
                (true, 5, "", &[(1, 4)], &["code_inline|a|`|0"]),
            ),
            ("``", 0, None, false, (true, 2, "``", &[(2, 0)], &[])),
            ("`x``", 0, None, true, (true, 1, "", &[(1, 0), (2, 2)], &[])),
            ("a`", 1, None, false, (true, 2, "`", &[(1, 1)], &[])),
        ];
        for (src, pos, pos_max, silent, (ok, end, pending, runs, tokens)) in cases {
            assert_eq!(
                run(src, pos, pos_max, silent),
                (
                    ok,
                    end,
                    pending.to_owned(),
                    runs.to_vec(),
                    tokens.iter().map(|t| t.to_string()).collect()
                ),
                "{src:?} at {pos}"
            );
        }
    }

    #[test]
    fn last_runs_record_the_last_start_of_each_length() {
        // node (`backticks`, first case): state.backticks for 'a`b``c`d```e``'
        let runs = build_last_runs(&js::to_utf16("a`b``c`d```e``"));
        let mut got: Vec<(usize, usize)> = runs.into_iter().collect();
        got.sort_unstable();
        assert_eq!(got, [(1, 6), (2, 12), (3, 8)]);
        assert!(build_last_runs(&js::to_utf16("no ticks")).is_empty());
    }
}
