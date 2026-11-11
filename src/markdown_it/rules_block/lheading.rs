//! Port of rules_block/lheading.ts: lheading (---, ===).
//!
//! Markdoc's allowIndentation patch leaves this rule alone (Markdoc disables it instead).

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::ascii_trim;
use crate::markdown_it::js;

/// `lheading(state, startLine, endLine)`. JS ignores `silent` (the rule is in no alt chain);
/// so does this.
pub fn lheading(state: &mut StateBlock, start_line: usize, end_line: usize, _silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    let terminator_rules = state.md.block.ruler.get_rules("paragraph");

    // if it's indented more than 3 spaces, it should be a code block
    if state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    let old_parent_type = state.parent_type;
    state.parent_type = "paragraph"; // use paragraph to match terminatorRules

    // jump line-by-line until empty one or EOF
    let mut level = 0;
    // JS `let marker` (undefined until a candidate underline is seen; read only once `level`
    // is set, when it holds that underline's character)
    let mut marker: u16 = 0;
    let mut next_line = start_line + 1;

    // for (; nextLine < endLine && !state.isEmpty(nextLine); nextLine++)
    while next_line < end_line && !state.is_empty(next_line) {
        // this would be a code block normally, but after paragraph
        // it's considered a lazy continuation regardless of what's there
        if state.s_count[next_line] - state.blk_indent > 3 {
            next_line += 1;
            continue;
        }

        //
        // Check for underline in setext header
        //
        if state.s_count[next_line] >= state.blk_indent {
            let mut pos = state.b_marks[next_line] + state.t_shift[next_line];
            let max = state.e_marks[next_line];

            if pos < max {
                marker = state.src[pos];

                // - =
                if marker == 0x2D || marker == 0x3D {
                    pos = state.skip_chars(pos, marker);
                    pos = state.skip_spaces(pos);

                    if pos >= max {
                        level = if marker == 0x3D { 1 } else { 2 };
                        break;
                    }
                }
            }
        }

        // quirk for blockquotes, this line should already be checked by that rule
        if state.s_count[next_line] < 0 {
            next_line += 1;
            continue;
        }

        // Some tags can terminate paragraph without empty line.
        let mut terminate = false;
        for &rule in terminator_rules {
            if rule(state, next_line, end_line, true) {
                terminate = true;
                break;
            }
        }
        if terminate {
            break;
        }
        next_line += 1;
    }

    if level == 0 {
        // Didn't find valid underline
        state.parent_type = old_parent_type;
        return false;
    }

    let content = js::to_string(ascii_trim(&state.get_lines(
        start_line,
        next_line,
        state.blk_indent,
        false,
    )));

    state.line = next_line + 1;

    // `token_*` borrow `state`: what JS reads from `state` after a push is read first.
    let line = state.line;
    let tag = format!("h{level}");
    let markup = js::to_string(&[marker]);

    let token_o = state.push("heading_open", &tag, 1);
    token_o.markup = markup.clone();
    token_o.map = Some([start_line, line]);

    let token_i = state.push("inline", "", 0);
    token_i.content = content;
    token_i.map = Some([start_line, line - 1]);
    token_i.children = Some(Vec::new());

    let token_c = state.push("heading_close", &tag, -1);
    token_c.markup = markup;

    state.parent_type = old_parent_type;

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn lheading_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "a\nb\n===\n",
                "{}",
                &[],
                &[
                    "heading_open/h1 [0,3] L0 markup=\"=\"",
                    "inline/ [0,2] L1 content=\"a\\nb\"",
                    "heading_close/h1 L0 markup=\"=\"",
                ],
            ),
            (
                "  a  \n  ---  \nc\n- - -\n",
                "{}",
                &[],
                &[
                    "heading_open/h2 [0,2] L0 markup=\"-\"",
                    "inline/ [0,1] L1 content=\"a\"",
                    "heading_close/h2 L0 markup=\"-\"",
                    "paragraph_open/p [2,3] L0",
                    "inline/ [2,3] L1 content=\"c\"",
                    "paragraph_close/p L0",
                    "hr/hr [3,4] L0 markup=\"---\"",
                ],
            ),
            (
                "a\n    ===\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"a\\n    ===\"",
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
                "a\n> b\n===\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a\"",
                    "paragraph_close/p L0",
                    "blockquote_open/blockquote [1,3] L0 markup=\">\"",
                    "paragraph_open/p [1,3] L1",
                    "inline/ [1,3] L2 content=\"b\\n===\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                ],
            ),
        ];
        check(cases);
    }
}
