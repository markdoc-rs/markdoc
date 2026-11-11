//! Port of upstream Markdoc v0.5.11's `src/tokenizer/plugins/comments.ts`.
//!
//! Owner: the plugins agent.
//!
//! `<!-- ... -->` as a `comment` token: a block rule (before `table`, interrupting
//! paragraphs) for a comment that starts a line, which consumes every line up to the one with
//! `-->` (text after it on that line is dropped, as upstream drops it), and an inline rule
//! (after every other inline rule) for a comment anywhere in inline content.

use crate::markdown_it::{MarkdownIt, RulerError, StateBlock, StateInline, js};

use super::super::{TokenizeError, throw};

/// `OPEN = '<!--'`
const OPEN: &[u16] = &[b'<' as u16, b'!' as u16, b'-' as u16, b'-' as u16];
/// `CLOSE = '-->'`
const CLOSE: &[u16] = &[b'-' as u16, b'-' as u16, b'>' as u16];

fn block(state: &mut StateBlock, start_line: usize, _end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let start = state.b_marks[start_line] + state.t_shift[start_line];
    if !js::starts_with(&state.src, OPEN, start) {
        return false;
    }

    // From `start`, so in `<!-->` the `-->` overlaps the `<!--` and the content is ''.
    let Some(close) = js::index_of(&state.src, CLOSE, start) else {
        return false;
    };
    if silent {
        return true;
    }

    let content = js::slice_to(&state.src, start + OPEN.len(), close);
    // `content.split('\n').length`
    let lines = 1 + content.iter().filter(|&&c| c == b'\n' as u16).count();
    let content = js::to_string(js::trim(content));
    let token = state.push("comment", "", 0);
    token.content = content;
    token.map = Some([start_line, start_line + lines]);
    state.line += lines;

    true
}

fn inline(state: &mut StateInline, silent: bool) -> bool {
    if !js::starts_with(&state.src, OPEN, state.pos) {
        return false;
    }

    let Some(close) = js::index_of(&state.src, CLOSE, state.pos) else {
        return false;
    };
    if silent {
        // Upstream returns true here without moving `state.pos`. Only `skipToken` (scanning a
        // link label) runs inline rules silently, and it then throws `Error: inline rule
        // didn't increment state.pos`, which escapes `tokenize`: `[<!-- c -->](u)` throws.
        // The port records that throw for `tokenize` to return, and returns false so the
        // engine goes on without panicking (what it goes on to produce is discarded).
        throw(
            state.env,
            TokenizeError {
                name: "Error".to_owned(),
                message: "inline rule didn't increment state.pos".to_owned(),
            },
        );
        return false;
    }

    let content = js::to_string(js::trim(js::slice_to(
        &state.src,
        state.pos + OPEN.len(),
        close,
    )));
    let token = state.push("comment", "", 0);
    token.content = content;
    state.pos = close + CLOSE.len();

    true
}

/// `plugin(md)`: errors where upstream throws (a markdown-it without a `table` rule).
pub fn plugin(md: &mut MarkdownIt) -> Result<(), RulerError> {
    md.block
        .ruler
        .before("table", "comment", block, &["paragraph"])?;
    md.inline.ruler.push("comment", inline, &[]);
    Ok(())
}
