//! Port of rules_block/list.ts: lists.

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::js;
use crate::markdown_it::token::AttrValue;

/// Search `[-+*][\n ]`, returns next pos after marker on success
/// or -1 on fail (`None`).
fn skip_bullet_list_marker(state: &StateBlock, start_line: usize) -> Option<usize> {
    let max = state.e_marks[start_line];
    let mut pos = state.b_marks[start_line] + state.t_shift[start_line];

    // `None` is JS NaN (an empty last line): no bullet
    let marker = js::char_code_at(&state.src, pos);
    pos += 1;
    // Check bullet: * - +
    if marker != Some(0x2A) && marker != Some(0x2D) && marker != Some(0x2B) {
        return None;
    }

    if pos < max {
        let ch = state.src[pos];

        if !is_space(ch) {
            // " -test " - is not a list item
            return None;
        }
    }

    Some(pos)
}

/// Search `\d+[.)][\n ]`, returns next pos after marker on success
/// or -1 on fail (`None`).
fn skip_ordered_list_marker(state: &StateBlock, start_line: usize) -> Option<usize> {
    let start = state.b_marks[start_line] + state.t_shift[start_line];
    let max = state.e_marks[start_line];
    let mut pos = start;

    // List marker should have at least 2 chars (digit + dot)
    if pos + 1 >= max {
        return None;
    }

    let mut ch = state.src[pos];
    pos += 1;

    // 0 9
    if ch < 0x30 || ch > 0x39 {
        return None;
    }

    loop {
        // EOL -> fail
        if pos >= max {
            return None;
        }

        ch = state.src[pos];
        pos += 1;

        // 0 9
        if ch >= 0x30 && ch <= 0x39 {
            // List marker should have no more than 9 digits
            // (prevents integer overflow in browsers)
            if pos - start >= 10 {
                return None;
            }

            continue;
        }

        // found valid marker: ) .
        if ch == 0x29 || ch == 0x2e {
            break;
        }

        return None;
    }

    if pos < max {
        ch = state.src[pos];

        if !is_space(ch) {
            // " 1.test " - is not a list item
            return None;
        }
    }
    Some(pos)
}

fn mark_tight_paragraphs(state: &mut StateBlock, idx: usize) {
    let level = state.level + 2;

    // for (let i = idx + 2, l = state.tokens.length - 2; i < l; i++)
    let mut i = idx + 2;
    let l = state.tokens.len() as i64 - 2;
    while (i as i64) < l {
        if state.tokens[i].level == level && state.tokens[i].kind == "paragraph_open" {
            state.tokens[i + 2].hidden = true;
            state.tokens[i].hidden = true;
            i += 2;
        }
        i += 1;
    }
}

pub fn list(state: &mut StateBlock, start_line: usize, end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    let mut max;
    let mut pos;
    // JS `let start`: undefined for a bullet list, which never reads it
    let mut start: usize = 0;
    let mut next_line = start_line;
    let mut tight = true;

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent >= 4 {
        return false;
    }

    // Special case:
    //  - item 1
    //   - item 2
    //    - item 3
    //     - item 4
    //      - this one is a paragraph continuation
    if !state.md.options.allow_indentation
        && state.list_indent >= 0
        && state.s_count[next_line] - state.list_indent >= 4
        && state.s_count[next_line] < state.blk_indent
    {
        return false;
    }

    let mut is_terminating_paragraph = false;

    // limit conditions when list can interrupt
    // a paragraph (validation mode only)
    if silent && state.parent_type == "paragraph" {
        // Next list item should still terminate previous list item;
        //
        // This code can fail if plugins use blkIndent as well as lists,
        // but I hope the spec gets fixed long before that happens.
        //
        if state.s_count[next_line] >= state.blk_indent {
            is_terminating_paragraph = true;
        }
    }

    // Detect list type and position after marker
    let is_ordered;
    // JS `let markerValue`: undefined for a bullet list, which never reads it
    let mut marker_value: f64 = 0.0;
    let mut pos_after_marker;
    if let Some(p) = skip_ordered_list_marker(state, next_line) {
        pos_after_marker = p;
        is_ordered = true;
        start = state.b_marks[next_line] + state.t_shift[next_line];
        marker_value = js::string_to_number(js::slice_to(&state.src, start, pos_after_marker - 1));

        // If we're starting a new ordered list right after
        // a paragraph, it should start with 1.
        if is_terminating_paragraph && marker_value != 1.0 {
            return false;
        }
    } else if let Some(p) = skip_bullet_list_marker(state, next_line) {
        pos_after_marker = p;
        is_ordered = false;
    } else {
        return false;
    }

    // If we're starting a new unordered list right after
    // a paragraph, first line should not be empty.
    if is_terminating_paragraph {
        if state.skip_spaces(pos_after_marker) >= state.e_marks[next_line] {
            return false;
        }
    }

    // For validation mode we can terminate immediately
    if silent {
        return true;
    }

    // We should terminate list on style change. Remember first one to compare.
    let marker_char_code = state.src[pos_after_marker - 1];
    // String.fromCharCode(markerCharCode)
    let marker_str = js::to_string(&[marker_char_code]);

    // Start list
    let list_tok_idx = state.tokens.len();

    let token = if is_ordered {
        let token = state.push("ordered_list_open", "ol", 1);
        if marker_value != 1.0 {
            token.attrs = Some(vec![("start".to_owned(), AttrValue::Num(marker_value))]);
        }
        token
    } else {
        state.push("bullet_list_open", "ul", 1)
    };

    // `const listLines = [nextLine, 0]; token.map = listLines` and later
    // `listLines[1] = nextLine`: the array is shared, so the map is completed in place at the
    // end (`list_tok_idx` is this token: only later tokens are pushed meanwhile).
    let list_lines_0 = next_line;
    token.map = Some([list_lines_0, 0]);
    token.markup = marker_str.clone();

    //
    // Iterate list items
    //

    let mut prev_empty_end = false;
    let terminator_rules = state.md.block.ruler.get_rules("list");

    let old_parent_type = state.parent_type;
    state.parent_type = "list";

    while next_line < end_line {
        pos = pos_after_marker;
        max = state.e_marks[next_line];

        let initial = state.s_count[next_line] + pos_after_marker as i64
            - (state.b_marks[next_line] + state.t_shift[next_line]) as i64;
        let mut offset = initial;

        while pos < max {
            let ch = state.src[pos];

            if ch == 0x09 {
                offset += 4 - (offset + state.bs_count[next_line]) % 4;
            } else if ch == 0x20 {
                offset += 1;
            } else {
                break;
            }

            pos += 1;
        }

        let content_start = pos;
        let mut indent_after_marker;

        if content_start >= max {
            // trimming space in "-    \n  3" case, indent is 1 here
            indent_after_marker = 1;
        } else {
            indent_after_marker = offset - initial;
        }

        // If we have more than 4 spaces, the indent is 1
        // (the rest is just indented code block)
        if !state.md.options.allow_indentation && indent_after_marker > 4 {
            indent_after_marker = 1;
        }

        // "  -  test"
        //  ^^^^^ - calculating total length of this thing
        let indent = initial + indent_after_marker;

        // Run subparser & write tokens
        // (`token` borrows `state`: what JS reads from `state` after the push is read first)
        let info = if is_ordered {
            Some(js::to_string(js::slice_to(
                &state.src,
                start,
                pos_after_marker - 1,
            )))
        } else {
            None
        };
        // `const itemLines = [nextLine, 0]; token.map = itemLines` and later
        // `itemLines[1] = nextLine`: completed in place below, as `listLines`
        let item_lines = state.tokens.len();
        let item_lines_0 = next_line;
        let token = state.push("list_item_open", "li", 1);
        token.markup = marker_str.clone();
        token.map = Some([item_lines_0, 0]);
        if let Some(info) = info {
            token.info = info;
        }

        // change current state, then restore it after parser subcall
        let old_tight = state.tight;
        let old_t_shift = state.t_shift[next_line];
        let old_s_count = state.s_count[next_line];

        //  - example list
        // ^ listIndent position will be here
        //   ^ blkIndent position will be here
        //
        let old_list_indent = state.list_indent;
        state.list_indent = state.blk_indent;
        state.blk_indent = indent;

        state.tight = true;
        state.t_shift[next_line] = content_start - state.b_marks[next_line];
        state.s_count[next_line] = offset;

        if content_start >= max && state.is_empty(next_line + 1) {
            // workaround for this case
            // (list item is empty, list terminates before "foo"):
            // ~~~~~~~~
            //   -
            //
            //     foo
            // ~~~~~~~~
            state.line = (state.line + 2).min(end_line);
        } else {
            let md = state.md;
            // Not JS's: a level of node's stack (see `cost`); past it the RangeError is thrown.
            if let Some(_level) = state.env.level(crate::markdown_it::cost::LIST) {
                crate::depth::grow!(md.block.tokenize(state, next_line, end_line));
            }
            // Not JS's: where the subcall threw (see `JsError`), JS is gone from here. The
            // port must not go on: `state.line` has not moved, so this loop would push items
            // for the same line forever.
            if state.env.has_thrown() {
                state.parent_type = old_parent_type;
                return true;
            }
        }

        // If any of list item is tight, mark list as tight
        if !state.tight || prev_empty_end {
            tight = false;
        }
        // Item become loose if finish with empty line,
        // but we should filter last element, because it means list finish
        prev_empty_end =
            (state.line as i64 - next_line as i64) > 1 && state.is_empty(state.line - 1);

        state.blk_indent = state.list_indent;
        state.list_indent = old_list_indent;
        state.t_shift[next_line] = old_t_shift;
        state.s_count[next_line] = old_s_count;
        state.tight = old_tight;

        let token = state.push("list_item_close", "li", -1);
        token.markup = marker_str.clone();

        next_line = state.line;
        if let Some(map) = state.tokens[item_lines].map.as_mut() {
            map[1] = next_line;
        }

        if next_line >= end_line {
            break;
        }

        //
        // Try to check if list is terminated or continued.
        //
        if state.s_count[next_line] < state.blk_indent {
            break;
        }

        // if it's indented more than 3 spaces, it should be a code block
        if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent >= 4 {
            break;
        }

        // fail if terminating block found
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

        // fail if list has another type
        if is_ordered {
            match skip_ordered_list_marker(state, next_line) {
                Some(p) => pos_after_marker = p,
                None => break,
            }
            start = state.b_marks[next_line] + state.t_shift[next_line];
        } else {
            match skip_bullet_list_marker(state, next_line) {
                Some(p) => pos_after_marker = p,
                None => break,
            }
        }

        if marker_char_code != state.src[pos_after_marker - 1] {
            break;
        }
    }

    // Finalize list
    let token = if is_ordered {
        state.push("ordered_list_close", "ol", -1)
    } else {
        state.push("bullet_list_close", "ul", -1)
    };
    token.markup = marker_str;

    if let Some(map) = state.tokens[list_tok_idx].map.as_mut() {
        map[1] = next_line;
    }
    state.line = next_line;

    state.parent_type = old_parent_type;

    // mark paragraphs tight if needed
    if tight {
        mark_tight_paragraphs(state, list_tok_idx);
    }

    true
}

#[cfg(test)]
mod tests {
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn list_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "- a\n- b\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,2] L0 markup=\"-\"",
                    "list_item_open/li [0,1] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [1,2] L1 markup=\"-\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "- a\n\n- b\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,2] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [2,3] L1 markup=\"-\"",
                    "paragraph_open/p [2,3] L2",
                    "inline/ [2,3] L3 content=\"b\"",
                    "paragraph_close/p L2",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "3. a\n4. b\n007. c\n",
                "{}",
                &[],
                &[
                    "ordered_list_open/ol [0,3] L0 markup=\".\" start=3",
                    "list_item_open/li [0,1] L1 markup=\".\" info=\"3\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\".\"",
                    "list_item_open/li [1,2] L1 markup=\".\" info=\"4\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\".\"",
                    "list_item_open/li [2,3] L1 markup=\".\" info=\"007\"",
                    "paragraph_open/p [2,3] L2 hidden",
                    "inline/ [2,3] L3 content=\"c\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\".\"",
                    "ordered_list_close/ol L0 markup=\".\"",
                ],
            ),
            (
                "0. a\n1) b\n",
                "{}",
                &[],
                &[
                    "ordered_list_open/ol [0,1] L0 markup=\".\" start=0",
                    "list_item_open/li [0,1] L1 markup=\".\" info=\"0\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\".\"",
                    "ordered_list_close/ol L0 markup=\".\"",
                    "ordered_list_open/ol [1,2] L0 markup=\")\"",
                    "list_item_open/li [1,2] L1 markup=\")\" info=\"1\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\")\"",
                    "ordered_list_close/ol L0 markup=\")\"",
                ],
            ),
            (
                "-\n\n  foo\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,2] L0 markup=\"-\"",
                    "list_item_open/li [0,2] L1 markup=\"-\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                    "paragraph_open/p [2,3] L0",
                    "inline/ [2,3] L1 content=\"foo\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "a\n2. b\n1. c\n- \n* d\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"a\\n2. b\"",
                    "paragraph_close/p L0",
                    "ordered_list_open/ol [2,3] L0 markup=\".\"",
                    "list_item_open/li [2,3] L1 markup=\".\" info=\"1\"",
                    "paragraph_open/p [2,3] L2 hidden",
                    "inline/ [2,3] L3 content=\"c\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\".\"",
                    "ordered_list_close/ol L0 markup=\".\"",
                    "bullet_list_open/ul [3,4] L0 markup=\"-\"",
                    "list_item_open/li [3,4] L1 markup=\"-\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                    "bullet_list_open/ul [4,5] L0 markup=\"*\"",
                    "list_item_open/li [4,5] L1 markup=\"*\"",
                    "paragraph_open/p [4,5] L2 hidden",
                    "inline/ [4,5] L3 content=\"d\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"*\"",
                    "bullet_list_close/ul L0 markup=\"*\"",
                ],
            ),
            (
                "- a\n+ b\n* c\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,1] L0 markup=\"-\"",
                    "list_item_open/li [0,1] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                    "bullet_list_open/ul [1,2] L0 markup=\"+\"",
                    "list_item_open/li [1,2] L1 markup=\"+\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"+\"",
                    "bullet_list_close/ul L0 markup=\"+\"",
                    "bullet_list_open/ul [2,3] L0 markup=\"*\"",
                    "list_item_open/li [2,3] L1 markup=\"*\"",
                    "paragraph_open/p [2,3] L2 hidden",
                    "inline/ [2,3] L3 content=\"c\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"*\"",
                    "bullet_list_close/ul L0 markup=\"*\"",
                ],
            ),
            (
                "- a\n  - b\n\n    c\n- d\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,5] L0 markup=\"-\"",
                    "list_item_open/li [0,4] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "bullet_list_open/ul [1,4] L2 markup=\"-\"",
                    "list_item_open/li [1,4] L3 markup=\"-\"",
                    "paragraph_open/p [1,2] L4",
                    "inline/ [1,2] L5 content=\"b\"",
                    "paragraph_close/p L4",
                    "paragraph_open/p [3,4] L4",
                    "inline/ [3,4] L5 content=\"c\"",
                    "paragraph_close/p L4",
                    "list_item_close/li L3 markup=\"-\"",
                    "bullet_list_close/ul L2 markup=\"-\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [4,5] L1 markup=\"-\"",
                    "paragraph_open/p [4,5] L2 hidden",
                    "inline/ [4,5] L3 content=\"d\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "-      a\n\n       b\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,3] L1 markup=\"-\"",
                    "code_block/code [0,3] L2 content=\" a\\n\\n b\\n\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "-      a\n\n       b\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,3] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2",
                    "paragraph_open/p [2,3] L2",
                    "inline/ [2,3] L3 content=\"b\"",
                    "paragraph_close/p L2",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "- a\n\n      b\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,3] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2",
                    "paragraph_open/p [2,3] L2",
                    "inline/ [2,3] L3 content=\"b\"",
                    "paragraph_close/p L2",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                " - a\n  - b\n   - c\n    - d\n     - e\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,5] L0 markup=\"-\"",
                    "list_item_open/li [0,1] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [1,2] L1 markup=\"-\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [2,5] L1 markup=\"-\"",
                    "paragraph_open/p [2,4] L2 hidden",
                    "inline/ [2,4] L3 content=\"c\\n- d\"",
                    "paragraph_close/p L2 hidden",
                    "bullet_list_open/ul [4,5] L2 markup=\"-\"",
                    "list_item_open/li [4,5] L3 markup=\"-\"",
                    "paragraph_open/p [4,5] L4 hidden",
                    "inline/ [4,5] L5 content=\"e\"",
                    "paragraph_close/p L4 hidden",
                    "list_item_close/li L3 markup=\"-\"",
                    "bullet_list_close/ul L2 markup=\"-\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                " - a\n  - b\n   - c\n    - d\n     - e\n",
                "{\"allowIndentation\":true}",
                &[],
                &[
                    "bullet_list_open/ul [0,5] L0 markup=\"-\"",
                    "list_item_open/li [0,1] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [1,2] L1 markup=\"-\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [2,3] L1 markup=\"-\"",
                    "paragraph_open/p [2,3] L2 hidden",
                    "inline/ [2,3] L3 content=\"c\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [3,4] L1 markup=\"-\"",
                    "paragraph_open/p [3,4] L2 hidden",
                    "inline/ [3,4] L3 content=\"d\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [4,5] L1 markup=\"-\"",
                    "paragraph_open/p [4,5] L2 hidden",
                    "inline/ [4,5] L3 content=\"e\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "1234567890. a\n123456789. b\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"1234567890. a\\n123456789. b\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "-\ta\n-\t\tb\n",
                "{}",
                &[],
                &[
                    "bullet_list_open/ul [0,2] L0 markup=\"-\"",
                    "list_item_open/li [0,1] L1 markup=\"-\"",
                    "paragraph_open/p [0,1] L2 hidden",
                    "inline/ [0,1] L3 content=\"a\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [1,2] L1 markup=\"-\"",
                    "code_block/code [1,2] L2 content=\"  b\\n\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
        ];
        check(cases);
    }
}
