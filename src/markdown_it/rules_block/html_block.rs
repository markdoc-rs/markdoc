//! Port of rules_block/html_block.ts: HTML block.

use super::state_block::StateBlock;
use crate::markdown_it::common::html_blocks::HTML_BLOCKS;
use crate::markdown_it::common::html_re::html_open_close_tag_re_to_end;
use crate::markdown_it::js;

/// `regexp.test(lineText)`.
type Test = fn(&[u16]) -> bool;

// An array of opening and corresponding closing sequences for html tags,
// last argument defines whether it can terminate a paragraph or not
//
// The regexes are matched by hand over UTF-16 code units (none has the `u` flag). With the
// `i` flag (and no `u`), JS case-folds a character only to one of the same ASCII-ness, so
// the names here (ASCII) match exactly their ASCII case variants.
const HTML_SEQUENCES: [(Test, Test, bool); 7] = [
    // [/^<(script|pre|style|textarea)(?=(\s|>|$))/i, /<\/(script|pre|style|textarea)>/i, true]
    (raw_text_open, raw_text_close, true),
    // [/^<!--/, /-->/, true]
    (
        |s| s.starts_with(&[0x3C, 0x21, 0x2D, 0x2D]),
        |s| js::includes(s, &[0x2D, 0x2D, 0x3E], 0),
        true,
    ),
    // [/^<\?/, /\?>/, true]
    (
        |s| s.starts_with(&[0x3C, 0x3F]),
        |s| js::includes(s, &[0x3F, 0x3E], 0),
        true,
    ),
    // [/^<![A-Za-z]/, />/, true]
    (
        |s| s.starts_with(&[0x3C, 0x21]) && s.get(2).is_some_and(|&c| is_ascii_alpha(c)),
        |s| js::includes(s, &[0x3E], 0),
        true,
    ),
    // [/^<!\[CDATA\[/, /\]\]>/, true]
    (
        |s| s.starts_with(&[0x3C, 0x21, 0x5B, 0x43, 0x44, 0x41, 0x54, 0x41, 0x5B]),
        |s| js::includes(s, &[0x5D, 0x5D, 0x3E], 0),
        true,
    ),
    // [new RegExp(`^</?(${block_names.join('|')})(?=(\\s|/?>|$))`, 'i'), /^$/, true]
    (block_name_open, |s| s.is_empty(), true),
    // [new RegExp(`${HTML_OPEN_CLOSE_TAG_RE.source}\\s*$`), /^$/, false]
    (html_open_close_tag_re_to_end, |s| s.is_empty(), false),
];

fn is_ascii_alpha(c: u16) -> bool {
    matches!(c, 0x41..=0x5A | 0x61..=0x7A)
}

/// `s` has `name` (ASCII, lower case) at `at`, ignoring ASCII case.
fn has_name_at(s: &[u16], at: usize, name: &str) -> bool {
    let name = name.as_bytes();
    s.len() >= at + name.len()
        && s[at..at + name.len()]
            .iter()
            .zip(name)
            .all(|(&c, &n)| c == u16::from(n) || c == u16::from(n.to_ascii_uppercase()))
}

const RAW_TEXT_NAMES: [&str; 4] = ["script", "pre", "style", "textarea"];

/// `/^<(script|pre|style|textarea)(?=(\s|>|$))/i.test(s)`. No name is a prefix of another,
/// so at most one alternative can match.
fn raw_text_open(s: &[u16]) -> bool {
    s.first() == Some(&0x3C)
        && RAW_TEXT_NAMES.iter().any(|name| {
            has_name_at(s, 1, name)
                && match s.get(1 + name.len()) {
                    None => true, // $
                    Some(&c) => js::is_js_space(u32::from(c)) || c == 0x3E,
                }
        })
}

/// `/<\/(script|pre|style|textarea)>/i.test(s)`: anywhere in `s`.
fn raw_text_close(s: &[u16]) -> bool {
    (0..s.len()).any(|i| {
        s[i] == 0x3C
            && s.get(i + 1) == Some(&0x2F)
            && RAW_TEXT_NAMES
                .iter()
                .any(|name| has_name_at(s, i + 2, name) && s.get(i + 2 + name.len()) == Some(&0x3E))
    })
}

/// `new RegExp('^</?(' + block_names.join('|') + ')(?=(\\s|/?>|$))', 'i').test(s)`.
///
/// A backtracking engine tries `/?` with the `/` first and every name in turn (so `col`
/// failing its lookahead in `<colgroup>` still lets `colgroup` match); without the `/`, the
/// names would have to start with `/`. So: some name at 1 or 2, then the lookahead.
fn block_name_open(s: &[u16]) -> bool {
    if s.first() != Some(&0x3C) {
        return false;
    }
    let at = if s.get(1) == Some(&0x2F) { 2 } else { 1 };
    HTML_BLOCKS.iter().any(|name| {
        has_name_at(s, at, name)
            && match s.get(at + name.len()) {
                None => true, // $
                Some(&c) => {
                    js::is_js_space(u32::from(c))
                        || c == 0x3E
                        || (c == 0x2F && s.get(at + name.len() + 1) == Some(&0x3E))
                }
            }
    })
}

pub fn html_block(
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
    let mut max = state.e_marks[start_line];

    // if it's indented more than 3 spaces, it should be a code block
    if !state.md.options.allow_indentation && state.s_count[start_line] - state.blk_indent >= 4 {
        return false;
    }

    if !state.md.options.html {
        return false;
    }

    // <
    // `None` is JS NaN (an empty last line), not '<'
    if js::char_code_at(&state.src, pos) != Some(0x3C) {
        return false;
    }

    let mut line_text: Vec<u16> = js::slice_to(&state.src, pos, max).to_vec();

    let mut i = 0;
    while i < HTML_SEQUENCES.len() {
        if (HTML_SEQUENCES[i].0)(&line_text) {
            break;
        }
        i += 1;
    }
    if i == HTML_SEQUENCES.len() {
        return false;
    }

    if silent {
        // true if this sequence can be a terminator, false otherwise
        return HTML_SEQUENCES[i].2;
    }

    let mut next_line = start_line + 1;

    // Block types 6 and 7 (the only ones whose end condition is a blank line)
    // have `/^$/` as their closing regexp. For all other types (1-5, e.g.
    // `<!--` comments), a blank line is regular content and must not terminate
    // the block - it ends only when its closing sequence is found.
    let ends_on_blank_line = (HTML_SEQUENCES[i].1)(&[]);

    // If we are here - we detected HTML block.
    // Let's roll down till block end.
    if !(HTML_SEQUENCES[i].1)(&line_text) {
        // for (; nextLine < endLine; nextLine++)
        while next_line < end_line {
            if state.s_count[next_line] < state.blk_indent {
                // An outdented blank line shouldn't end a block that doesn't end on a
                // blank line (e.g. a `<!--` comment inside a list item). Such blocks
                // must continue until their closing sequence regardless of indent.
                if ends_on_blank_line || !state.is_empty(next_line) {
                    break;
                }
            }

            pos = state.b_marks[next_line] + state.t_shift[next_line];
            max = state.e_marks[next_line];
            line_text = js::slice_to(&state.src, pos, max).to_vec();

            if (HTML_SEQUENCES[i].1)(&line_text) {
                if !line_text.is_empty() {
                    next_line += 1;
                }
                break;
            }
            next_line += 1;
        }
    }

    state.line = next_line;

    // `token` borrows `state`: what JS reads from `state` after the push is read first.
    let content = state.get_lines(start_line, next_line, state.blk_indent, true);
    let token = state.push("html_block", "", 0);
    token.map = Some([start_line, next_line]);
    token.content = js::to_string(&content);

    true
}

#[cfg(test)]
mod tests {
    use super::HTML_SEQUENCES;
    use crate::markdown_it::js;
    use crate::markdown_it::rules_block::paragraph::test_util::{Case, check};

    #[test]
    fn html_sequences_match_node() {
        // node: [HTML_SEQUENCES.map(q => q[0].test(s)), HTML_SEQUENCES.map(q => q[1].test(s))],
        // with HTML_SEQUENCES lifted from the bundle's source
        let cases: &[(&str, [bool; 7], [bool; 7])] = &[
            (
                "<script",
                [true, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<script>",
                [true, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<scriptx",
                [false, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<SCRIPT\t",
                [true, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<pre/",
                [false, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<pre>",
                [true, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<textarea \u{a0}",
                [true, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<style",
                [true, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<div",
                [false, false, false, false, false, true, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "</div",
                [false, false, false, false, false, true, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<DIV>",
                [false, false, false, false, false, true, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<div/>",
                [false, false, false, false, false, true, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<div/",
                [false, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<divx",
                [false, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<colgroup>",
                [false, false, false, false, false, true, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<col>",
                [false, false, false, false, false, true, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<h7>",
                [false, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<h6 x",
                [false, false, false, false, false, true, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "</tr>",
                [false, false, false, false, false, true, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<track>",
                [false, false, false, false, false, true, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<!--x",
                [false, true, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<?",
                [false, false, true, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<!x",
                [false, false, false, true, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<![CDATA[",
                [false, false, false, false, true, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<!",
                [false, false, false, false, false, false, false],
                [false, false, false, false, false, false, false],
            ),
            (
                "<a>",
                [false, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<a> \u{a0}",
                [false, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<a>x",
                [false, false, false, false, false, false, false],
                [false, false, false, true, false, false, false],
            ),
            (
                "</a>\t",
                [false, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "<x-y z>",
                [false, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "x</script>",
                [false, false, false, false, false, false, false],
                [true, false, false, true, false, false, false],
            ),
            (
                "</script>",
                [false, false, false, false, false, false, true],
                [true, false, false, true, false, false, false],
            ),
            (
                "a</PRE>b",
                [false, false, false, false, false, false, false],
                [true, false, false, true, false, false, false],
            ),
            (
                "</style >",
                [false, false, false, false, false, false, true],
                [false, false, false, true, false, false, false],
            ),
            (
                "-->",
                [false, false, false, false, false, false, false],
                [false, true, false, true, false, false, false],
            ),
            (
                "?>",
                [false, false, false, false, false, false, false],
                [false, false, true, true, false, false, false],
            ),
            (
                ">",
                [false, false, false, false, false, false, false],
                [false, false, false, true, false, false, false],
            ),
            (
                "]]>",
                [false, false, false, false, false, false, false],
                [false, false, false, true, true, false, false],
            ),
            (
                "",
                [false, false, false, false, false, false, false],
                [false, false, false, false, false, true, true],
            ),
        ];
        for &(s, open, close) in cases {
            let s16 = js::to_utf16(s);
            assert_eq!(HTML_SEQUENCES.map(|q| (q.0)(&s16)), open, "open {s:?}");
            assert_eq!(HTML_SEQUENCES.map(|q| (q.1)(&s16)), close, "close {s:?}");
        }
    }

    #[test]
    fn html_block_tokens_match_node() {
        // node: see `test_util` (block tokens, then env.references)
        let cases: &[Case] = &[
            (
                "<div>\nfoo\n\nbar\n",
                "{\"html\":true}",
                &[],
                &[
                    "html_block/ [0,2] L0 content=\"<div>\\nfoo\\n\"",
                    "paragraph_open/p [3,4] L0",
                    "inline/ [3,4] L1 content=\"bar\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "<!--\n\n-->\nx\n",
                "{\"html\":true}",
                &[],
                &[
                    "html_block/ [0,3] L0 content=\"<!--\\n\\n-->\\n\"",
                    "paragraph_open/p [3,4] L0",
                    "inline/ [3,4] L1 content=\"x\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "<x-y z>\nfoo\n\na\n<x-y z>\n",
                "{\"html\":true}",
                &[],
                &[
                    "html_block/ [0,2] L0 content=\"<x-y z>\\nfoo\\n\"",
                    "paragraph_open/p [3,5] L0",
                    "inline/ [3,5] L1 content=\"a\\n<x-y z>\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "<DIV class=\"a\">\n<SCRIPT>\n</sCrIpT> x\ny\n",
                "{\"html\":true}",
                &[],
                &[
                    "html_block/ [0,4] L0 content=\"<DIV class=\\\"a\\\">\\n<SCRIPT>\\n</sCrIpT> x\\ny\\n\"",
                ],
            ),
            (
                "- <!--\n\n  -->\n- b\n",
                "{\"html\":true}",
                &[],
                &[
                    "bullet_list_open/ul [0,4] L0 markup=\"-\"",
                    "list_item_open/li [0,3] L1 markup=\"-\"",
                    "html_block/ [0,3] L2 content=\"<!--\\n\\n-->\\n\"",
                    "list_item_close/li L1 markup=\"-\"",
                    "list_item_open/li [3,4] L1 markup=\"-\"",
                    "paragraph_open/p [3,4] L2 hidden",
                    "inline/ [3,4] L3 content=\"b\"",
                    "paragraph_close/p L2 hidden",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "- <div>\n\n  x\n",
                "{\"html\":true}",
                &[],
                &[
                    "bullet_list_open/ul [0,3] L0 markup=\"-\"",
                    "list_item_open/li [0,3] L1 markup=\"-\"",
                    "html_block/ [0,1] L2 content=\"<div>\\n\"",
                    "paragraph_open/p [2,3] L2",
                    "inline/ [2,3] L3 content=\"x\"",
                    "paragraph_close/p L2",
                    "list_item_close/li L1 markup=\"-\"",
                    "bullet_list_close/ul L0 markup=\"-\"",
                ],
            ),
            (
                "<a b=c\u{a0}d>  \nx\n\n<a>x\n",
                "{\"html\":true}",
                &[],
                &[
                    "html_block/ [0,2] L0 content=\"<a b=c\u{a0}d>  \\nx\\n\"",
                    "paragraph_open/p [3,4] L0",
                    "inline/ [3,4] L1 content=\"<a>x\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "<colgroup>\n<col\n<?php\n?>\n<!DOCTYPE x>\n<![CDATA[\n]]>\n",
                "{\"html\":true}",
                &[],
                &[
                    "html_block/ [0,7] L0 content=\"<colgroup>\\n<col\\n<?php\\n?>\\n<!DOCTYPE x>\\n<![CDATA[\\n]]>\\n\"",
                ],
            ),
            (
                "<div>\n",
                "{\"html\":false}",
                &[],
                &[
                    "paragraph_open/p [0,1] L0",
                    "inline/ [0,1] L1 content=\"<div>\"",
                    "paragraph_close/p L0",
                ],
            ),
            (
                "    <div>\n",
                "{\"html\":true,\"allowIndentation\":true}",
                &["code"],
                &["html_block/ [0,1] L0 content=\"    <div>\\n\""],
            ),
        ];
        check(cases);
    }
}
