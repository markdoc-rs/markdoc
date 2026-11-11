//! Port of rules_inline/text.ts.
//!
//! Skip text characters for text token, place those to pending buffer
//! and increment current pos

use super::state_inline::StateInline;
use crate::markdown_it::js;

// Rule to skip pure text
// '{}$%@~+=:' reserved for extentions

// !, ", #, $, %, &, ', (, ), *, +, ,, -, ., /, :, ;, <, =, >, ?, @, [, \, ], ^, _, `, {, |, }, or ~

// !!!! Don't confuse with "Markdown ASCII Punctuation" chars
// http://spec.commonmark.org/0.15/#ascii-punctuation-character
fn is_terminator_char(ch: u16) -> bool {
    match ch {
        0x0A/* \n */ |
        0x21/* ! */ |
        0x23/* # */ |
        0x24/* $ */ |
        0x25/* % */ |
        0x26/* & */ |
        0x2A/* * */ |
        0x2B/* + */ |
        0x2D/* - */ |
        0x3A/* : */ |
        0x3C/* < */ |
        0x3D/* = */ |
        0x3E/* > */ |
        0x40/* @ */ |
        0x5B/* [ */ |
        0x5C/* \ */ |
        0x5D/* ] */ |
        0x5E/* ^ */ |
        0x5F/* _ */ |
        0x60/* ` */ |
        0x7B/* { */ |
        0x7D/* } */ |
        0x7E/* ~ */ => true,
        _ => false,
    }
}

pub fn text(state: &mut StateInline, silent: bool) -> bool {
    let mut pos = state.pos;

    // `isTerminatorChar(NaN)` is false: past the end of `src` the scan goes on to `posMax`.
    while pos < state.pos_max && !js::char_code_at(&state.src, pos).is_some_and(is_terminator_char)
    {
        pos += 1;
    }

    if pos == state.pos {
        return false;
    }

    if !silent {
        let text = js::slice_to(&state.src, state.pos, pos);
        state.pending.extend_from_slice(text);
    }

    state.pos = pos;

    true
}

// Alternative implementation, for memory.
//
// It costs 10% of performance, but allows extend terminators list, if place it
// to `ParserInline` property. Probably, will switch to it sometime, such
// flexibility required.

/*
var TERMINATOR_RE = /[\n!#$%&*+\-:<=>@[\\\]^_`{}~]/;

module.exports = function text(state, silent) {
  var pos = state.pos,
      idx = state.src.slice(pos).search(TERMINATOR_RE);

  // first char is terminator -> empty text
  if (idx === 0) { return false; }

  // no terminator -> text till end of string
  if (idx < 0) {
    if (!silent) { state.pending += state.src.slice(pos); }
    state.pos = state.src.length;
    return true;
  }

  if (!silent) { state.pending += state.src.slice(pos, pos + idx); }

  state.pos += idx;

  return true;
}; */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_it::{Env, MarkdownIt};

    #[test]
    fn terminators_are_the_ts_list() {
        // node (tests/markdown_it/inline_rule_values.cjs, `text_terminators`): the code units
        // below 0x80 at which text() returns false
        let want: Vec<u16> = [
            0x0A, 0x21, 0x23, 0x24, 0x25, 0x26, 0x2A, 0x2B, 0x2D, 0x3A, 0x3C, 0x3D, 0x3E, 0x40,
            0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x7B, 0x7D, 0x7E,
        ]
        .to_vec();
        let got: Vec<u16> = (0..0x80u16).filter(|&c| is_terminator_char(c)).collect();
        assert_eq!(got, want);
        // non-ASCII never terminates (U+FF01 FULLWIDTH EXCLAMATION MARK, a surrogate)
        assert!(!is_terminator_char(0xFF01) && !is_terminator_char(0xD83D));
    }

    #[test]
    fn text_moves_text_to_pending() {
        // node (`text_state`): s = new md.inline.State('ab\u{1F600}c*d', md, {}, []);
        // text(s, false) => true, s.pos 5, s.pending 'ab\u{1F600}c'; again => false;
        // s.pos = 6, s.pending = '', text(s, true) => true, s.pos 7, pending still ''
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("ab\u{1F600}c*d"), &md, &mut env, Vec::new());
        assert!(text(&mut st, false));
        assert_eq!(
            (st.pos, js::to_string(&st.pending)),
            (5, "ab\u{1F600}c".into())
        );
        assert!(!text(&mut st, false));
        st.pos = 6;
        st.pending.clear();
        assert!(text(&mut st, true));
        assert_eq!((st.pos, st.pending.len()), (7, 0));
    }
}
