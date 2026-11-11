//! Port of rules_inline/balance_pairs.ts.
//!
//! For each opening emphasis-like marker find a matching closing one
//!

use std::collections::HashMap;

use super::state_inline::{Delimiter, StateInline};

/// `processDelimiters(delimiters)`. Indices that JS keeps in numbers which can be -1 or
/// lower (`openerIdx`, `minOpenerIdx`, `lastTokenIdx`, `jumps`) are `i64`.
// `% 3 === 0` is kept as written in the TS.
#[allow(clippy::manual_is_multiple_of)]
fn process_delimiters(delimiters: &mut [Delimiter]) {
    let mut openers_bottom: HashMap<u16, [i64; 6]> = HashMap::new();
    let max = delimiters.len();

    if max == 0 {
        return;
    }

    // headerIdx is the first delimiter of the current (where closer is) delimiter run
    let mut header_idx: usize = 0;
    let mut last_token_idx: i64 = -2; // needs any value lower than -1
    let mut jumps: Vec<i64> = Vec::new();

    for closer_idx in 0..max {
        // `closer` is `delimiters[closer_idx]` (and `opener` `delimiters[opener_idx]`
        // below): read and written in place, as the JS object is.

        jumps.push(0);

        // markers belong to same delimiter run if:
        //  - they have adjacent tokens
        //  - AND markers are the same
        //
        if delimiters[header_idx].marker != delimiters[closer_idx].marker
            || last_token_idx != delimiters[closer_idx].token as i64 - 1
        {
            header_idx = closer_idx;
        }

        last_token_idx = delimiters[closer_idx].token as i64;

        // Length is only used for emphasis-specific "rule of 3",
        // if it's not defined (in strikethrough or 3rd party plugins),
        // we can default it to 0 to disable those checks.
        //
        // (`closer.length = closer.length || 0`: `length` is always a number here, and
        // strikethrough pushes 0.)

        if !delimiters[closer_idx].close {
            continue;
        }

        // Previously calculated lower bounds (previous fails)
        // for each marker, each delimiter length modulo 3,
        // and for whether this closer can be an opener;
        // https://github.com/commonmark/cmark/commit/34250e12ccebdc6372b8b49c44fab57c72443460
        /* eslint-disable-next-line no-prototype-builtins */
        let closer_marker = delimiters[closer_idx].marker;
        openers_bottom
            .entry(closer_marker)
            .or_insert([-1, -1, -1, -1, -1, -1]);

        let min_opener_idx =
            openers_bottom[&closer_marker][(if delimiters[closer_idx].open { 3 } else { 0 })
                + (delimiters[closer_idx].length % 3)];

        let mut opener_idx: i64 = header_idx as i64 - jumps[header_idx] - 1;

        let mut new_min_opener_idx = opener_idx;

        // for (; openerIdx > minOpenerIdx; openerIdx -= jumps[openerIdx] + 1)
        while opener_idx > min_opener_idx {
            let o = opener_idx as usize;

            if delimiters[o].marker != delimiters[closer_idx].marker {
                opener_idx -= jumps[o] + 1;
                continue;
            }

            if delimiters[o].open && delimiters[o].end < 0 {
                let mut is_odd_match = false;

                // from spec:
                //
                // If one of the delimiters can both open and close emphasis, then the
                // sum of the lengths of the delimiter runs containing the opening and
                // closing delimiters must not be a multiple of 3 unless both lengths
                // are multiples of 3.
                //
                if delimiters[o].close || delimiters[closer_idx].open {
                    if (delimiters[o].length + delimiters[closer_idx].length) % 3 == 0 {
                        if delimiters[o].length % 3 != 0 || delimiters[closer_idx].length % 3 != 0 {
                            is_odd_match = true;
                        }
                    }
                }

                if !is_odd_match {
                    // If previous delimiter cannot be an opener, we can safely skip
                    // the entire sequence in future checks. This is required to make
                    // sure algorithm has linear complexity (see *_*_*_*_*_... case).
                    //
                    let last_jump = if opener_idx > 0 && !delimiters[o - 1].open {
                        jumps[o - 1] + 1
                    } else {
                        0
                    };

                    jumps[closer_idx] = closer_idx as i64 - opener_idx + last_jump;
                    jumps[o] = last_jump;

                    delimiters[closer_idx].open = false;
                    delimiters[o].end = closer_idx as i64;
                    delimiters[o].close = false;
                    new_min_opener_idx = -1;
                    // treat next token as start of run,
                    // it optimizes skips in **<...>**a**<...>** pathological case
                    last_token_idx = -2;
                    break;
                }
            }

            opener_idx -= jumps[o] + 1;
        }

        if new_min_opener_idx != -1 {
            // If match for this delimiter run failed, we want to set lower bound for
            // future lookups. This is required to make sure algorithm has linear
            // complexity.
            //
            // See details here:
            // https://github.com/commonmark/cmark/issues/178#issuecomment-270417442
            //
            let slot = (if delimiters[closer_idx].open { 3 } else { 0 })
                + (delimiters[closer_idx].length % 3);
            openers_bottom
                .get_mut(&closer_marker)
                .expect("set above for this marker")[slot] = new_min_opener_idx;
        }
    }
}

/// `link_pairs(state)` (in `md.inline.ruler2` as `balance_pairs`).
pub fn link_pairs(state: &mut StateInline) {
    let max = state.tokens_meta.len();

    // processDelimiters(state.delimiters): throws (panics) when it is undefined
    process_delimiters(&mut state.current_delimiters().borrow_mut());

    for curr in 0..max {
        // tokens_meta[curr]?.delimiters
        if let Some(Some(meta)) = state.tokens_meta.get(curr)
            && let Some(delimiters) = &meta.delimiters
        {
            process_delimiters(&mut delimiters.borrow_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::markdown_it::rules_inline::state_inline::TokenMeta;
    use crate::markdown_it::{Env, MarkdownIt};

    fn d(marker: u16, length: usize, token: usize, open: bool, close: bool) -> Delimiter {
        Delimiter {
            marker,
            length,
            token,
            end: -1,
            open,
            close,
        }
    }

    fn show(list: &[Delimiter]) -> String {
        list.iter()
            .map(|d| format!("{},{},{}", d.end, d.open, d.close))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn pairs_match_balance_pairs_ts() {
        // node (tests/markdown_it/inline_rule_values.cjs, `balance_pairs`): s.delimiters = the
        // list, s.tokens_meta = [undefined, { delimiters: a copy }]; link_pairs(s); then
        // end,open,close of each delimiter (both lists come out the same)
        let cases: [(&str, Vec<Delimiter>, &str); 7] = [
            (
                "em",
                vec![d(42, 1, 0, true, false), d(42, 1, 2, false, true)],
                "1,true,false -1,false,true",
            ),
            (
                "strong_em",
                vec![
                    d(42, 3, 0, true, false),
                    d(42, 3, 1, true, false),
                    d(42, 3, 2, true, false),
                    d(42, 3, 4, false, true),
                    d(42, 3, 5, false, true),
                    d(42, 3, 6, false, true),
                ],
                "5,true,false 4,true,false 3,true,false -1,false,true -1,false,true -1,false,true",
            ),
            (
                "rule_of_3",
                vec![
                    d(42, 1, 0, true, false),
                    d(42, 2, 2, true, true),
                    d(42, 2, 3, true, true),
                    d(42, 1, 5, false, true),
                ],
                "3,true,false -1,true,true -1,true,true -1,false,true",
            ),
            (
                "mixed",
                vec![
                    d(95, 1, 0, true, false),
                    d(42, 1, 1, true, false),
                    d(95, 1, 3, false, true),
                    d(42, 1, 4, false, true),
                ],
                "2,true,false -1,true,false -1,false,true -1,false,true",
            ),
            (
                "strike",
                vec![
                    d(126, 0, 1, true, false),
                    d(126, 0, 3, false, true),
                    d(126, 0, 5, true, true),
                ],
                "1,true,false -1,false,true -1,true,true",
            ),
            (
                "unmatched",
                vec![
                    d(42, 1, 0, false, true),
                    d(42, 1, 2, true, false),
                    d(42, 2, 4, true, true),
                ],
                "-1,false,true -1,true,false -1,true,true",
            ),
            (
                "jumps",
                vec![
                    d(42, 1, 0, true, true),
                    d(95, 1, 2, true, true),
                    d(42, 1, 4, true, true),
                    d(95, 1, 6, true, true),
                    d(42, 1, 8, true, true),
                    d(95, 1, 10, true, true),
                ],
                "2,true,false -1,true,true -1,false,true 5,true,false -1,true,true -1,false,true",
            ),
        ];
        let md = MarkdownIt::default();
        for (name, list, want) in cases {
            let mut env = Env::default();
            let mut st = StateInline::new(Vec::new(), &md, &mut env, Vec::new());
            let top = Rc::new(RefCell::new(list.clone()));
            let meta = Rc::new(RefCell::new(list));
            st.delimiters = Some(Rc::clone(&top));
            st.tokens_meta = vec![
                None,
                Some(TokenMeta {
                    delimiters: Some(Rc::clone(&meta)),
                }),
            ];
            link_pairs(&mut st);
            assert_eq!(show(&top.borrow()), want, "{name}");
            assert_eq!(show(&meta.borrow()), want, "{name} (tokens_meta)");
        }
    }

    #[test]
    fn undefined_delimiters_throw_like_js() {
        // node: link_pairs on a state whose delimiters are undefined throws this TypeError
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(Vec::new(), &md, &mut env, Vec::new());
        st.delimiters = None;
        link_pairs(&mut st);
        assert_eq!(
            env.thrown,
            Some(crate::markdown_it::JsError::type_error(
                "Cannot read properties of undefined (reading 'length')"
            ))
        );
    }
}
