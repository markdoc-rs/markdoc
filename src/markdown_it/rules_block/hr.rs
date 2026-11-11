//! Port of rules_block/hr.ts: horizontal rule.

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::js;

pub fn hr(state: &mut StateBlock, start_line: usize, _end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let max = state.e_marks[start_line];
    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    let mut pos = state.b_marks[start_line] + state.t_shift[start_line];
    // `None` is JS NaN (an empty last line): none of the markers
    let marker = js::char_code_at(&state.src, pos);
    pos += 1;

    // Check hr marker: * - _
    let Some(marker @ (0x2A | 0x2D | 0x5F)) = marker else {
        return false;
    };

    // markers can be mixed with spaces, but there should be at least 3 of them

    let mut cnt = 1;
    while pos < max {
        let ch = state.src[pos];
        pos += 1;
        if ch != marker && !is_space(ch) {
            return false;
        }
        if ch == marker {
            cnt += 1;
        }
    }

    if cnt < 3 {
        return false;
    }

    if silent {
        return true;
    }

    state.line = start_line + 1;

    let line = state.line;
    let token = state.push("hr", "hr", 0);
    token.map = Some([start_line, line]);
    // Array(cnt + 1).join(String.fromCharCode(marker))
    token.markup = js::to_string(&vec![marker; cnt]);

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn hr_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "* * *\n_\t_ _ _\n- - a\n",
                "{}",
                &[],
                &[
                    "hr/hr [0,1] L0 markup=\"***\"",
                    "hr/hr [1,2] L0 markup=\"____\"",
                    "bullet_list_open/ul [2,3] L0 markup=\"-\"",
                    "list_item_open/li [2,3] L1 markup=\"-\"",
                    "bullet_list_open/ul [2,3] L2 markup=\"-\"",
                    "list_item_open/li [2,3] L3 markup=\"-\"",
                    "paragraph_open/p [2,3] L4 hidden",
                    "inline/ [2,3] L5 content=\"a\"",
                    "paragraph_close/p L4 hidden",
                    "list_item_close/li L3 markup=\"-\"",
                    "bullet_list_close/ul L2 markup=\"-\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "a\n***\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a\"",
                    "paragraph_close/p L0",
                    "hr/hr [1,2] L0 markup=\"***\"",
                ],
            ),
            (
                "    ---\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &["hr/hr [0,1] L0 markup=\"---\""],
            ),
        ];
        check(cases);
    }
}
