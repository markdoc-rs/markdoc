//! Port of rules_block/heading.ts: heading (#, ##, ...).

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::{ascii_trim, is_space};
use crate::markdown_it::js;

pub fn heading(state: &mut StateBlock, start_line: usize, _end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let mut pos = state.b_marks[start_line] + state.t_shift[start_line];
    let mut max = state.e_marks[start_line];

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    // `None` is JS NaN (past the end of src)
    let mut ch = js::char_code_at(&state.src, pos);

    // #
    if ch != Some(0x23) || pos >= max {
        return false;
    }

    // count heading level
    let mut level = 1;
    pos += 1;
    ch = js::char_code_at(&state.src, pos);
    while ch == Some(0x23) && pos < max && level <= 6 {
        level += 1;
        pos += 1;
        ch = js::char_code_at(&state.src, pos);
    }

    if level > 6 || (pos < max && !ch.is_some_and(is_space)) {
        return false;
    }

    if silent {
        return true;
    }

    // Let's cut tails like '    ###  ' from the end of string

    max = state.skip_spaces_back(max, pos);
    let tmp = state.skip_chars_back(max, 0x23, pos); // #
    if tmp > pos && js::char_code_at(&state.src, tmp - 1).is_some_and(is_space) {
        max = tmp;
    }

    state.line = start_line + 1;

    // `token_*` borrow `state`: what JS reads from `state` after a push is read first.
    let line = state.line;
    let tag = format!("h{level}");
    // '########'.slice(0, level)
    let markup = &"########"[..level];
    let content = js::to_string(ascii_trim(js::slice_to(&state.src, pos, max)));

    let token_o = state.push("heading_open", &tag, 1);
    token_o.markup = markup.to_owned();
    token_o.map = Some([start_line, line]);

    let token_i = state.push("inline", "", 0);
    token_i.content = content;
    token_i.map = Some([start_line, line]);
    token_i.children = Some(Vec::new());

    let token_c = state.push("heading_close", &tag, -1);
    token_c.markup = markup.to_owned();

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn heading_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "# foo ##  \n#5 bolt\n####### a\n# foo#\n###\n## \\##\n",
                "{}",
                &[],
                &[
                    "heading_open/h1 [0,1] L0 markup=\"#\"",
                    "inline/ [0,1] L1 content=\"foo\"",
                    "heading_close/h1 L0 markup=\"#\"",
                    "paragraph_open/p [1,3] L0",
                    "inline/ [1,3] L1 content=\"#5 bolt\\n####### a\"",
                    "paragraph_close/p L0",
                    "heading_open/h1 [3,4] L0 markup=\"#\"",
                    "inline/ [3,4] L1 content=\"foo#\"",
                    "heading_close/h1 L0 markup=\"#\"",
                    "heading_open/h3 [4,5] L0 markup=\"###\"",
                    "inline/ [4,5] L1",
                    "heading_close/h3 L0 markup=\"###\"",
                    "heading_open/h2 [5,6] L0 markup=\"##\"",
                    "inline/ [5,6] L1 content=\"\\\\##\"",
                    "heading_close/h2 L0 markup=\"##\"",
                ],
            ),
            (
                "#\tfoo\t#\t\n# \u{a0}foo\u{a0} #\n",
                "{}",
                &[],
                &[
                    "heading_open/h1 [0,1] L0 markup=\"#\"",
                    "inline/ [0,1] L1 content=\"foo\"",
                    "heading_close/h1 L0 markup=\"#\"",
                    "heading_open/h1 [1,2] L0 markup=\"#\"",
                    "inline/ [1,2] L1 content=\"\u{a0}foo\u{a0}\"",
                    "heading_close/h1 L0 markup=\"#\"",
                ],
            ),
            (
                "a\n# b\n    # c\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a\"",
                    "paragraph_close/p L0",
                    "heading_open/h1 [1,2] L0 markup=\"#\"",
                    "inline/ [1,2] L1 content=\"b\"",
                    "heading_close/h1 L0 markup=\"#\"",
                    "code_block/code [2,3] L0 content=\"# c\\n\"",
                ],
            ),
            (
                "    # c\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &[
                    "heading_open/h1 [0,1] L0 markup=\"#\"",
                    "inline/ [0,1] L1 content=\"c\"",
                    "heading_close/h1 L0 markup=\"#\"",
                ],
            ),
        ];
        check(cases);
    }
}
