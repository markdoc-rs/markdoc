//! Port of rules_block/table.ts: GFM table, <https://github.github.com/gfm/#tables-extension->

use super::state_block::StateBlock;
use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::js;
use crate::markdown_it::token::AttrValue;

// Limit the amount of empty autocompleted cells in a table,
// see https://github.com/markdown-it/markdown-it/issues/1000,
//
// Both pulldown-cmark and commonmark-hs limit the number of cells this way to ~200k.
// We set it to 65k, which can expand user input by a factor of x370
// (256x256 square is 1.8kB expanded into 650kB).
const MAX_AUTOCOMPLETED_CELLS: i64 = 0x10000;

fn get_line<'s>(state: &'s StateBlock, line: usize) -> &'s [u16] {
    let pos = state.b_marks[line] + state.t_shift[line];
    let max = state.e_marks[line];

    js::slice_to(&state.src, pos, max)
}

fn escaped_split(str: &[u16]) -> Vec<Vec<u16>> {
    let mut result: Vec<Vec<u16>> = Vec::new();
    let max = str.len();

    let mut pos = 0;
    // `None` is JS NaN (the empty string)
    let mut ch = js::char_code_at(str, pos);
    let mut is_escaped = false;
    let mut last_pos = 0;
    let mut current: Vec<u16> = Vec::new();

    while pos < max {
        if ch == Some(0x7c /* | */) {
            if !is_escaped {
                // pipe separating cells, '|'
                // result.push(current + str.substring(lastPos, pos))
                let mut cell = std::mem::take(&mut current);
                cell.extend_from_slice(js::substring(str, last_pos as i64, Some(pos as i64)));
                result.push(cell);
                last_pos = pos + 1;
            } else {
                // escaped pipe, '\|'
                current.extend_from_slice(js::substring(
                    str,
                    last_pos as i64,
                    Some(pos as i64 - 1),
                ));
                last_pos = pos;
            }
        }

        is_escaped = ch == Some(0x5c /* \ */);
        pos += 1;

        ch = js::char_code_at(str, pos);
    }

    // result.push(current + str.substring(lastPos))
    current.extend_from_slice(js::substring(str, last_pos as i64, None));
    result.push(current);

    result
}

/// `/^:?-+:?$/.test(t)`, by hand: the regex is ASCII-only, and `-+` cannot give back a
/// character to `:?$` (a ':' is never a '-'), so one greedy pass decides it.
fn is_align_cell(t: &[u16]) -> bool {
    let mut i = 0;
    if t.get(i) == Some(&0x3A) {
        i += 1;
    }
    let dashes = i;
    while t.get(i) == Some(&0x2D) {
        i += 1;
    }
    if i == dashes {
        return false;
    }
    if t.get(i) == Some(&0x3A) {
        i += 1;
    }
    i == t.len()
}

pub fn table(state: &mut StateBlock, start_line: usize, end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let end_line = end_line.min(state.line_max);
    // should have at least two lines
    if start_line + 2 > end_line {
        return false;
    }

    let mut next_line = start_line + 1;

    if state.s_count[next_line] < state.blk_indent {
        return false;
    }

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent >= 4 {
        return false;
    }

    // first character of the second line should be '|', '-', ':',
    // and no other characters are allowed but spaces;
    // basically, this is the equivalent of /^[-:|][-:|\s]*$/ regexp

    let mut pos = state.b_marks[next_line] + state.t_shift[next_line];
    if pos >= state.e_marks[next_line] {
        return false;
    }

    let first_ch = state.src[pos];
    pos += 1;
    // | - :
    if first_ch != 0x7C && first_ch != 0x2D && first_ch != 0x3A {
        return false;
    }

    if pos >= state.e_marks[next_line] {
        return false;
    }

    let second_ch = state.src[pos];
    pos += 1;
    // | - :
    if second_ch != 0x7C && second_ch != 0x2D && second_ch != 0x3A && !is_space(second_ch) {
        return false;
    }

    // if first character is '-', then second character must not be a space
    // (due to parsing ambiguity with list)
    if first_ch == 0x2D && is_space(second_ch) {
        return false;
    }

    while pos < state.e_marks[next_line] {
        let ch = state.src[pos];

        // | - :
        if ch != 0x7C && ch != 0x2D && ch != 0x3A && !is_space(ch) {
            return false;
        }

        pos += 1;
    }

    let mut line_text: Vec<u16> = get_line(state, start_line + 1).to_vec();
    let mut columns: Vec<Vec<u16>> = js::split(&line_text, &[0x7C])
        .into_iter()
        .map(<[u16]>::to_vec)
        .collect();
    let mut aligns: Vec<&str> = Vec::new();
    for i in 0..columns.len() {
        let t = js::trim(&columns[i]);
        if t.is_empty() {
            // allow empty columns before and after table, but not in between columns;
            // e.g. allow ` |---| `, disallow ` ---||--- `
            if i == 0 || i == columns.len() - 1 {
                continue;
            } else {
                return false;
            }
        }

        if !is_align_cell(t) {
            return false;
        }
        // :
        if t[t.len() - 1] == 0x3A {
            aligns.push(if t[0] == 0x3A { "center" } else { "right" });
        } else if t[0] == 0x3A {
            aligns.push("left");
        } else {
            aligns.push("");
        }
    }

    line_text = js::trim(get_line(state, start_line)).to_vec();
    if js::index_of(&line_text, &[0x7C], 0).is_none() {
        return false;
    }
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }
    columns = escaped_split(&line_text);
    if !columns.is_empty() && columns[0].is_empty() {
        columns.remove(0);
    }
    if !columns.is_empty() && columns[columns.len() - 1].is_empty() {
        columns.pop();
    }

    // header row will define an amount of columns in the entire table,
    // and align row should be exactly the same (the rest of the rows can differ)
    let column_count = columns.len();
    if column_count == 0 || column_count != aligns.len() {
        return false;
    }

    if silent {
        return true;
    }

    let old_parent_type = state.parent_type;
    state.parent_type = "table";

    // use 'blockquote' lists for termination because it's
    // the most similar to tables
    let terminator_rules = state.md.block.ruler.get_rules("blockquote");

    // `const tableLines = [startLine, 0]; token_to.map = tableLines` and later
    // `tableLines[1] = nextLine`: the array is shared, so the map is completed in place at
    // the end (only later tokens are pushed meanwhile, so the index stays valid).
    let table_lines = state.tokens.len();
    let token_to = state.push("table_open", "table", 1);
    token_to.map = Some([start_line, 0]);

    let token_tho = state.push("thead_open", "thead", 1);
    token_tho.map = Some([start_line, start_line + 1]);

    let token_htro = state.push("tr_open", "tr", 1);
    token_htro.map = Some([start_line, start_line + 1]);

    for i in 0..columns.len() {
        let token_ho = state.push("th_open", "th", 1);
        if !aligns[i].is_empty() {
            token_ho.attrs = Some(vec![(
                "style".to_owned(),
                AttrValue::Str(format!("text-align:{}", aligns[i])),
            )]);
        }

        let token_il = state.push("inline", "", 0);
        token_il.content = js::to_string(js::trim(&columns[i]));
        token_il.children = Some(Vec::new());

        state.push("th_close", "th", -1);
    }

    state.push("tr_close", "tr", -1);
    state.push("thead_close", "thead", -1);

    // `let tbodyLines`: the tbody_open token, whose map is completed in place at the end
    let mut tbody_lines: Option<usize> = None;
    let mut autocompleted_cells: i64 = 0;

    // for (nextLine = startLine + 2; nextLine < endLine; nextLine++)
    next_line = start_line + 2;
    while next_line < end_line {
        if state.s_count[next_line] < state.blk_indent {
            break;
        }

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
        line_text = js::trim(get_line(state, next_line)).to_vec();
        if line_text.is_empty() {
            break;
        }
        if !state.md.options.allow_indentation && state.s_count[next_line] - state.blk_indent >= 4 {
            break;
        }
        columns = escaped_split(&line_text);
        if !columns.is_empty() && columns[0].is_empty() {
            columns.remove(0);
        }
        if !columns.is_empty() && columns[columns.len() - 1].is_empty() {
            columns.pop();
        }

        // note: autocomplete count can be negative if user specifies more columns than header,
        // but that does not affect intended use (which is limiting expansion)
        autocompleted_cells += column_count as i64 - columns.len() as i64;
        if autocompleted_cells > MAX_AUTOCOMPLETED_CELLS {
            break;
        }

        if next_line == start_line + 2 {
            tbody_lines = Some(state.tokens.len());
            let token_tbo = state.push("tbody_open", "tbody", 1);
            token_tbo.map = Some([start_line + 2, 0]);
        }

        let token_tro = state.push("tr_open", "tr", 1);
        token_tro.map = Some([next_line, next_line + 1]);

        for i in 0..column_count {
            let token_tdo = state.push("td_open", "td", 1);
            if !aligns[i].is_empty() {
                token_tdo.attrs = Some(vec![(
                    "style".to_owned(),
                    AttrValue::Str(format!("text-align:{}", aligns[i])),
                )]);
            }

            let token_il = state.push("inline", "", 0);
            // columns[i] ? columns[i].trim() : ''
            token_il.content = columns
                .get(i)
                .map_or_else(String::new, |c| js::to_string(js::trim(c)));
            token_il.children = Some(Vec::new());

            state.push("td_close", "td", -1);
        }
        state.push("tr_close", "tr", -1);
        next_line += 1;
    }

    if let Some(tbody_lines) = tbody_lines {
        state.push("tbody_close", "tbody", -1);
        if let Some(map) = state.tokens[tbody_lines].map.as_mut() {
            map[1] = next_line;
        }
    }

    state.push("table_close", "table", -1);
    if let Some(map) = state.tokens[table_lines].map.as_mut() {
        map[1] = next_line;
    }

    state.parent_type = old_parent_type;
    state.line = next_line;
    true
}

#[cfg(test)]
mod tests {
    use super::{escaped_split, is_align_cell};
    use crate::markdown_it::js;
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn escaped_split_matches_node() {
        // node: escapedSplit(s), the bundle's own function
        let cases: &[(&str, &[&str])] = &[
            ("", &[""]),
            ("|", &["", ""]),
            ("a", &["a"]),
            ("a|b", &["a", "b"]),
            ("|a|", &["", "a", ""]),
            ("a\\|b|c", &["a|b", "c"]),
            ("\\|", &["|"]),
            ("\\\\|x", &["\\|x"]),
            ("a||b", &["a", "", "b"]),
            ("a\\", &["a\\"]),
            ("\\|\\|", &["||"]),
            ("a|\\|", &["a", "|"]),
        ];
        for &(s, want) in cases {
            let got: Vec<String> = escaped_split(&js::to_utf16(s))
                .iter()
                .map(|c| js::to_string(c))
                .collect();
            assert_eq!(got, want, "{s:?}");
        }
    }

    #[test]
    fn align_cell_is_the_regex() {
        // node: /^:?-+:?$/.test(t)
        for (t, want) in [
            ("-", true),
            (":-", true),
            ("-:", true),
            (":-:", true),
            ("---", true),
            ("--:", true),
            (":", false),
            ("::", false),
            ("", false),
            (":--::", false),
            ("-:-", false),
            ("a", false),
            (" - ", false),
            ("::-", false),
        ] {
            assert_eq!(is_align_cell(&js::to_utf16(t)), want, "{t:?}");
        }
    }

    #[test]
    fn table_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "| a | b | c |\n|:-|-:|:-:|\n| d | e |\n| f | g | h | i |\n\nx\n",
                "{}",
                &[],
                &[
                    "table_open/table [0,4] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3 style=\"text-align:left\"",
                    "inline/ L4 content=\"a\"",
                    "th_close/th L3",
                    "th_open/th L3 style=\"text-align:right\"",
                    "inline/ L4 content=\"b\"",
                    "th_close/th L3",
                    "th_open/th L3 style=\"text-align:center\"",
                    "inline/ L4 content=\"c\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "tbody_open/tbody [2,4] L1",
                    "tr_open/tr [2,3] L2",
                    "td_open/td L3 style=\"text-align:left\"",
                    "inline/ L4 content=\"d\"",
                    "td_close/td L3",
                    "td_open/td L3 style=\"text-align:right\"",
                    "inline/ L4 content=\"e\"",
                    "td_close/td L3",
                    "td_open/td L3 style=\"text-align:center\"",
                    "inline/ L4",
                    "td_close/td L3",
                    "tr_close/tr L2",
                    "tr_open/tr [3,4] L2",
                    "td_open/td L3 style=\"text-align:left\"",
                    "inline/ L4 content=\"f\"",
                    "td_close/td L3",
                    "td_open/td L3 style=\"text-align:right\"",
                    "inline/ L4 content=\"g\"",
                    "td_close/td L3",
                    "td_open/td L3 style=\"text-align:center\"",
                    "inline/ L4 content=\"h\"",
                    "td_close/td L3",
                    "tr_close/tr L2",
                    "tbody_close/tbody L1",
                    "table_close/table L0",
                    "paragraph_open/p [5,6] L0",
                    "inline/ [5,6] L1 content=\"x\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "a \\| b | c\n--- | ---\n",
                "{}",
                &[],
                &[
                    "table_open/table [0,2] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3",
                    "inline/ L4 content=\"a | b\"",
                    "th_close/th L3",
                    "th_open/th L3",
                    "inline/ L4 content=\"c\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "table_close/table L0",
                ],
            ),
            (
                "| a |\n|---|\n",
                "{}",
                &[],
                &[
                    "table_open/table [0,2] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3",
                    "inline/ L4 content=\"a\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "table_close/table L0",
                ],
            ),
            (
                "a | b\n-- | -\n> c\n",
                "{}",
                &[],
                &[
                    "table_open/table [0,2] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3",
                    "inline/ L4 content=\"a\"",
                    "th_close/th L3",
                    "th_open/th L3",
                    "inline/ L4 content=\"b\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "table_close/table L0",
                    "blockquote_open/blockquote [2,3] L0 markup=\">\"",
                    "paragraph_open/p [2,3] L1",
                    "inline/ [2,3] L2 content=\"c\"",
                    "paragraph_close/p L1",
                    "blockquote_close/blockquote L0 markup=\">\"",
                ],
            ),
            (
                "a | b\n|--|\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"a | b\\n|--|\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "a | b\n- | -\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a | b\"",
                    "paragraph_close/p L0",
                    "bullet_list_open/ul [1,2] L0 markup=\"-\"",
                    "list_item_open/li [1,2] L1 markup=\"-\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"| -\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "a\n|-|\n",
                "{}",
                &[],
                &[
                    "paragraph_open/p [0,2] L0",
                    "inline/ [0,2] L1 content=\"a\\n|-|\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "| a | b |\n| - | - |\n    | c | d |\n",
                "{}",
                &[],
                &[
                    "table_open/table [0,2] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3",
                    "inline/ L4 content=\"a\"",
                    "th_close/th L3",
                    "th_open/th L3",
                    "inline/ L4 content=\"b\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "table_close/table L0",
                    "code_block/code [2,3] L0 content=\"| c | d |\\n\"",
                ],
            ),
            (
                "| a | b |\n| - | - |\n    | c | d |\n",
                "{\"allowIndentation\":true}",
                &[],
                &[
                    "table_open/table [0,3] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3",
                    "inline/ L4 content=\"a\"",
                    "th_close/th L3",
                    "th_open/th L3",
                    "inline/ L4 content=\"b\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "tbody_open/tbody [2,3] L1",
                    "tr_open/tr [2,3] L2",
                    "td_open/td L3",
                    "inline/ L4 content=\"c\"",
                    "td_close/td L3",
                    "td_open/td L3",
                    "inline/ L4 content=\"d\"",
                    "td_close/td L3",
                    "tr_close/tr L2",
                    "tbody_close/tbody L1",
                    "table_close/table L0",
                ],
            ),
            (
                "    a | b\n    - | -\n",
                "{\"allowIndentation\":true}",
                &["code"],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"a | b\"",
                    "paragraph_close/p L0",
                    "bullet_list_open/ul [1,2] L0 markup=\"-\"",
                    "list_item_open/li [1,2] L1 markup=\"-\"",
                    "paragraph_open/p [1,2] L2 hidden",
                    "inline/ [1,2] L3 content=\"| -\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "a|b\n:-:|---:\n\\|c\\\\|d|\n|e\n",
                "{}",
                &[],
                &[
                    "table_open/table [0,4] L0",
                    "thead_open/thead [0,1] L1",
                    "tr_open/tr [0,1] L2",
                    "th_open/th L3 style=\"text-align:center\"",
                    "inline/ L4 content=\"a\"",
                    "th_close/th L3",
                    "th_open/th L3 style=\"text-align:right\"",
                    "inline/ L4 content=\"b\"",
                    "th_close/th L3",
                    "tr_close/tr L2",
                    "thead_close/thead L1",
                    "tbody_open/tbody [2,4] L1",
                    "tr_open/tr [2,3] L2",
                    "td_open/td L3 style=\"text-align:center\"",
                    "inline/ L4 content=\"|c\\\\|d\"",
                    "td_close/td L3",
                    "td_open/td L3 style=\"text-align:right\"",
                    "inline/ L4",
                    "td_close/td L3",
                    "tr_close/tr L2",
                    "tr_open/tr [3,4] L2",
                    "td_open/td L3 style=\"text-align:center\"",
                    "inline/ L4 content=\"e\"",
                    "td_close/td L3",
                    "td_open/td L3 style=\"text-align:right\"",
                    "inline/ L4",
                    "td_close/td L3",
                    "tr_close/tr L2",
                    "tbody_close/tbody L1",
                    "table_close/table L0",
                ],
            ),
        ];
        check(cases);
    }
}
