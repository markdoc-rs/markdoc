//! Port of helpers/parse_link_label.ts.

use crate::markdown_it::rules_inline::state_inline::StateInline;

/// `parseLinkLabel(state, start, disableNested)`: finds the end of a link or image label
/// (`[label]`) that opens at `start`. Returns the position of the closing `]` (`None` where
/// JS returns -1) and leaves `state.pos` unchanged.
pub fn parse_link_label(
    state: &mut StateInline,
    start: usize,
    disable_nested: bool,
) -> Option<usize> {
    let mut found = false;
    let max = state.pos_max;
    let old_pos = state.pos;

    // `start + 1` (a JS number: past `usize::MAX` it is past `max` all the same)
    state.pos = start.saturating_add(1);
    let mut level = 1;

    while state.pos < max {
        let marker = state.src.get(state.pos).copied();
        if marker == Some(0x5D /* ] */) {
            level -= 1;
            if level == 0 {
                found = true;
                break;
            }
        }

        let prev_pos = state.pos;
        let md = state.md;
        md.inline.skip_token(state);
        if marker == Some(0x5B /* [ */) {
            if prev_pos == state.pos - 1 {
                // increase level if we find text `[`, which is not a part of any token
                level += 1;
            } else if disable_nested {
                state.pos = old_pos;
                return None;
            }
        }
    }

    let mut label_end = None;

    if found {
        label_end = Some(state.pos);
    }

    // restore old state
    state.pos = old_pos;

    label_end
}
