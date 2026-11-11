//! Port of rules_block/code.ts: code block (4 spaces padded).
//!
//! Markdoc's allowIndentation patch leaves this rule alone (Markdoc disables it instead).

use super::state_block::StateBlock;
use crate::markdown_it::js;

/// `code(state, startLine, endLine)`. JS ignores `silent` (the rule is in no alt chain, so
/// it is never called as a terminator); so does this.
pub fn code(state: &mut StateBlock, start_line: usize, end_line: usize, _silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    if state.s_count[start_line] - state.blk_indent < 4 {
        return false;
    }

    let mut next_line = start_line + 1;
    let mut last = next_line;

    while next_line < end_line {
        if state.is_empty(next_line) {
            next_line += 1;
            continue;
        }

        if state.s_count[next_line] - state.blk_indent >= 4 {
            next_line += 1;
            last = next_line;
            continue;
        }
        break;
    }

    state.line = last;

    // `token` borrows `state`: what JS reads from `state` after the push is read first.
    let mut content = state.get_lines(start_line, last, 4 + state.blk_indent, false);
    content.push(0x0A);
    let line = state.line;
    let token = state.push("code_block", "code", 0);
    token.content = js::to_string(&content);
    token.map = Some([start_line, line]);

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn code_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "    a\n\n    b\n  \n\nc\n",
                "{}",
                &[],
                &[
                    "code_block/code [0,3] L0 content=\"a\\n\\nb\\n\"",
                    "paragraph_open/p [5,6] L0",
                    "inline/ [5,6] L1 content=\"c\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "\t\tfoo\n",
                "{}",
                &[],
                &["code_block/code [0,1] L0 content=\"\\tfoo\\n\""],
            ),
            (
                "- x\n\n        code\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,3] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2",
                    "inline/ [0,1] L3 content=\"x\"",
                    "paragraph_close/p L2",
                    "code_block/code [2,3] L2 content=\"  code\\n\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "    a",
                "{}",
                &["code"],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a\"",
                    "paragraph_close/p L0",
                ],
            ),
        ];
        check(cases);
    }
}
