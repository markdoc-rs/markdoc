//! Port of rules_block/state_block.ts.

use crate::markdown_it::common::utils::is_space;
use crate::markdown_it::token::Token;
use crate::markdown_it::{Env, JsError, MarkdownIt};

/// Mutable state passed to block rules while tokenizing a source document.
///
/// Positions (`b_marks`, `e_marks`, `t_shift`, `line`, ...) are `usize` UTF-16 indices;
/// column counts that rules subtract and compare (`s_count`, `bs_count`, `blk_indent`,
/// `list_indent`, `dd_indent`) are `i64`, since they go negative in JS (`sCount` is set to
/// -1 by blockquote, `listIndent` starts at -1).
#[derive(Debug)]
pub struct StateBlock<'a> {
    /// The source, as UTF-16 code units (JS string).
    pub src: Vec<u16>,
    /// Link to parser instance.
    pub md: &'a MarkdownIt,
    pub env: &'a mut Env,

    //
    // Internal state variables
    //
    pub tokens: Vec<Token>,

    /// Line begin offsets for fast jumps.
    pub b_marks: Vec<usize>,
    /// Line end offsets for fast jumps.
    pub e_marks: Vec<usize>,
    /// Offsets of the first non-space characters (tabs not expanded).
    pub t_shift: Vec<usize>,
    /// Indents for each line (tabs expanded).
    pub s_count: Vec<i64>,

    /// An amount of virtual spaces (tabs expanded) between beginning
    /// of each line (bMarks) and real beginning of that line.
    ///
    /// It exists only as a hack because blockquotes override bMarks
    /// losing information in the process.
    ///
    /// It's used only when expanding tabs, you can think about it as
    /// an initial tab length, e.g. bsCount=21 applied to string `\t123`
    /// means first tab should be expanded to 4-21%4 === 3 spaces.
    pub bs_count: Vec<i64>,

    // block parser variables
    /// Required block content indent (for example, if we are
    /// inside a list, it would be positioned after list marker).
    pub blk_indent: i64,
    /// Line index in src.
    pub line: usize,
    /// Lines count.
    pub line_max: usize,
    /// Loose/tight mode for lists.
    pub tight: bool,
    /// Not a field of 15.0.1's StateBlock (markdown-it 13 had `ddIndent = -1`); kept, at -1,
    /// for plugins such as definition lists that set it. No engine rule reads it.
    pub dd_indent: i64,
    /// Indent of the current list block (-1 if there isn't any).
    pub list_indent: i64,

    /// Can be 'blockquote', 'list', 'root', 'paragraph' or 'reference'
    /// used in lists to determine if they interrupt a paragraph.
    pub parent_type: &'static str,

    pub level: i32,
}

impl<'a> StateBlock<'a> {
    pub fn new(src: Vec<u16>, md: &'a MarkdownIt, env: &'a mut Env, tokens: Vec<Token>) -> Self {
        let mut state = StateBlock {
            src,
            md,
            env,
            tokens,
            b_marks: Vec::new(),
            e_marks: Vec::new(),
            t_shift: Vec::new(),
            s_count: Vec::new(),
            bs_count: Vec::new(),
            blk_indent: 0,
            line: 0,
            line_max: 0,
            tight: false,
            dd_indent: -1,
            list_indent: -1,
            parent_type: "root",
            level: 0,
        };

        // Create caches
        // Generate markers.
        let s = &state.src;
        let len = s.len();
        let (mut start, mut pos, mut indent, mut offset, mut indent_found) =
            (0usize, 0usize, 0usize, 0i64, false);

        while pos < len {
            let ch = s[pos];

            if !indent_found {
                if is_space(ch) {
                    indent += 1;

                    if ch == 0x09 {
                        offset += 4 - offset % 4;
                    } else {
                        offset += 1;
                    }
                    pos += 1;
                    continue;
                } else {
                    indent_found = true;
                }
            }

            if ch == 0x0A || pos == len - 1 {
                if ch != 0x0A {
                    pos += 1;
                }
                state.b_marks.push(start);
                state.e_marks.push(pos);
                state.t_shift.push(indent);
                state.s_count.push(offset);
                state.bs_count.push(0);

                indent_found = false;
                indent = 0;
                offset = 0;
                start = pos + 1;
            }
            pos += 1;
        }

        // Push fake entry to simplify cache bounds checks
        state.b_marks.push(len);
        state.e_marks.push(len);
        state.t_shift.push(0);
        state.s_count.push(0);
        state.bs_count.push(0);

        state.line_max = state.b_marks.len() - 1; // don't count last fake line
        state
    }

    /// `push(type, tag, nesting)`: push a new block token to the stream.
    pub fn push(&mut self, kind: &str, tag: &str, nesting: i8) -> &mut Token {
        let mut token = Token::new(kind, tag, nesting);
        token.block = true;

        if nesting < 0 {
            self.level -= 1; // closing tag
        }
        token.level = self.level;
        if nesting > 0 {
            self.level += 1; // opening tag
        }

        self.tokens.push(token);
        self.tokens.last_mut().expect("just pushed")
    }

    /// `isEmpty(line)`. A line past the marks is not empty (JS compares `NaN`).
    pub fn is_empty(&self, line: usize) -> bool {
        match (
            self.b_marks.get(line),
            self.t_shift.get(line),
            self.e_marks.get(line),
        ) {
            (Some(&b), Some(&t), Some(&e)) => b + t >= e,
            _ => false,
        }
    }

    /// `skipEmptyLines(from)`.
    pub fn skip_empty_lines(&self, mut from: usize) -> usize {
        let max = self.line_max;
        while from < max {
            if self.b_marks[from] + self.t_shift[from] < self.e_marks[from] {
                break;
            }
            from += 1;
        }
        from
    }

    /// `skipSpaces(pos)`: skip spaces from given position.
    pub fn skip_spaces(&self, mut pos: usize) -> usize {
        let max = self.src.len();
        while pos < max {
            let ch = self.src[pos];
            if !is_space(ch) {
                break;
            }
            pos += 1;
        }
        pos
    }

    /// `skipSpacesBack(pos, min)`: skip spaces from given position in reverse.
    pub fn skip_spaces_back(&self, mut pos: usize, min: usize) -> usize {
        if pos <= min {
            return pos;
        }

        while pos > min {
            pos -= 1;
            if !self.src.get(pos).is_some_and(|&c| is_space(c)) {
                return pos + 1;
            }
        }
        pos
    }

    /// `skipChars(pos, code)`: skip char codes from given position.
    pub fn skip_chars(&self, mut pos: usize, code: u16) -> usize {
        let max = self.src.len();
        while pos < max {
            if self.src[pos] != code {
                break;
            }
            pos += 1;
        }
        pos
    }

    /// `skipCharsBack(pos, code, min)`: skip char codes reverse from given position - 1.
    pub fn skip_chars_back(&self, mut pos: usize, code: u16, min: usize) -> usize {
        if pos <= min {
            return pos;
        }

        while pos > min {
            pos -= 1;
            if self.src.get(pos) != Some(&code) {
                return pos + 1;
            }
        }
        pos
    }

    /// `getLines(begin, end, indent, keepLastLF)`: cut lines range from source.
    ///
    /// Where JS throws for a size no document reaches (`new Array(n)` of more than 2^32 - 1
    /// elements: `RangeError: Invalid array length`; a string longer than V8's 2^29 - 24
    /// code units: `RangeError: Invalid string length`), the error is recorded on the
    /// environment and the lines are empty. Both are known before anything is built.
    pub fn get_lines(
        &mut self,
        begin: usize,
        end: usize,
        indent: i64,
        keep_last_lf: bool,
    ) -> Vec<u16> {
        if begin >= end {
            return Vec::new();
        }
        match self.lines_length(begin, end, indent, keep_last_lf) {
            Ok(length) => self.cut_lines(begin, end, indent, keep_last_lf, length),
            Err(error) => {
                self.env.throw(error);
                Vec::new()
            }
        }
    }

    /// Where line `line` (one with marks) starts once `indent` columns are taken off it, how
    /// many columns that took, and where it ends: getLines' loop body up to the slice.
    fn line_cut(
        &self,
        line: usize,
        end: usize,
        indent: i64,
        keep_last_lf: bool,
    ) -> (usize, i64, usize) {
        let line_start = self.b_marks[line];
        let mut line_indent: i64 = 0;
        let mut first = line_start;
        let last = if line + 1 < end || keep_last_lf {
            // No need for bounds check because we have fake entry on tail.
            self.e_marks[line] + 1
        } else {
            self.e_marks[line]
        };

        while first < last && line_indent < indent {
            // `ch` is NaN past the end of src (`last` can be `src.length + 1`): not a space
            match self.src.get(first) {
                Some(&ch) if is_space(ch) => {
                    if ch == 0x09 {
                        line_indent += 4 - (line_indent + self.bs_count[line]) % 4;
                    } else {
                        line_indent += 1;
                    }
                }
                _ if first - line_start < self.t_shift[line] => {
                    // patched tShift masked characters to look like spaces (blockquotes, list markers)
                    line_indent += 1;
                }
                _ => break,
            }

            first += 1;
        }
        (first, line_indent, last)
    }

    /// The length of what getLines returns, or what JS throws building it (each check
    /// where JS makes it: the queue, then each line's padding and line, then the join).
    fn lines_length(
        &self,
        begin: usize,
        end: usize,
        indent: i64,
        keep_last_lf: bool,
    ) -> Result<usize, JsError> {
        let array = || JsError::range_error("Invalid array length");
        let string = || JsError::range_error("Invalid string length");
        // `new Array(n)` takes up to 2^32 - 1 elements, `n` spaces are `new Array(n + 1)`
        let padding = |line_indent: i64| -> Result<i128, JsError> {
            let n = i128::from(line_indent) - i128::from(indent);
            if n <= 0 {
                return Ok(0);
            }
            if n + 1 > MAX_ARRAY_LENGTH {
                return Err(array());
            }
            if n > MAX_STRING_LENGTH {
                return Err(string());
            }
            Ok(n)
        };
        if (end - begin) as u128 > MAX_ARRAY_LENGTH as u128 {
            return Err(array());
        }
        let mut total: i128 = 0;
        let marked = end.min(self.b_marks.len());
        for line in begin..marked {
            let (first, line_indent, last) = self.line_cut(line, end, indent, keep_last_lf);
            let piece = padding(line_indent)? + js_slice(&self.src, first, last).len() as i128;
            if piece > MAX_STRING_LENGTH {
                return Err(string());
            }
            total += piece;
        }
        // Past the marks (upstream oddity, kept): `bMarks[line]` and `eMarks[line]` are
        // undefined, so no indent is consumed, a negative `indent` still pads, and
        // `src.slice(undefined, NaN)` is '' while, on the last line without keepLastLF,
        // `src.slice(undefined, undefined)` is the whole source.
        if end > marked {
            let lines = (end - begin.max(marked)) as i128;
            let pad = padding(0)?;
            let whole = if keep_last_lf {
                0
            } else {
                self.src.len() as i128
            };
            if pad + whole > MAX_STRING_LENGTH {
                return Err(string());
            }
            total += lines * pad + whole;
        }
        if total > MAX_STRING_LENGTH {
            return Err(string());
        }
        Ok(total as usize)
    }

    fn cut_lines(
        &self,
        begin: usize,
        end: usize,
        indent: i64,
        keep_last_lf: bool,
        length: usize,
    ) -> Vec<u16> {
        let mut queue: Vec<u16> = Vec::with_capacity(length);
        let marked = end.min(self.b_marks.len());
        for line in begin..marked {
            let (first, line_indent, last) = self.line_cut(line, end, indent, keep_last_lf);
            if line_indent > indent {
                // partially expanding tabs in code blocks, e.g '\t\tfoobar'
                // with indent=2 becomes '  \tfoobar'
                queue.extend(std::iter::repeat_n(0x20, (line_indent - indent) as usize));
            }
            queue.extend_from_slice(js_slice(&self.src, first, last));
        }
        if end > marked {
            if 0 > indent {
                let lines = end - begin.max(marked);
                queue.extend(std::iter::repeat_n(
                    0x20,
                    lines * indent.unsigned_abs() as usize,
                ));
            }
            if !keep_last_lf {
                queue.extend_from_slice(&self.src);
            }
        }
        queue
    }
}

/// V8's longest array, `new Array(n)`: 2^32 - 1 elements.
const MAX_ARRAY_LENGTH: i128 = (1 << 32) - 1;

/// V8's longest string on 64-bit hosts (node's `buffer.constants.MAX_STRING_LENGTH`):
/// 2^29 - 24 code units.
const MAX_STRING_LENGTH: i128 = (1 << 29) - 24;

/// `src.slice(first, last)` with non-negative bounds.
fn js_slice(src: &[u16], first: usize, last: usize) -> &[u16] {
    crate::markdown_it::js::slice_to(src, first, last)
}

#[cfg(test)]
#[allow(clippy::type_complexity)]
mod tests {
    use super::*;
    use crate::markdown_it::js;

    fn u(s: &str) -> Vec<u16> {
        js::to_utf16(s)
    }

    #[test]
    fn line_marks_match_state_block_ts() {
        // node: new md.block.State(src, md, {}, []) -> [bMarks, eMarks, tShift, sCount, bsCount, lineMax]
        let md = MarkdownIt::default();
        let cases: [(&str, &[usize], &[usize], &[usize], &[i64], usize); 7] = [
            ("", &[0], &[0], &[0], &[0], 0),
            ("a", &[0, 1], &[1, 1], &[0, 0], &[0, 0], 1),
            ("a\n", &[0, 2], &[1, 2], &[0, 0], &[0, 0], 1),
            (
                "  a\n\tb\n \t c",
                &[0, 4, 7, 11],
                &[3, 6, 11, 11],
                &[2, 1, 3, 0],
                &[2, 4, 5, 0],
                3,
            ),
            // a last line of only spaces without a newline is dropped
            ("x\n   ", &[0, 5], &[1, 5], &[0, 0], &[0, 0], 1),
            ("\n\n", &[0, 1, 2], &[0, 1, 2], &[0, 0, 0], &[0, 0, 0], 2),
            ("> q\n", &[0, 4], &[3, 4], &[0, 0], &[0, 0], 1),
        ];
        for (src, b, e, t, s, line_max) in cases {
            let mut env = Env::default();
            let st = StateBlock::new(u(src), &md, &mut env, Vec::new());
            assert_eq!(
                (
                    &st.b_marks[..],
                    &st.e_marks[..],
                    &st.t_shift[..],
                    &st.s_count[..],
                    st.line_max
                ),
                (b, e, t, s, line_max),
                "{src:?}"
            );
            assert!(st.bs_count.iter().all(|&x| x == 0));
        }
    }

    #[test]
    fn get_lines_skip_helpers_and_push_match_state_block_ts() {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateBlock::new(u("  ab\n\t\tcd\n   \n>  ef\n"), &md, &mut env, Vec::new());
        // node: s.getLines(begin, end, indent, keepLastLF)
        let cases: [(usize, usize, i64, bool, &str); 9] = [
            (0, 2, 0, false, "  ab\n\t\tcd"),
            (0, 2, 2, false, "ab\n  \tcd"),
            (1, 2, 2, true, "  \tcd\n"),
            (1, 2, 6, false, "  cd"),
            (0, 4, 1, true, " ab\n   \tcd\n  \n>  ef\n"),
            (3, 3, 0, false, ""),
            (2, 4, 3, false, "\n>  ef"),
            // past the marks: the last line without keepLastLF is the whole source again
            (
                0,
                9,
                0,
                false,
                "  ab\n\t\tcd\n   \n>  ef\n  ab\n\t\tcd\n   \n>  ef\n",
            ),
            (0, 9, 0, true, "  ab\n\t\tcd\n   \n>  ef\n"),
        ];
        for (begin, end, indent, keep, want) in cases {
            assert_eq!(
                js::to_string(&st.get_lines(begin, end, indent, keep)),
                want,
                "{begin} {end} {indent} {keep}"
            );
        }
        // tShift masking (blockquote) and bsCount tab offsets
        st.t_shift[3] = 1;
        st.bs_count[1] = 1;
        assert_eq!(js::to_string(&st.get_lines(3, 4, 3, false)), "ef");
        assert_eq!(js::to_string(&st.get_lines(1, 2, 3, false)), "\tcd");
        // node: [skipSpaces(0), skipSpaces(2), skipSpacesBack(2,0), skipSpacesBack(4,2), skipSpacesBack(0,0),
        //        skipChars(5,9), skipCharsBack(7,9,5), skipCharsBack(5,9,5), isEmpty(2), isEmpty(0), isEmpty(99),
        //        skipEmptyLines(2), skipEmptyLines(4)]
        assert_eq!(
            (
                st.skip_spaces(0),
                st.skip_spaces(2),
                st.skip_spaces_back(2, 0),
                st.skip_spaces_back(4, 2)
            ),
            (2, 2, 0, 4)
        );
        assert_eq!(
            (
                st.skip_spaces_back(0, 0),
                st.skip_chars(5, 9),
                st.skip_chars_back(7, 9, 5),
                st.skip_chars_back(5, 9, 5)
            ),
            (0, 7, 5, 5)
        );
        assert_eq!(
            (st.is_empty(2), st.is_empty(0), st.is_empty(99)),
            (true, false, false)
        );
        assert_eq!((st.skip_empty_lines(2), st.skip_empty_lines(4)), (3, 4));
        // node: levels of push('x_open','x',1), push('y','',0), push('x_close','x',-1), then state.level
        let levels = [
            st.push("x_open", "x", 1).level,
            st.push("y", "", 0).level,
            st.push("x_close", "x", -1).level,
        ];
        assert_eq!((levels, st.level, st.tokens[0].block), ([0, 1, 0], 0, true));
    }
}
