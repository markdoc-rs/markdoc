//! Port of rules_block/blockquote.ts: block quotes.

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::js;

pub fn blockquote(
    state: &mut StateBlock,
    start_line: usize,
    end_line: usize,
    silent: bool,
) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    let mut pos = state.b_marks[start_line] + state.t_shift[start_line];
    // JS starts it at `state.eMarks[startLine]`, which nothing reads before the loop sets it
    let mut max;

    let old_line_max = state.line_max;

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    // check the block quote marker
    // `None` is JS NaN (an empty last line), not '>'
    if js::char_code_at(&state.src, pos) != Some(0x3E) {
        return false;
    }

    // we know that it's going to be a valid blockquote,
    // so no point trying to find the end of it in silent mode
    if silent {
        return true;
    }

    let mut old_b_marks: Vec<usize> = Vec::new();
    let mut old_bs_count: Vec<i64> = Vec::new();
    let mut old_s_count: Vec<i64> = Vec::new();
    let mut old_t_shift: Vec<usize> = Vec::new();

    let terminator_rules = state.md.block.ruler.get_rules("blockquote");

    let old_parent_type = state.parent_type;
    state.parent_type = "blockquote";
    let mut last_line_empty = false;
    let mut next_line;

    // Search the end of the block
    //
    // Block ends with either:
    //  1. an empty line outside:
    //     ```
    //     > test
    //
    //     ```
    //  2. an empty line inside:
    //     ```
    //     >
    //     test
    //     ```
    //  3. another tag:
    //     ```
    //     > test
    //      - - -
    //     ```
    // for (nextLine = startLine; nextLine < endLine; nextLine++)
    next_line = start_line;
    while next_line < end_line {
        // check if it's outdented, i.e. it's inside list item and indented
        // less than said list item:
        //
        // ```
        // 1. anything
        //    > current blockquote
        // 2. checking this line
        // ```
        let is_outdented = state.s_count[next_line] < state.blk_indent;

        pos = state.b_marks[next_line] + state.t_shift[next_line];
        max = state.e_marks[next_line];

        if pos >= max {
            // Case 1: line is not inside the blockquote, and this line is empty.
            break;
        }

        // state.src.charCodeAt(pos++): pos < max here
        let ch = state.src[pos];
        pos += 1;
        if ch == 0x3E /* > */ && !is_outdented {
            // This line is inside the blockquote.

            // set offset past spaces and ">"
            let mut initial = state.s_count[next_line] + 1;
            let space_after_marker;
            let adjust_tab;

            // skip one optional space after '>'
            if js::char_code_at(&state.src, pos) == Some(0x20) {
                // ' >   test '
                //     ^ -- position start of line here:
                pos += 1;
                initial += 1;
                adjust_tab = false;
                space_after_marker = true;
            } else if js::char_code_at(&state.src, pos) == Some(0x09) {
                space_after_marker = true;

                if (state.bs_count[next_line] + initial) % 4 == 3 {
                    // '  >\t  test '
                    //       ^ -- position start of line here (tab has width===1)
                    pos += 1;
                    initial += 1;
                    adjust_tab = false;
                } else {
                    // ' >\t  test '
                    //    ^ -- position start of line here + shift bsCount slightly
                    //         to make extra space appear
                    adjust_tab = true;
                }
            } else {
                space_after_marker = false;
                // JS leaves `adjustTab` undefined here: falsy
                adjust_tab = false;
            }

            let mut offset = initial;
            old_b_marks.push(state.b_marks[next_line]);
            state.b_marks[next_line] = pos;

            while pos < max {
                let ch = state.src[pos];

                if is_space(ch) {
                    if ch == 0x09 {
                        offset += 4
                            - (offset + state.bs_count[next_line] + if adjust_tab { 1 } else { 0 })
                                % 4;
                    } else {
                        offset += 1;
                    }
                } else {
                    break;
                }

                pos += 1;
            }

            last_line_empty = pos >= max;

            old_bs_count.push(state.bs_count[next_line]);
            state.bs_count[next_line] =
                state.s_count[next_line] + 1 + if space_after_marker { 1 } else { 0 };

            old_s_count.push(state.s_count[next_line]);
            state.s_count[next_line] = offset - initial;

            old_t_shift.push(state.t_shift[next_line]);
            state.t_shift[next_line] = pos - state.b_marks[next_line];
            next_line += 1;
            continue;
        }

        // Case 2: line is not inside the blockquote, and the last line was empty.
        if last_line_empty {
            break;
        }

        // Case 3: another tag found.
        let mut terminate = false;
        for &rule in terminator_rules {
            if rule(state, next_line, end_line, true) {
                terminate = true;
                break;
            }
        }

        if terminate {
            // Quirk to enforce "hard termination mode" for paragraphs;
            // normally if you call `tokenize(state, startLine, nextLine)`,
            // paragraphs will look below nextLine for paragraph continuation,
            // but if blockquote is terminated by another tag, they shouldn't
            state.line_max = next_line;

            if state.blk_indent != 0 {
                // state.blkIndent was non-zero, we now set it to zero,
                // so we need to re-calculate all offsets to appear as
                // if indent wasn't changed
                old_b_marks.push(state.b_marks[next_line]);
                old_bs_count.push(state.bs_count[next_line]);
                old_t_shift.push(state.t_shift[next_line]);
                old_s_count.push(state.s_count[next_line]);
                state.s_count[next_line] -= state.blk_indent;
            }

            break;
        }

        old_b_marks.push(state.b_marks[next_line]);
        old_bs_count.push(state.bs_count[next_line]);
        old_t_shift.push(state.t_shift[next_line]);
        old_s_count.push(state.s_count[next_line]);

        // A negative indentation means that this is a paragraph continuation
        //
        state.s_count[next_line] = -1;
        next_line += 1;
    }

    let old_indent = state.blk_indent;
    state.blk_indent = 0;

    let token_o = state.push("blockquote_open", "blockquote", 1);
    token_o.markup = ">".to_owned();
    // `const lines = [startLine, 0]; token_o.map = lines` and later `lines[1] = state.line`:
    // the array is shared, so the map is completed in place after the subcall (only tokens
    // after this one are pushed meanwhile, so its index stays valid).
    let lines = state.tokens.len() - 1;
    state.tokens[lines].map = Some([start_line, 0]);

    let md = state.md;
    // Not JS's: a level of node's stack (see `cost`); past it the RangeError is thrown.
    if let Some(_level) = state.env.level(crate::markdown_it::cost::BLOCKQUOTE) {
        crate::depth::grow!(md.block.tokenize(state, start_line, next_line));
    }

    let token_c = state.push("blockquote_close", "blockquote", -1);
    token_c.markup = ">".to_owned();

    state.line_max = old_line_max;
    state.parent_type = old_parent_type;
    let line = state.line;
    if let Some(map) = state.tokens[lines].map.as_mut() {
        map[1] = line;
    }

    // Restore original tShift; this might not be necessary since the parser
    // has already been here, but just to make sure we can do that.
    #[allow(clippy::manual_memcpy)] // the loop of blockquote.ts, kept as it is
    for i in 0..old_t_shift.len() {
        state.b_marks[i + start_line] = old_b_marks[i];
        state.t_shift[i + start_line] = old_t_shift[i];
        state.s_count[i + start_line] = old_s_count[i];
        state.bs_count[i + start_line] = old_bs_count[i];
    }
    state.blk_indent = old_indent;

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn blockquote_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "> a\nb\n> c\n\nd\n",
                "{}",
                &[],
                &[
                    "blockquote_open/blockquote [0,3] L0 markup=\">\"",
                    "paragraph_open/p [0,3] L1",
                    "inline/ [0,3] L2 content=\"a\\nb\\nc\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                    "paragraph_open/p [4,5] L0",
                    "inline/ [4,5] L1 content=\"d\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "> a\n---\n",
                "{}",
                &[],
                &[
                    "blockquote_open/blockquote [0,1] L0 markup=\">\"",
                    "paragraph_open/p [0,1] L1",
                    "inline/ [0,1] L2 content=\"a\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                    "hr/hr [1,2] L0 markup=\"---\"",
                ],
            ),
            (
                ">\tfoo\n>\t\tbar\n  >\t\tbaz\n",
                "{}",
                &[],
                &[
                    "blockquote_open/blockquote [0,3] L0 markup=\">\"",
                    "paragraph_open/p [0,3] L1",
                    "inline/ [0,3] L2 content=\"foo\\n\\t\\tbar\\n\\tbaz\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                ],
            ),
            (
                "- > a\n  > b\n> c\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,2] L0 markup=\"-\"",
                    "list_item_open/li [0,2] L1 markup=\"-\"",
                    "blockquote_open/blockquote [0,2] L2 markup=\">\"",
                    "paragraph_open/p [0,2] L3",
                    "inline/ [0,2] L4 content=\"a\\nb\"",
                    "paragraph_close/p L3",
                    "blockquote_close/blockquote L2 markup=\">\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                    "blockquote_open/blockquote [2,3] L0 markup=\">\"",
                    "paragraph_open/p [2,3] L1",
                    "inline/ [2,3] L2 content=\"c\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                ],
            ),
            (
                "> > a\n> b\nc\n",
                "{}",
                &[],
                &[
                    "blockquote_open/blockquote [0,3] L0 markup=\">\"",
                    "blockquote_open/blockquote [0,3] L1 markup=\">\"",
                    "paragraph_open/p [0,3] L2",
                    "inline/ [0,3] L3 content=\"a\\nb\\nc\"",
                    "paragraph_close/p L2",
                    "blockquote_close/blockquote L1 markup=\">\"",
                    "blockquote_close/blockquote L0 markup=\">\"",
                ],
            ),
            (
                ">\n> a\n>\nb\n",
                "{}",
                &[],
                &[
                    "blockquote_open/blockquote [0,3] L0 markup=\">\"",
                    "paragraph_open/p [1,2] L1",
                    "inline/ [1,2] L2 content=\"a\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                    "paragraph_open/p [3,4] L0",
                    "inline/ [3,4] L1 content=\"b\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "- a\n  > b\n- c\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,2] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "blockquote_open/blockquote [1,2] L2 markup=\">\"",
                    "paragraph_open/p [1,2] L3",
                    "inline/ [1,2] L4 content=\"b\"",
                    "paragraph_close/p L3",
                    "blockquote_close/blockquote L2 markup=\">\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [2,3] L1 markup=\"-\"",
                    "paragraph_open/p [2,3] L2 hidden",
                    "inline/ [2,3] L3 content=\"c\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "    > a\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &[
                    "blockquote_open/blockquote [0,1] L0 markup=\">\"",
                    "paragraph_open/p [0,1] L1",
                    "inline/ [0,1] L2 content=\"a\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                ],
            ),
        ];
        check(cases);
    }
}
