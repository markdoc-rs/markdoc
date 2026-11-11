//! Port of rules_block/reference.ts: link reference definitions.

use std::collections::HashMap;

use super::state_block::StateBlock;
use crate::markdown_it::Reference;
use crate::markdown_it::common::utils::{is_space, normalize_reference};
use crate::markdown_it::helpers::{parse_link_destination, parse_link_title};
use crate::markdown_it::js;

/// The `getNextLine(nextLine)` closure of `reference`: the next source line (with its
/// newline) when it continues the definition, `None` (JS `null`) otherwise.
fn get_next_line(state: &mut StateBlock, next_line: usize) -> Option<Vec<u16>> {
    let end_line = state.line_max;

    if next_line >= end_line || state.is_empty(next_line) {
        // empty line or end of input
        return None;
    }

    let mut is_continuation = false;

    // this would be a code block normally, but after paragraph
    // it's considered a lazy continuation regardless of what's there
    if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent > 3 {
        is_continuation = true;
    }

    // quirk for blockquotes, this line should already be checked by that rule
    if state.s_count[next_line] < 0 {
        is_continuation = true;
    }

    if !is_continuation {
        let terminator_rules = state.md.block.ruler.get_rules("reference");
        let old_parent_type = state.parent_type;
        state.parent_type = "reference";

        // Some tags can terminate paragraph without empty line.
        let mut terminate = false;
        for &rule in terminator_rules {
            if rule(state, next_line, end_line, true) {
                terminate = true;
                break;
            }
        }

        state.parent_type = old_parent_type;
        if terminate {
            // terminated by another block
            return None;
        }
    }

    let pos = state.b_marks[next_line] + state.t_shift[next_line];
    let max = state.e_marks[next_line];

    // max + 1 explicitly includes the newline
    Some(js::slice_to(&state.src, pos, max + 1).to_vec())
}

pub fn reference(
    state: &mut StateBlock,
    start_line: usize,
    _end_line: usize,
    silent: bool,
) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let mut pos = state.b_marks[start_line] + state.t_shift[start_line];
    let mut max = state.e_marks[start_line];
    let mut next_line = start_line + 1;

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    // [
    // `None` is JS NaN (an empty last line), not '['
    if js::char_code_at(&state.src, pos) != Some(0x5B) {
        return false;
    }

    let mut str: Vec<u16> = js::slice_to(&state.src, pos, max + 1).to_vec();

    max = str.len();
    // JS -1 is `None`
    let mut label_end: Option<usize> = None;

    // for (pos = 1; pos < max; pos++)
    pos = 1;
    while pos < max {
        let ch = str[pos];
        if ch == 0x5B {
            // [
            return false;
        } else if ch == 0x5D {
            // ]
            label_end = Some(pos);
            break;
        } else if ch == 0x0A {
            // \n
            if let Some(line_content) = get_next_line(state, next_line) {
                str.extend_from_slice(&line_content);
                max = str.len();
                next_line += 1;
            }
        } else if ch == 0x5C {
            // \
            pos += 1;
            if pos < max && str[pos] == 0x0A {
                if let Some(line_content) = get_next_line(state, next_line) {
                    str.extend_from_slice(&line_content);
                    max = str.len();
                    next_line += 1;
                }
            }
        }
        pos += 1;
    }

    // :
    let Some(label_end) = label_end else {
        return false;
    };
    if js::char_code_at(&str, label_end + 1) != Some(0x3A) {
        return false;
    }

    // [label]:   destination   'title'
    //         ^^^ skip optional whitespace here
    // for (pos = labelEnd + 2; pos < max; pos++)
    pos = label_end + 2;
    while pos < max {
        let ch = str[pos];
        if ch == 0x0A {
            if let Some(line_content) = get_next_line(state, next_line) {
                str.extend_from_slice(&line_content);
                max = str.len();
                next_line += 1;
            }
        } else if is_space(ch) {
            /* eslint no-empty:0 */
        } else {
            break;
        }
        pos += 1;
    }

    // [label]:   destination   'title'
    //            ^^^^^^^^^^^ parse this
    let dest_res = parse_link_destination(&str, pos, max);
    if !dest_res.ok {
        return false;
    }

    let href = state.md.normalize_link(&dest_res.str);
    if !state.md.validate_link(&href) {
        return false;
    }

    pos = dest_res.pos;

    // save cursor state, we could require to rollback later
    let dest_end_pos = pos;
    let dest_end_line_no = next_line;

    // [label]:   destination   'title'
    //                       ^^^ skipping those spaces
    let start = pos;
    // for (; pos < max; pos++)
    while pos < max {
        let ch = str[pos];
        if ch == 0x0A {
            if let Some(line_content) = get_next_line(state, next_line) {
                str.extend_from_slice(&line_content);
                max = str.len();
                next_line += 1;
            }
        } else if is_space(ch) {
            /* Nothing */
        } else {
            break;
        }
        pos += 1;
    }

    // [label]:   destination   'title'
    //                          ^^^^^^^ parse this
    let mut title_res = parse_link_title(&str, pos, max, None);
    while title_res.can_continue {
        let Some(line_content) = get_next_line(state, next_line) else {
            break;
        };
        str.extend_from_slice(&line_content);
        pos = max;
        max = str.len();
        next_line += 1;
        title_res = parse_link_title(&str, pos, max, Some(&title_res));
    }
    let mut title: Vec<u16>;

    if pos < max && start != pos && title_res.ok {
        title = title_res.str;
        pos = title_res.pos;
    } else {
        title = Vec::new();
        pos = dest_end_pos;
        next_line = dest_end_line_no;
    }

    // skip trailing spaces until the rest of the line
    while pos < max {
        let ch = str[pos];
        if !is_space(ch) {
            break;
        }
        pos += 1;
    }

    if pos < max && str[pos] != 0x0A {
        if !title.is_empty() {
            // garbage at the end of the line after title,
            // but it could still be a valid reference if we roll back
            title = Vec::new();
            pos = dest_end_pos;
            next_line = dest_end_line_no;
            while pos < max {
                let ch = str[pos];
                if !is_space(ch) {
                    break;
                }
                pos += 1;
            }
        }
    }

    if pos < max && str[pos] != 0x0A {
        // garbage at the end of the line
        return false;
    }

    let label = normalize_reference(js::slice_to(&str, 1, label_end));
    if label.is_empty() {
        // CommonMark 0.20 disallows empty labels
        return false;
    }

    // Reference can not terminate anything. This check is for safety only.
    /* istanbul ignore if */
    if silent {
        return true;
    }

    // if (typeof state.env.references === 'undefined') state.env.references = {}
    let references = state.env.references.get_or_insert_with(HashMap::new);
    // if (typeof state.env.references[label] === 'undefined') ... (the first definition wins)
    references
        .entry(label.clone())
        .or_insert_with(|| Reference {
            href: js::to_string(&href),
            title: js::to_string(&title),
        });

    // Marks the place definition took in the source. Renders to nothing,
    // href/title stay in `env.references`.
    let token = state.push("reference_definition", "", 0);
    token.map = Some([start_line, next_line]);
    token.hidden = true;

    // const meta = Object.create(null); meta.label = label
    let mut meta = serde_json::Map::new();
    meta.insert("label".to_owned(), serde_json::Value::String(label));
    token.meta = Some(serde_json::Value::Object(meta));

    state.line = next_line;
    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn reference_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "[a]: /u 'b\nc'\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,2] L0 hidden meta={\"label\":\"A\"}",
                    "ref A=\"/u\" \"b\\nc\"",
                ],
            ),
            (
                "[a]: /u 'b' x\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"[a]: /u 'b' x\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "[a]: /u\n'b'\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,2] L0 hidden meta={\"label\":\"A\"}",
                    "ref A=\"/u\" \"b\"",
                ],
            ),
            (
                "[a]: /u\n'b' x\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,1] L0 hidden meta={\"label\":\"A\"}",
                    "paragraph_open/p [1,2] L0",
                    "inline/ [1,2] L1 content=\"'b' x\"",
                    "paragraph_close/p L0",
                    "ref A=\"/u\" \"\"",
                ],
            ),
            (
                "[a\nb]: /u\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,2] L0 hidden meta={\"label\":\"A B\"}",
                    "ref A B=\"/u\" \"\"",
                ],
            ),
            (
                "[a]: /1\n[A]: /2 \"t\"\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,1] L0 hidden meta={\"label\":\"A\"}",
                    "reference_definition/ [1,2] L0 hidden meta={\"label\":\"A\"}",
                    "ref A=\"/1\" \"\"",
                ],
            ),
            (
                "[]: /u\n[ ]: /u\n[a]]: /u\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,3] L0",
                    "inline/ [0,3] L1 content=\"[]: /u\\n[ ]: /u\\n[a]]: /u\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "[a]:\n\n/u\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"[a]:\"",
                    "paragraph_close/p L0",
                    "paragraph_open/p [2,3] L0",
                    "inline/ [2,3] L1 content=\"/u\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "[a]: <b> (t)\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,1] L0 hidden meta={\"label\":\"A\"}",
                    "ref A=\"b\" \"t\"",
                ],
            ),
            (
                "[a]: javascript:x\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"[a]: javascript:x\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "> [a]: /u\n> 'b\nc'\n",
                "{}",
                &[],
                &[
                    "blockquote_open/blockquote [0,3] L0 markup=\">\"",
                    "reference_definition/ [0,3] L1 hidden meta={\"label\":\"A\"}",
                    "blockquote_close/blockquote L0 markup=\">\"",
                    "ref A=\"/u\" \"b\\nc\"",
                ],
            ),
            (
                "[a\\]b]: /u\n[c]: /u 'd\n\ne'\n",
                "{}",
                &[],
                &[
                    "reference_definition/ [0,1] L0 hidden meta={\"label\":\"A\\\\]B\"}",
                    "paragraph_open/p [1,2] L0",
                    "inline/ [1,2] L1 content=\"[c]: /u 'd\"",
                    "paragraph_close/p L0",
                    "paragraph_open/p [3,4] L0",
                    "inline/ [3,4] L1 content=\"e'\"",
                    "paragraph_close/p L0",
                    "ref A\\]B=\"/u\" \"\"",
                ],
            ),
            (
                "    [a]: /u\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &[
                    "reference_definition/ [0,1] L0 hidden meta={\"label\":\"A\"}",
                    "ref A=\"/u\" \"\"",
                ],
            ),
        ];
        check(cases);
    }
}
