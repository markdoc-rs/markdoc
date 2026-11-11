//! Port of rules_block/fence.ts: fences (``` lang, ~~~ lang).

use super::state_block::StateBlock;
use crate::markdown_it::js;

pub fn fence(state: &mut StateBlock, start_line: usize, end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    let mut pos = state.b_marks[start_line] + state.t_shift[start_line];
    let mut max = state.e_marks[start_line];

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    if pos + 3 > max {
        return false;
    }

    let marker = state.src[pos];

    // ~ `
    if marker != 0x7E && marker != 0x60 {
        return false;
    }

    // scan marker length
    let mut mem = pos;
    pos = state.skip_chars(pos, marker);

    // `len` is reused below for an indent (`sCount`), hence i64
    let mut len = (pos - mem) as i64;

    if len < 3 {
        return false;
    }

    let markup = state.src[mem..pos].to_vec();
    let params = js::slice_to(&state.src, pos, max).to_vec();

    // `
    if marker == 0x60 {
        if js::index_of(&params, &js::from_char_code(&[u32::from(marker)]), 0).is_some() {
            return false;
        }
    }

    // Since start is found, we can report success here in validation mode
    if silent {
        return true;
    }

    // search end of block
    let mut next_line = start_line;
    let mut have_end_marker = false;

    loop {
        next_line += 1;
        if next_line >= end_line {
            // unclosed block should be autoclosed by end of document.
            // also block seems to be autoclosed by end of parent
            break;
        }

        pos = state.b_marks[next_line] + state.t_shift[next_line];
        mem = pos;
        max = state.e_marks[next_line];

        if pos < max && state.s_count[next_line] < state.blk_indent {
            // non-empty line with negative indent should stop the list:
            // - ```
            //  test
            break;
        }

        // `charCodeAt` is NaN past the end of src: not the marker
        if js::char_code_at(&state.src, pos) != Some(marker) {
            continue;
        }

        if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent >= 4 {
            // closing fence should be indented less than 4 spaces
            continue;
        }

        pos = state.skip_chars(pos, marker);

        // closing code fence must be at least as long as the opening one
        if ((pos - mem) as i64) < len {
            continue;
        }

        // make sure tail has spaces only
        pos = state.skip_spaces(pos);

        if pos < max {
            continue;
        }

        have_end_marker = true;
        // found!
        break;
    }

    // If a fence has heading spaces, they should be removed from its inner block
    len = state.s_count[start_line];

    state.line = next_line + if have_end_marker { 1 } else { 0 };

    // `token` borrows `state`: what JS reads from `state` after the push is read first.
    let content = state.get_lines(start_line + 1, next_line, len, true);
    let line = state.line;
    let token = state.push("fence", "code", 0);
    token.info = js::to_string(&params);
    token.content = js::to_string(&content);
    token.markup = js::to_string(&markup);
    token.map = Some([start_line, line]);

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn fence_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "```js  x\ncode\n```\n",
                "{}",
                &[],
                &["fence/code [0,3] L0 markup=\"```\" info=\"js  x\" content=\"code\\n\""],
            ),
            (
                "~~~\na\n",
                "{}",
                &[],
                &["fence/code [0,2] L0 markup=\"~~~\" content=\"a\\n\""],
            ),
            (
                "``` a`b\n```\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"``` a`b\"",
                    "paragraph_close/p L0",
                    "fence/code [1,2] L0 markup=\"```\"",
                ],
            ),
            (
                "  ```\n   a\n  b\nc\n  ```\n",
                "{}",
                &[],
                &["fence/code [0,5] L0 markup=\"```\" content=\" a\\nb\\nc\\n\""],
            ),
            (
                "````\na\n```\n`````  \n",
                "{}",
                &[],
                &["fence/code [0,4] L0 markup=\"````\" content=\"a\\n```\\n\""],
            ),
            (
                "- ```\n  a\n b\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,2] L0 markup=\"-\"",
                    "list_item_open/li [0,2] L1 markup=\"-\"",
                    "fence/code [0,2] L2 markup=\"```\" content=\"a\\n\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                    "paragraph_open/p [2,3] L0",
                    "inline/ [2,3] L1 content=\"b\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "    ```\n    a\n    ```\n",
                "{\"allowIndentation\":true}",
                &[],
                &["code_block/code [0,3] L0 content=\"```\\na\\n```\\n\""],
            ),
            (
                "```\na\n    ```\n```\n",
                "{\"allowIndentation\":true}",
                &[],
                &[
                    "fence/code [0,3] L0 markup=\"```\" content=\"a\\n\"",
                    "fence/code [3,4] L0 markup=\"```\"",
                ],
            ),
        ];
        check(cases);
    }
}
