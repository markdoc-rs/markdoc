//! Port of helpers/parse_link_title.ts.

use crate::markdown_it::common::utils::unescape_all;
use crate::markdown_it::js;

/// What `parseLinkTitle` returns (`ParseLinkTitleResult`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseLinkTitleResult {
    /// if `true`, this is a valid link title
    pub ok: bool,
    /// if `true`, this link can be continued on the next line
    pub can_continue: bool,
    /// if `ok`, it's the position of the first character after the closing marker
    pub pos: usize,
    /// if `ok`, it's the unescaped title (a JS string: UTF-16 code units)
    pub str: Vec<u16>,
    /// expected closing marker character code
    pub marker: u16,
}

/// `parseLinkTitle(str, start, max, prev_state)`: parses the optional title in
/// `[label](destination "title")` or `[label]: destination "title"`.
///
/// `prev_state` continues a reference title on the next source line.
pub fn parse_link_title(
    str: &[u16],
    start: usize,
    max: usize,
    prev_state: Option<&ParseLinkTitleResult>,
) -> ParseLinkTitleResult {
    let mut pos = start;
    let mut start = start;

    let mut state = ParseLinkTitleResult {
        ok: false,
        can_continue: false,
        pos: 0,
        str: Vec::new(),
        marker: 0,
    };

    if let Some(prev_state) = prev_state {
        // this is a continuation of a previous parseLinkTitle call on the next line,
        // used in reference links only
        state.str = prev_state.str.clone();
        state.marker = prev_state.marker;
    } else {
        if pos >= max {
            return state;
        }

        let Some(mut marker) = js::char_code_at(str, pos) else {
            return state; // NaN is none of the markers
        };
        // " ' (
        if marker != 0x22 && marker != 0x27 && marker != 0x28 {
            return state;
        }

        start += 1;
        pos += 1;

        // if opening marker is "(", switch it to closing marker ")"
        if marker == 0x28 {
            marker = 0x29;
        }

        state.marker = marker;
    }

    while pos < max {
        let code = js::char_code_at(str, pos);
        if code.is_none() {
            // NaN from here to `max`: no marker, the loop steps on to `max` (taken in one
            // step: `max` can be far)
            pos = max;
            break;
        }
        if code == Some(state.marker) {
            state.pos = pos + 1;
            state
                .str
                .extend(unescape_all(js::slice_to(str, start, pos)));
            state.ok = true;
            return state;
        } else if code == Some(0x28 /* ( */) && state.marker == 0x29
        /* ) */
        {
            return state;
        } else if code == Some(0x5C /* \ */) && pos + 1 < max {
            pos += 1;
        }

        pos += 1;
    }

    // no closing marker found, but this link title may continue on the next line (for references)
    state.can_continue = true;
    state
        .str
        .extend(unescape_all(js::slice_to(str, start, pos)));
    state
}
