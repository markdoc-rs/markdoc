//! Port of upstream Markdoc v0.5.11's `src/tokenizer/plugins/frontmatter.ts`.
//!
//! Owner: the plugins agent.
//!
//! A block rule (before `hr`): when line 0 of the block being tokenized is `---` (trimmed)
//! and a later line is too, the lines between are one hidden `frontmatter` token whose
//! content is the text between them, trimmed. The rule tests `startLine != 0`, not the
//! nesting, so a container that starts on line 0 (a blockquote) can hold one too, as
//! upstream's can.

use crate::markdown_it::{MarkdownIt, RulerError, StateBlock, js};

/// `fence = '---'`
const FENCE: &[u16] = &[b'-' as u16, b'-' as u16, b'-' as u16];

/// `getLine(state, n)`: line `n`, trimmed.
fn get_line<'s>(state: &'s StateBlock, n: usize) -> &'s [u16] {
    js::trim(js::slice_to(&state.src, state.b_marks[n], state.e_marks[n]))
}

/// `findClose(state, endLine)`: the first line after line 0 that is `---`.
fn find_close(state: &StateBlock, end_line: usize) -> Option<usize> {
    (1..end_line).find(|&line| get_line(state, line) == FENCE)
}

fn block(state: &mut StateBlock, start_line: usize, end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    if start_line != 0 || get_line(state, 0) != FENCE {
        return false;
    }

    // `if (!close)`: undefined (a found line is never 0)
    let Some(close) = find_close(state, end_line) else {
        return false;
    };
    if silent {
        return true;
    }

    let content = js::to_string(js::trim(js::slice_to(
        &state.src,
        state.e_marks[0],
        state.b_marks[close],
    )));
    let token = state.push("frontmatter", "", 0);
    token.content = content;
    token.map = Some([0, close]);
    token.hidden = true;

    state.line = close + 1;
    true
}

/// `plugin(md)`: errors where upstream throws (a markdown-it without an `hr` rule).
pub fn plugin(md: &mut MarkdownIt) -> Result<(), RulerError> {
    md.block.ruler.before("hr", "frontmatter", block, &[])
}
