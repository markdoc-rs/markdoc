//! Port of helpers/parse_link_destination.ts.

use crate::markdown_it::common::utils::unescape_all;
use crate::markdown_it::js;

/// What `parseLinkDestination` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseLinkDestinationResult {
    pub ok: bool,
    pub pos: usize,
    /// The unescaped destination (a JS string: UTF-16 code units).
    pub str: Vec<u16>,
}

/// `parseLinkDestination(str, start, max)`: parses the destination in
/// `[label](destination "title")`.
pub fn parse_link_destination(str: &[u16], start: usize, max: usize) -> ParseLinkDestinationResult {
    let mut pos = start;

    let mut result = ParseLinkDestinationResult {
        ok: false,
        pos: 0,
        str: Vec::new(),
    };

    // `charCodeAt` past the end is NaN: no comparison below holds for `None`.
    if js::char_code_at(str, pos) == Some(0x3C /* < */) {
        pos += 1;
        while pos < max {
            let code = js::char_code_at(str, pos);
            if code.is_none() {
                // NaN from here to `max`: nothing below holds, the loop runs out
                break;
            }
            if code == Some(0x0A /* \n */) {
                return result;
            }
            if code == Some(0x3C /* < */) {
                return result;
            }
            if code == Some(0x3E /* > */) {
                result.pos = pos + 1;
                result.str = unescape_all(js::slice_to(str, start + 1, pos));
                result.ok = true;
                return result;
            }
            if code == Some(0x5C /* \ */) && pos + 1 < max {
                pos += 2;
                continue;
            }

            pos += 1;
        }

        // no closing '>'
        return result;
    }

    // this should be ... } else { ... branch

    let mut level = 0;
    while pos < max {
        let Some(code) = js::char_code_at(str, pos) else {
            // NaN from here to `max`: not a space, not a control character, not an escape or
            // a paren, so the loop steps on to `max` (taken in one step: `max` can be far)
            pos = max;
            break;
        };

        if code == 0x20 {
            break;
        }

        // ascii control characters
        if code < 0x20 || code == 0x7F {
            break;
        }

        if code == 0x5C /* \ */ && pos + 1 < max {
            if js::char_code_at(str, pos + 1) == Some(0x20) {
                pos += 1;
                continue;
            }
            pos += 2;
            continue;
        }

        if code == 0x28
        /* ( */
        {
            level += 1;
            if level > 32 {
                return result;
            }
        }

        if code == 0x29
        /* ) */
        {
            if level == 0 {
                break;
            }
            level -= 1;
        }

        pos += 1;
    }

    if start == pos {
        return result;
    }
    if level != 0 {
        return result;
    }

    result.str = unescape_all(js::slice_to(str, start, pos));
    result.pos = pos;
    result.ok = true;
    result
}
