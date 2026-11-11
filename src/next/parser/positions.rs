//! Where each token is in the source: the byte spans behind this crate's positions.
//!
//! Upstream's tokens say little about where they are: a block token has a line map
//! (`map`, 0-based, end exclusive), an inline token has nothing, and a tag Markdoc finds in
//! a fence has a line and a 1-based column in the fence's content. The rest is found again
//! in the source the tokens were made from, by reading it as markdown-it read it:
//!
//! - **Lines** are markdown-it's (`\r\n`, `\r` and `\n` end one), so a map's line numbers
//!   index them directly.
//! - **Blocks** start at their own marker on the first line of their map, searched from
//!   where the enclosing containers' markers end on that line: a blockquote takes its `>`
//!   on every line it spans, a list item its marker on its first line. A Markdoc tag spans
//!   its `{%` to the `%}` that `findTagEnd` finds, as the tokenizer's block rule does. A
//!   block ends where the last line of its map does, line ending included.
//! - **Inline content** (a paragraph's, a heading's, a table cell's `content`) is markdown-it's
//!   lines with the container prefixes and indentation stripped, joined with `\n`, and
//!   trimmed. Each line of it is matched back to its source line: a paragraph's lines are
//!   suffixes of theirs (the last one before trailing spaces and tabs), a heading's text
//!   follows its `#`s, a cell is found by splitting the row as the table rule does (over
//!   markdown-it's normalised text, where NUL is U+FFFD; the `|` of an escaped pipe, the only
//!   one a cell keeps, stands at its backslash, so what ends before it ends before the
//!   backslash). That gives every UTF-16 unit of the content a byte offset.
//! - **Inline tokens** are walked in order with a cursor in the content. Each one's extent
//!   comes from its own syntax: a delimiter's `markup`, a tag's `{% %}` (`findTagEnd`
//!   again), a code span's backtick runs (an escaped backtick before one is text, not part
//!   of the run), a link's `]` and destination (markdown-it's own helpers), a break's
//!   newline. A text token's extent is found by producing its `content` from the source as
//!   the inline rules do (escapes, entities, and the typographer's quotes and replacements),
//!   and where that cannot be done the next token's start, searched for, ends it.
//! - **Fence children** (Markdoc's pseudo-tokens) are slices of the fence's content: text
//!   verbatim, tags by their `{% %}` text.
//!
//! Nothing here panics or indexes outside the source: every offset is on a character
//! boundary of the source and every span has `start <= end`.

use crate::markdown_it::common::utils::{
    decode_html_strict, from_code_point, is_space, is_valid_entity_code,
};
use crate::markdown_it::helpers::{parse_link_destination, parse_link_title};
use crate::markdown_it::{MarkdownIt, Token, js};
use std::sync::OnceLock;

/// A byte range of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Extent {
    pub start: usize,
    pub end: usize,
}

impl Extent {
    /// The empty span at `offset`.
    pub(crate) fn at(offset: usize) -> Extent {
        Extent {
            start: offset,
            end: offset,
        }
    }

    fn new(start: usize, end: usize) -> Extent {
        Extent {
            start,
            end: end.max(start),
        }
    }
}

/// One of markdown-it's lines: `start..end` is its text, `next` where the next one begins.
#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    end: usize,
    next: usize,
}

/// markdown-it's lines: after its normalisation (`\r\n?` to `\n`) each `\n` ends one, and
/// text after the last one is a line of its own. The first `limit` of them.
fn split_lines(src: &str, limit: usize) -> Vec<Line> {
    let bytes = src.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() && lines.len() < limit {
        match bytes[i] {
            b'\n' => {
                lines.push(Line {
                    start,
                    end: i,
                    next: i + 1,
                });
                start = i + 1;
                i += 1;
            }
            b'\r' => {
                let next = if bytes.get(i + 1) == Some(&b'\n') {
                    i + 2
                } else {
                    i + 1
                };
                lines.push(Line {
                    start,
                    end: i,
                    next,
                });
                start = next;
                i = next;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() && lines.len() < limit {
        lines.push(Line {
            start,
            end: bytes.len(),
            next: bytes.len(),
        });
    }
    lines
}

/// The YAML of a frontmatter token of `src`: its lines between the fences, untrimmed, as the
/// old parser reads them (`None` without a usable map). Only the lines up to the closing
/// fence are looked at, so it costs nothing with spans off.
pub(crate) fn frontmatter_text(src: &str, token: &Token) -> Option<String> {
    let [_, close] = token.map?;
    let lines = split_lines(src, close);
    if close > lines.len() || close == 0 {
        return None;
    }
    Some(
        lines[1..close]
            .iter()
            .map(|l| &src[l.start..l.end])
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn utf16(s: &str) -> Vec<u16> {
    js::to_utf16(s)
}

fn is_high_surrogate(u: u16) -> bool {
    (0xD800..=0xDBFF).contains(&u)
}

fn is_low_surrogate(u: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&u)
}

/// The code point at `units[i]`, and how many units it takes.
fn code_point_at(units: &[u16], i: usize) -> (u32, usize) {
    let u = units[i];
    if is_high_surrogate(u) && i + 1 < units.len() && is_low_surrogate(units[i + 1]) {
        let cp = 0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(units[i + 1]) - 0xDC00);
        (cp, 2)
    } else {
        (u32::from(u), 1)
    }
}

/// The code point ending at `units[i - 1]`, and how many units it takes.
fn code_point_before(units: &[u16], i: usize) -> (u32, usize) {
    let u = units[i - 1];
    if is_low_surrogate(u) && i >= 2 && is_high_surrogate(units[i - 2]) {
        code_point_at(units, i - 2)
    } else {
        (u32::from(u), 1)
    }
}

/// A source character as markdown-it sees it (its normalisation makes NUL U+FFFD).
fn normalized(c: char) -> u32 {
    if c == '\0' { 0xFFFD } else { u32::from(c) }
}

/// `findTagEnd(content, start)` (Markdoc's `utils.ts`): where the `%}` closing a tag that
/// opens at `start` is, skipping strings.
fn find_tag_end(content: &[u16], start: usize) -> Option<usize> {
    #[derive(PartialEq)]
    enum State {
        Normal,
        String,
        Escape,
    }
    let mut state = State::Normal;
    for pos in start..content.len() {
        let char = content[pos];
        match state {
            State::String => match char {
                0x22 => state = State::Normal,
                0x5C => state = State::Escape,
                _ => {}
            },
            State::Escape => state = State::String,
            State::Normal => {
                if char == 0x22 {
                    state = State::String;
                } else if char == 0x25 && content.get(pos + 1) == Some(&0x7D) {
                    return Some(pos);
                }
            }
        }
    }
    None
}

/// [`find_tag_end`] over the source's bytes. The states change only on ASCII, and an
/// escape skips the first byte of a character whose other bytes change nothing, so this
/// finds the `%}` the UTF-16 scan finds.
fn find_tag_end_bytes(src: &[u8], start: usize) -> Option<usize> {
    let mut in_string = false;
    let mut escape = false;
    for pos in start..src.len() {
        let b = src[pos];
        if escape {
            escape = false;
        } else if in_string {
            match b {
                b'"' => in_string = false,
                b'\\' => escape = true,
                _ => {}
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b'%' && src.get(pos + 1) == Some(&b'}') {
            return Some(pos);
        }
    }
    None
}

/// The engine, for `normalizeLink` and `validateLink` (they read nothing of the instance).
fn engine() -> &'static MarkdownIt {
    static ENGINE: OnceLock<MarkdownIt> = OnceLock::new();
    ENGINE.get_or_init(MarkdownIt::default)
}

/// One cell of a table row: its text between the pipes, and its content (trimmed, escaped
/// pipes unescaped) with each unit's byte offset and the end's.
#[derive(Debug, Clone)]
struct Cell {
    raw: Extent,
    text: Vec<u16>,
    map: Vec<usize>,
}

/// An open block token, for its close.
#[derive(Debug, Clone)]
struct Open {
    kind: String,
    span: Extent,
    /// The cell of the current row a `th`/`td` is.
    cell: Option<usize>,
}

/// The positions of one token stream in its source; [`Positions::token`] is called for each
/// top-level token in order.
pub(crate) struct Positions<'a> {
    src: &'a str,
    lines: Vec<Line>,
    /// Per line, where what the enclosing containers' markers took ends.
    cursors: Vec<usize>,
    open: Vec<Open>,
    /// After the `#`s of the heading just opened.
    heading_text: Option<usize>,
    /// The content map a paragraph computed for the inline token after it.
    pending: Option<(usize, Vec<usize>)>,
    /// The cells of the table row just opened.
    row: Vec<Cell>,
    next_cell: usize,
    /// The end of the last span given.
    last: usize,
}

impl<'a> Positions<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        let lines = split_lines(src, usize::MAX);
        let cursors = lines.iter().map(|l| l.start).collect();
        Positions {
            src,
            lines,
            cursors,
            open: Vec::new(),
            heading_text: None,
            pending: None,
            row: Vec::new(),
            next_cell: 0,
            last: 0,
        }
    }

    /// The span of `tokens[i]`, and of its children when it has any.
    pub(crate) fn token(&mut self, tokens: &[Token], i: usize) -> (Extent, Option<Vec<Extent>>) {
        let (span, children) = self.dispatch(tokens, i);
        let span = self.clamp(span);
        self.last = span.end;
        (span, children)
    }

    // --- the source --------------------------------------------------------------------

    fn line(&self, line: usize) -> Option<Line> {
        self.lines.get(line).copied()
    }

    /// An offset made safe: within the source and on a character boundary.
    fn snap(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.src.len());
        while !self.src.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn clamp(&self, span: Extent) -> Extent {
        Extent::new(self.snap(span.start), self.snap(span.end))
    }

    /// From `from`, past spaces and tabs, no further than `to`.
    fn skip_blanks(&self, from: usize, to: usize) -> usize {
        let bytes = self.src.as_bytes();
        let mut p = from;
        while p < to && (bytes[p] == b' ' || bytes[p] == b'\t') {
            p += 1;
        }
        p
    }

    /// From `from`, past JS whitespace (`String.prototype.trim`'s), no further than `to`.
    fn skip_js_space(&self, from: usize, to: usize) -> usize {
        let mut p = from;
        for c in self.src[from..to].chars() {
            if !js::is_js_space(u32::from(c)) {
                break;
            }
            p += c.len_utf8();
        }
        p
    }

    /// Back from `to`, past JS whitespace, no further than `from`.
    fn trim_js_space_end(&self, from: usize, to: usize) -> usize {
        let mut p = to;
        for c in self.src[from..to].chars().rev() {
            if !js::is_js_space(u32::from(c)) {
                break;
            }
            p -= c.len_utf8();
        }
        p
    }

    /// Back from `to`, past spaces and tabs (markdown-it's `asciiTrim` within a line, where
    /// no `\r` or `\n` is), no further than `from`.
    fn trim_blanks_end(&self, from: usize, to: usize) -> usize {
        let bytes = self.src.as_bytes();
        let mut p = to;
        while p > from && (bytes[p - 1] == b' ' || bytes[p - 1] == b'\t') {
            p -= 1;
        }
        p
    }

    /// Where a block's text on `line` begins: the first character after what the enclosing
    /// containers took, spaces and tabs skipped.
    fn first_char(&self, line: usize) -> usize {
        match self.line(line) {
            Some(l) => self.skip_blanks(self.cursors[line].max(l.start), l.end),
            None => self.src.len(),
        }
    }

    /// Where `needle` (a block's marker) is on `line`: first after spaces and tabs, else
    /// anywhere after what the containers took.
    fn find_marker(&self, line: usize, needle: &str) -> Option<usize> {
        let l = self.line(line)?;
        let from = self.cursors[line].max(l.start);
        let p = self.skip_blanks(from, l.end);
        if !needle.is_empty() && self.src[p..l.end].starts_with(needle) {
            return Some(p);
        }
        if needle.is_empty() {
            return None;
        }
        self.src[from..l.end].find(needle).map(|i| from + i)
    }

    /// The end of a block whose map is `[s, e]`: its last line, line ending included.
    fn block_end(&self, s: usize, e: usize) -> usize {
        let last = e.saturating_sub(1).max(s);
        match self.line(last) {
            Some(l) => l.next,
            None => self.src.len(),
        }
    }

    // --- mapping content back to the source --------------------------------------------

    /// `piece`, the end of a source line as markdown-it saw it, matched from the right
    /// against `self.src[lo..hi]`: each unit's byte offset. Units left over at the head
    /// (a tab markdown-it expanded into spaces) take the offsets of what precedes, by count.
    fn align_right(&self, piece: &[u16], lo: usize, hi: usize) -> (Vec<usize>, bool) {
        let mut out = vec![lo; piece.len()];
        let (mut i, mut b) = (piece.len(), hi);
        let mut exact = true;
        while i > 0 {
            let (cp, units) = code_point_before(piece, i);
            let Some(c) = self.src[lo..b].chars().next_back() else {
                break;
            };
            if normalized(c) != cp {
                if exact && !piece[..i].iter().all(|&u| u == 0x20) {
                    exact = false;
                }
                if exact {
                    // The head of spaces a partially expanded tab became: all of it at the
                    // tab.
                    let at = b - c.len_utf8();
                    for slot in &mut out[..i] {
                        *slot = at;
                    }
                    return (out, true);
                }
            }
            b -= c.len_utf8();
            for slot in &mut out[i - units..i] {
                *slot = b;
            }
            i -= units;
        }
        if i > 0 {
            exact = false;
        }
        (out, exact)
    }

    /// `piece` laid forward from `from`, unit by unit against the source's characters, no
    /// further than `to`.
    fn align_forward(&self, piece: &[u16], from: usize, to: usize) -> Vec<usize> {
        let mut out = Vec::with_capacity(piece.len() + 1);
        let mut chars = self.src[from..to].char_indices();
        let mut i = 0;
        while i < piece.len() {
            let (_, units) = code_point_at(piece, i);
            let at = match chars.next() {
                Some((k, _)) => from + k,
                None => to,
            };
            for _ in 0..units {
                out.push(at);
            }
            i += units;
        }
        out.push(match chars.next() {
            Some((k, _)) => from + k,
            None => to,
        });
        out.truncate(piece.len() + 1);
        out
    }

    /// Does the source at `at` (before `to`) read `piece`?
    fn reads(&self, piece: &[u16], at: usize, to: usize) -> bool {
        let mut chars = self.src[at..to].chars();
        let mut i = 0;
        while i < piece.len() {
            let (cp, units) = code_point_at(piece, i);
            match chars.next() {
                Some(c) if normalized(c) == cp => i += units,
                _ => return false,
            }
        }
        true
    }

    /// A paragraph's content (`asciiTrim(getLines(...))` of the lines `s..e`): each
    /// `\n`-joined piece matched to the end of its line, the last before trailing spaces and
    /// tabs. Should the pieces be fewer than the lines, where they start is tried. Returns
    /// each unit's offset and the end's.
    fn paragraph_map(&self, content: &[u16], s: usize, e: usize) -> Vec<usize> {
        let pieces = js::split(content, &[0x0A]);
        let available = e.min(self.lines.len()).saturating_sub(s);
        let k = pieces.len();
        let mut first = 0;
        if k < available {
            for a in 0..=(available - k) {
                if pieces
                    .iter()
                    .enumerate()
                    .all(|(j, p)| self.piece_fits(p, s + a + j, j + 1 == k))
                {
                    first = a;
                    break;
                }
            }
        }
        let mut map = Vec::with_capacity(content.len() + 1);
        let mut end = self.lines.get(s).map_or(self.src.len(), |l| l.start);
        for (j, piece) in pieces.iter().enumerate() {
            let line = s + first + j;
            let Some(l) = self.line(line) else {
                // More content than lines: what is left sits at the end.
                map.extend(std::iter::repeat_n(self.src.len(), piece.len()));
                if j + 1 < k {
                    map.push(self.src.len());
                }
                end = self.src.len();
                continue;
            };
            let hi = if j + 1 == k {
                self.trim_blanks_end(l.start, l.end)
            } else {
                l.end
            };
            let (offsets, _) = self.align_right(piece, l.start, hi);
            map.extend(offsets);
            if j + 1 < k {
                map.push(l.end);
            }
            end = hi;
        }
        map.push(end);
        map
    }

    fn piece_fits(&self, piece: &[u16], line: usize, last: bool) -> bool {
        let Some(l) = self.line(line) else {
            return false;
        };
        let hi = if last {
            self.trim_blanks_end(l.start, l.end)
        } else {
            l.end
        };
        self.align_right(piece, l.start, hi).1
    }

    /// An ATX heading's content (`asciiTrim` of what follows the `#`s): on its line, after
    /// the `#`s and spaces.
    fn heading_map(&self, content: &[u16], line: usize) -> Vec<usize> {
        let Some(l) = self.line(line) else {
            return vec![self.src.len(); content.len() + 1];
        };
        let from = self
            .heading_text
            .filter(|&h| h >= l.start && h <= l.end)
            .unwrap_or_else(|| self.first_char(line));
        let start = self.skip_blanks(from, l.end);
        let at = if self.reads(content, start, l.end) {
            start
        } else {
            let mut found = None;
            for (k, _) in self.src[from..l.end].char_indices() {
                if self.reads(content, from + k, l.end) {
                    found = Some(from + k);
                    break;
                }
            }
            found.unwrap_or(start)
        };
        self.align_forward(content, at, l.end)
    }

    /// A fence's content (`getLines(s + 1, close, indent, true)`): each line of it the end
    /// of its source line.
    fn fence_map(&self, content: &[u16], first_line: usize) -> Vec<usize> {
        let pieces = js::split(content, &[0x0A]);
        let mut map = Vec::with_capacity(content.len() + 1);
        let mut end = self.line(first_line).map_or(self.src.len(), |l| l.start);
        for (j, piece) in pieces.iter().enumerate() {
            let last = j + 1 == pieces.len();
            let Some(l) = self.line(first_line + j) else {
                map.extend(std::iter::repeat_n(self.src.len(), piece.len()));
                if !last {
                    map.push(self.src.len());
                }
                end = self.src.len();
                continue;
            };
            if last && piece.is_empty() && j > 0 {
                // After the content's final `\n`: where the previous line's ending ends.
                break;
            }
            let (offsets, _) = self.align_right(piece, l.start, l.end);
            map.extend(offsets);
            if !last {
                map.push(l.end);
                end = l.next;
            } else {
                end = l.end;
            }
        }
        map.push(end);
        map
    }

    // --- table rows ---------------------------------------------------------------------

    /// A row's cells, split from its line as the table rule splits `getLine(...).trim()`.
    /// Where that line's text begins is not in the tokens (a blockquote's or a list item's
    /// marker may precede it), so each place it could begin is tried until the cells come
    /// out as the tokens have them.
    fn row_layout(&self, tokens: &[Token], i: usize, line: usize) -> Vec<Cell> {
        let mut contents = Vec::new();
        for t in &tokens[i + 1..] {
            if t.kind == "tr_close" {
                break;
            }
            if t.kind == "inline" {
                contents.push(utf16(&t.content));
            }
        }
        let Some(l) = self.line(line) else {
            return Vec::new();
        };
        let from = self.cursors[line].max(l.start);
        let mut first_try: Option<Vec<Cell>> = None;
        for (k, c) in self.src[from..l.end].char_indices() {
            if c == ' ' || c == '\t' {
                continue;
            }
            let cells = self.split_row(from + k, l.end);
            let fits = contents.iter().enumerate().all(|(i, want)| {
                cells
                    .get(i)
                    .map_or(want.is_empty(), |cell| &cell.text == want)
            });
            if fits {
                return self.fit_row(cells, contents.len(), l.end);
            }
            if first_try.is_none() {
                first_try = Some(cells);
            }
            // Only a blockquote's or a list item's marker can come before the row's text.
            if !matches!(c, '>' | '-' | '*' | '+' | '.' | ')' | '0'..='9') {
                break;
            }
        }
        let cells = first_try.unwrap_or_else(|| self.split_row(from, l.end));
        self.fit_row(cells, contents.len(), l.end)
    }

    fn fit_row(&self, mut cells: Vec<Cell>, count: usize, end: usize) -> Vec<Cell> {
        let at = cells.last().map_or(end, |c| c.raw.end);
        while cells.len() < count {
            cells.push(Cell {
                raw: Extent::at(at),
                text: Vec::new(),
                map: vec![at],
            });
        }
        cells
    }

    /// `escapedSplit(lineText)` of `src[from..to].trim()`, with the first and last column
    /// dropped when empty, each column trimmed: the cells, with their offsets. The text is
    /// markdown-it's, whose source is normalised (NUL is U+FFFD); an escaped pipe's `|` stands
    /// where its backslash is, so that what ends before the pipe ends before the backslash.
    fn split_row(&self, from: usize, to: usize) -> Vec<Cell> {
        let start = self.skip_js_space(from, to);
        let end = self.trim_js_space_end(start, to);
        let mut units: Vec<u16> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();
        for (k, c) in self.src[start..end].char_indices() {
            let mut buf = [0u16; 2];
            let c = if c == '\0' { '\u{FFFD}' } else { c };
            for u in c.encode_utf16(&mut buf) {
                units.push(*u);
                offsets.push(start + k);
            }
        }
        offsets.push(end);

        // (units, offsets, raw start, raw end) per column
        let mut columns: Vec<(Vec<u16>, Vec<usize>, usize, usize)> = Vec::new();
        let max = units.len();
        let mut pos = 0;
        let mut is_escaped = false;
        let mut last_pos = 0;
        let mut current: (Vec<u16>, Vec<usize>) = (Vec::new(), Vec::new());
        let mut column_start = offsets[0];
        while pos < max {
            let ch = units[pos];
            if ch == 0x7C {
                if !is_escaped {
                    // pipe separating cells, '|'
                    current.0.extend_from_slice(&units[last_pos..pos]);
                    current.1.extend_from_slice(&offsets[last_pos..pos]);
                    let (u, o) = std::mem::take(&mut current);
                    columns.push((u, o, column_start, offsets[pos]));
                    last_pos = pos + 1;
                    column_start = offsets[pos + 1];
                } else {
                    // escaped pipe, '\|'
                    current.0.extend_from_slice(&units[last_pos..pos - 1]);
                    current.1.extend_from_slice(&offsets[last_pos..pos - 1]);
                    last_pos = pos;
                    // The `|` the content keeps stands for the two characters.
                    offsets[pos] = offsets[pos - 1];
                }
            }
            is_escaped = ch == 0x5C;
            pos += 1;
        }
        current.0.extend_from_slice(&units[last_pos..]);
        current.1.extend_from_slice(&offsets[last_pos..max]);
        columns.push((current.0, current.1, column_start, end));

        if columns.first().is_some_and(|c| c.0.is_empty()) {
            columns.remove(0);
        }
        if columns.last().is_some_and(|c| c.0.is_empty()) {
            columns.pop();
        }

        columns
            .into_iter()
            .map(|(u, o, raw_start, raw_end)| {
                // `.trim()`
                let mut a = 0;
                let mut b = u.len();
                while a < b && js::is_js_space(code_point_at(&u, a).0) {
                    a += code_point_at(&u, a).1;
                }
                while b > a && js::is_js_space(code_point_before(&u, b).0) {
                    b -= code_point_before(&u, b).1;
                }
                let mut map = o[a..b].to_vec();
                let after = if b < o.len() { o[b] } else { raw_end };
                // The end: just past the last character kept (both of an escaped pipe's, the
                // only `|` a cell keeps).
                let end = if b > a {
                    let last = o[b - 1];
                    let len = self.src[last..].chars().next().map_or(0, char::len_utf8);
                    let len = if u[b - 1] == 0x7C { len + 1 } else { len };
                    (last + len).min(after.max(last + len))
                } else {
                    after.min(raw_end).max(raw_start)
                };
                map.push(end);
                Cell {
                    raw: Extent::new(raw_start, raw_end),
                    text: u[a..b].to_vec(),
                    map,
                }
            })
            .collect()
    }

    // --- the tokens -----------------------------------------------------------------------

    fn push_open(&mut self, kind: &str, span: Extent, cell: Option<usize>) {
        self.open.push(Open {
            kind: kind.trim_end_matches("_open").to_string(),
            span,
            cell,
        });
    }

    /// A block's closing token: where its block ends (the end of its last line's text), as
    /// an empty span.
    fn close(&mut self, kind: &str) -> Extent {
        let base = kind.trim_end_matches("_close");
        match self.open.iter().rposition(|o| o.kind == base) {
            Some(at) => {
                let open = self.open.remove(at);
                self.open.truncate(at);
                let mut end = open.span.end.max(open.span.start);
                let text = &self.src[open.span.start..end];
                if text.ends_with("\r\n") {
                    end -= 2;
                } else if text.ends_with('\n') || text.ends_with('\r') {
                    end -= 1;
                }
                Extent::at(end)
            }
            None => Extent::at(self.last),
        }
    }

    fn dispatch(&mut self, tokens: &[Token], i: usize) -> (Extent, Option<Vec<Extent>>) {
        let t = &tokens[i];
        let kind = t.kind.as_str();
        let is_markdoc_tag = matches!(
            kind,
            "tag_open" | "tag_close" | "tag" | "annotation" | "variable" | "error"
        );
        if is_markdoc_tag && let Some([s, _]) = t.map {
            return (self.block_tag(s), None);
        }
        if t.nesting < 0 && t.map.is_none() {
            return (self.close(kind), None);
        }
        if kind == "inline" {
            return self.inline(tokens, i);
        }
        let Some([s, e]) = t.map else {
            // th_open, td_open, and anything else without a map.
            if kind == "th_open" || kind == "td_open" {
                let cell = self.next_cell;
                self.next_cell += 1;
                let span = self.row.get(cell).map_or(Extent::at(self.last), |c| c.raw);
                self.push_open(kind, span, Some(cell));
                return (span, None);
            }
            return (Extent::at(self.last), None);
        };
        let end = self.block_end(s, e);
        let span = match kind {
            "frontmatter" => {
                let start = self.line(0).map_or(0, |l| l.start);
                Extent::new(start, self.block_end(e, e + 1))
            }
            "paragraph_open" => {
                let start = match tokens.get(i + 1).filter(|n| n.kind == "inline") {
                    Some(inline) => {
                        let [is, ie] = inline.map.unwrap_or([s, e]);
                        let map = self.paragraph_map(&utf16(&inline.content), is, ie);
                        let start = map[0];
                        self.pending = Some((i + 1, map));
                        start
                    }
                    None => self.first_char(s),
                };
                Extent::new(start, end)
            }
            "heading_open" => {
                if t.markup.starts_with('#') {
                    let start = self
                        .find_marker(s, "#")
                        .unwrap_or_else(|| self.first_char(s));
                    let hashes = self.src[start..].bytes().take_while(|&b| b == b'#').count();
                    self.heading_text = Some(start + hashes);
                    Extent::new(start, end)
                } else {
                    self.heading_text = None;
                    Extent::new(self.first_char(s), end)
                }
            }
            "blockquote_open" => {
                let start = self
                    .find_marker(s, ">")
                    .unwrap_or_else(|| self.first_char(s));
                for line in s..e.min(self.lines.len()) {
                    let l = self.lines[line];
                    let p = self.skip_blanks(self.cursors[line].max(l.start), l.end);
                    if self.src.as_bytes().get(p) == Some(&b'>') && p < l.end {
                        self.cursors[line] = p + 1;
                    }
                }
                Extent::new(start, end)
            }
            "bullet_list_open" | "ordered_list_open" => {
                let needle = tokens
                    .get(i + 1)
                    .filter(|n| n.kind == "list_item_open")
                    .map(|n| format!("{}{}", n.info, n.markup))
                    .unwrap_or_else(|| t.markup.clone());
                let start = self
                    .find_marker(s, &needle)
                    .unwrap_or_else(|| self.first_char(s));
                Extent::new(start, end)
            }
            "list_item_open" => {
                let needle = format!("{}{}", t.info, t.markup);
                match self.find_marker(s, &needle) {
                    Some(p) => {
                        self.cursors[s] = p + needle.len();
                        Extent::new(p, end)
                    }
                    None => Extent::new(self.first_char(s), end),
                }
            }
            "fence" | "code_block" => {
                let start = if kind == "fence" {
                    self.find_marker(s, &t.markup)
                } else {
                    None
                }
                .unwrap_or_else(|| self.first_char(s));
                let span = Extent::new(start, end);
                let children = t.children.as_ref().map(|children| {
                    let content = utf16(&t.content);
                    let map = self.fence_map(&content, s + 1);
                    fence_children(&content, children)
                        .into_iter()
                        .map(|(a, b)| Extent::new(map[a], map[b]))
                        .collect()
                });
                return (span, children);
            }
            "tr_open" => {
                self.row = self.row_layout(tokens, i, s);
                self.next_cell = 0;
                Extent::new(self.first_char(s), end)
            }
            _ => Extent::new(self.first_char(s), end),
        };
        if t.nesting > 0 {
            self.push_open(kind, span, None);
        }
        (span, None)
    }

    /// A Markdoc tag the block rule made: `{%` on its first line, to the `%}` closing it.
    fn block_tag(&mut self, line: usize) -> Extent {
        let Some(l) = self.line(line) else {
            return Extent::at(self.src.len());
        };
        let Some(start) = self.find_marker(line, "{%") else {
            return Extent::new(self.first_char(line), l.end);
        };
        match find_tag_end_bytes(self.src.as_bytes(), start) {
            Some(end) => {
                let end = end + 2;
                if end <= l.end {
                    self.cursors[line] = end;
                }
                Extent::new(start, end)
            }
            None => Extent::new(start, l.end),
        }
    }

    /// An inline token: its content mapped to the source, and its children's spans.
    fn inline(&mut self, tokens: &[Token], i: usize) -> (Extent, Option<Vec<Extent>>) {
        let t = &tokens[i];
        let content = utf16(&t.content);
        let open = self.open.last().cloned();
        let map = match (&self.pending, &open) {
            (Some((at, _)), _) if *at == i => self.pending.take().expect("pending").1,
            (
                _,
                Some(Open {
                    cell: Some(cell), ..
                }),
            ) => match self.row.get(*cell) {
                Some(c) if c.text == content => c.map.clone(),
                Some(c) => {
                    let to = c.raw.end.max(c.map[0]);
                    self.align_forward(&content, c.map[0], to)
                }
                None => vec![self.last; content.len() + 1],
            },
            (_, Some(Open { kind, .. })) if kind == "heading" && self.heading_text.is_some() => {
                let line = t.map.map_or(0, |m| m[0]);
                self.heading_map(&content, line)
            }
            _ => match t.map {
                Some([s, e]) => self.paragraph_map(&content, s, e),
                None => vec![self.last; content.len() + 1],
            },
        };
        let map: Vec<usize> = map.into_iter().map(|o| self.snap(o)).collect();
        let span = Extent::new(map[0], map[content.len()]);
        let children = t.children.as_ref().map(|children| {
            inline_children(&content, children)
                .into_iter()
                .map(|(a, b)| {
                    let (a, b) = (a.min(content.len()), b.min(content.len()));
                    Extent::new(map[a], map[b])
                })
                .collect()
        });
        (span, children)
    }
}

// --- inline tokens ---------------------------------------------------------------------------

/// The units `\` escapes to themselves (escape.ts's `ESCAPED`).
fn escapable(u: u16) -> bool {
    u < 256 && b"\\!\"#$%&'()*+,./:;<=>?@[]^_`{|}~-".contains(&(u as u8))
}

/// `/^&#((?:x[a-f0-9]{1,6}|[0-9]{1,7}));/i`: the match's length (as entity.rs matches it).
fn digital_entity(s: &[u16]) -> Option<usize> {
    if s.len() < 2 || s[0] != 0x26 || s[1] != 0x23 {
        return None;
    }
    let body = &s[2..];
    let is_digit = |c: u16| (0x30..=0x39).contains(&c);
    let is_hex = |c: u16| is_digit(c) || (0x41..=0x46).contains(&c) || (0x61..=0x66).contains(&c);
    let group = match body.first() {
        Some(&c) if c == 0x78 || c == 0x58 => {
            let digits = body[1..].iter().take_while(|&&c| is_hex(c)).count();
            if !(1..=6).contains(&digits) {
                return None;
            }
            1 + digits
        }
        _ => {
            let digits = body.iter().take_while(|&&c| is_digit(c)).count();
            if !(1..=7).contains(&digits) {
                return None;
            }
            digits
        }
    };
    (body.get(group) == Some(&0x3B)).then_some(2 + group + 1)
}

/// `/^&([a-z][a-z0-9]{1,31});/i`: the match's length.
fn named_entity(s: &[u16]) -> Option<usize> {
    let is_alpha = |c: u16| (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c);
    let is_digit = |c: u16| (0x30..=0x39).contains(&c);
    if s.len() < 2 || s[0] != 0x26 || !is_alpha(s[1]) {
        return None;
    }
    let rest = s[2..]
        .iter()
        .take_while(|&&c| is_alpha(c) || is_digit(c))
        .count();
    if !(1..=31).contains(&rest) || s.get(2 + rest) != Some(&0x3B) {
        return None;
    }
    Some(2 + rest + 1)
}

/// What the entity rule makes of `c[p..]` (`&...;`), and how much it takes.
fn entity_at(c: &[u16], p: usize) -> Option<(Vec<u16>, usize)> {
    if p + 1 >= c.len() {
        return None;
    }
    let rest = &c[p..];
    if c[p + 1] == 0x23 {
        let len = digital_entity(rest)?;
        let digits = &rest[2..len - 1];
        let code = if digits[0] == 0x78 || digits[0] == 0x58 {
            js::parse_int(&digits[1..], 16)
        } else {
            js::parse_int(digits, 10)
        } as u32;
        let decoded = if is_valid_entity_code(code) {
            from_code_point(code)
        } else {
            from_code_point(0xFFFD)
        };
        Some((decoded, len))
    } else {
        let len = named_entity(rest)?;
        let decoded = decode_html_strict(&rest[..len]);
        (decoded != rest[..len]).then_some((decoded, len))
    }
}

/// The typographer's changes to text (smartquotes.ts, replacements.ts) that turn the source
/// at `c[p..]` into `t`'s start: how much of each they take.
fn typographic(c: &[u16], p: usize, t: &[u16]) -> Option<(usize, usize)> {
    let ch = c[p];
    let first = *t.first()?;
    let starts = |s: &str| {
        let s = utf16(s);
        c.len() >= p + s.len()
            && c[p..p + s.len()]
                .iter()
                .zip(&s)
                .all(|(&a, &b)| a == b || (a < 128 && b < 128 && (a | 0x20) == (b | 0x20)))
    };
    // smartquotes: a straight quote that became another character
    if (ch == 0x22 || ch == 0x27) && first != ch {
        return Some((code_point_at(t, 0).1, 1));
    }
    let run = |u: u16| c[p..].iter().take_while(|&&x| x == u).count();
    match first {
        0xA9 if starts("(c)") => Some((1, 3)),
        0xAE if starts("(r)") => Some((1, 3)),
        0x2122 if starts("(tm)") => Some((1, 4)),
        0xB1 if starts("+-") => Some((1, 2)),
        0x2026 if ch == 0x2E && run(0x2E) >= 2 => Some((1, run(0x2E))),
        0x2014 if starts("---") => Some((1, 3)),
        0x2013 if starts("--") => Some((1, 2)),
        _ => None,
    }
}

/// How far a text token reaches from `start`: its `content` produced from the source as the
/// inline rules produce it. Stops where that cannot be done.
fn match_text(c: &[u16], start: usize, t: &[u16]) -> usize {
    let n = c.len();
    let (mut p, mut j) = (start, 0);
    while j < t.len() && p < n {
        let rest = &t[j..];
        if c[p] == 0x5C && p + 1 < n {
            let ch1 = c[p + 1];
            if ch1 == 0x0A {
                // `\` before a line ending: a hardbreak, not text.
                break;
            }
            let (produced, used) = if ch1 == 0x20 {
                (vec![0x5C], 1)
            } else {
                let mut escaped = vec![ch1];
                if is_high_surrogate(ch1) && p + 2 < n && is_low_surrogate(c[p + 2]) {
                    escaped.push(c[p + 2]);
                }
                let used = 1 + escaped.len();
                if escapable(ch1) {
                    (escaped, used)
                } else {
                    let mut original = vec![0x5C];
                    original.extend(escaped);
                    (original, used)
                }
            };
            if rest.starts_with(&produced) {
                j += produced.len();
                p += used;
                continue;
            }
        }
        if c[p] == 0x26
            && let Some((decoded, used)) = entity_at(c, p)
            && rest.starts_with(&decoded)
        {
            j += decoded.len();
            p += used;
            continue;
        }
        let units = code_point_at(c, p).1;
        if rest.starts_with(&c[p..p + units]) {
            j += units;
            p += units;
            continue;
        }
        if let Some((dj, dp)) = typographic(c, p, rest) {
            j += dj;
            p += dp;
            continue;
        }
        break;
    }
    p
}

/// Is `c[i]` escaped: after an odd number of backslashes, counted back no further than `from`
/// (an escape is never split between tokens)?
fn escaped(c: &[u16], from: usize, i: usize) -> bool {
    let mut k = 0;
    while i > from + k && c[i - 1 - k] == 0x5C {
        k += 1;
    }
    k % 2 == 1
}

fn index_of(c: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(c.len()));
    }
    (from..c.len().saturating_sub(needle.len() - 1)).find(|&p| c[p..].starts_with(needle))
}

/// `parseLinkLabel(state, start)` without a parser state: the `]` that closes the `[` at
/// `start`, nested brackets counted and escapes, code spans and tags skipped.
fn label_end(c: &[u16], start: usize) -> Option<usize> {
    let mut level = 1;
    let mut p = start + 1;
    while p < c.len() {
        match c[p] {
            0x5D => {
                level -= 1;
                if level == 0 {
                    return Some(p);
                }
                p += 1;
            }
            0x5B => {
                level += 1;
                p += 1;
            }
            0x5C if p + 1 < c.len() => p += 2,
            0x60 => {
                let run = c[p..].iter().take_while(|&&u| u == 0x60).count();
                let mut q = p + run;
                let mut end = None;
                while q < c.len() {
                    if c[q] == 0x60 {
                        let r = c[q..].iter().take_while(|&&u| u == 0x60).count();
                        if r == run {
                            end = Some(q + r);
                            break;
                        }
                        q += r;
                    } else {
                        q += 1;
                    }
                }
                p = end.unwrap_or(p + run);
            }
            0x7B if c.get(p + 1) == Some(&0x25) => {
                p = find_tag_end(c, p).map_or(p + 2, |e| e + 2);
            }
            _ => p += 1,
        }
    }
    None
}

fn skip_link_space(c: &[u16], mut pos: usize) -> usize {
    while pos < c.len() && (is_space(c[pos]) || c[pos] == 0x0A) {
        pos += 1;
    }
    pos
}

/// Where a link or image whose label ends at `c[label_end_at]` (`]`) ends, as link.ts and
/// image.ts read what follows: `(destination "title")`, `[reference]`, `[]` or nothing.
/// `reference` is whether the token says the reference branch made it (`meta.label`), which
/// settles what the tail would otherwise have to be parsed again to know.
fn link_end(c: &[u16], label_end_at: usize, reference: bool, image: bool) -> usize {
    let max = c.len();
    let mut pos = label_end_at + 1;
    if pos < max && c[pos] == 0x28 {
        // Inline link: `(  <href>  "title"  )`
        pos = skip_link_space(c, pos + 1);
        if pos >= max {
            return label_end_at + 1;
        }
        let res = parse_link_destination(c, pos, max);
        if res.ok {
            let href = engine().normalize_link(&res.str);
            if engine().validate_link(&href) {
                pos = res.pos;
            }
        }
        // link.ts reads a title only after a destination; image.ts always.
        if res.ok || image {
            let start = pos;
            pos = skip_link_space(c, pos);
            let title = parse_link_title(c, pos, max, None);
            if pos < max && start != pos && title.ok {
                pos = skip_link_space(c, title.pos);
            }
        }
        if pos < max && c[pos] == 0x29 {
            if image || !reference {
                return pos + 1;
            }
        } else if image || !reference {
            // The token says the inline form made it: its `)` is ahead.
            return index_of(c, &[0x29], pos).map_or(max, |p| p + 1);
        }
        // parsing a valid shortcut link failed, fallback to reference
        pos += 1;
    } else if !reference {
        return label_end_at + 1;
    }
    // Link reference: `[label]`, `[]`, or nothing after the `]`.
    if pos < max && c[pos] == 0x5B {
        return label_end(c, pos).map_or(label_end_at + 1, |end| end + 1);
    }
    label_end_at + 1
}

/// Each inline child's range of `c`, the inline token's content, in UTF-16 units.
fn inline_children(c: &[u16], children: &[Token]) -> Vec<(usize, usize)> {
    let n = c.len();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(children.len());
    let mut cur = 0usize;
    // The range in `out` of the text token just before, which a later token's search may
    // find ends further on.
    let mut text_before: Option<usize> = None;
    let mut autolink_close: Option<usize> = None;
    let mut links: Vec<bool> = Vec::new();

    // Where `wanted` holds, from `cur` on; the text before grows to it.
    let seek = |cur: usize,
                out: &mut Vec<(usize, usize)>,
                text_before: Option<usize>,
                wanted: &dyn Fn(usize) -> bool|
     -> Option<usize> {
        let found = (cur..=n).find(|&p| wanted(p))?;
        if found > cur
            && let Some(k) = text_before
        {
            out[k].1 = found;
        }
        Some(found)
    };

    for t in children {
        let kind = t.kind.as_str();
        let mut is_text = false;
        let mut next = None;
        let range = match kind {
            "text" | "text_special" => {
                is_text = true;
                match autolink_close.filter(|&gt| gt >= cur) {
                    Some(gt) => (cur, gt),
                    None => {
                        let content = utf16(&t.content);
                        // A delimiter the emphasis rules emptied (the other half of a `**`
                        // whose first half closed something) joins the text after it without
                        // a character of its own: the text starts after it.
                        let mut start = cur;
                        while let (Some(&first), Some(&d)) = (content.first(), c.get(start)) {
                            if matches!(d, 0x2A | 0x5F | 0x7E) && first != d {
                                start += 1;
                            } else {
                                break;
                            }
                        }
                        (start, match_text(c, start, &content))
                    }
                }
            }
            "softbreak" | "hardbreak" => {
                let mut p = cur;
                while p < n && c[p] == 0x20 {
                    p += 1;
                }
                let found = if p < n && c[p] == 0x0A {
                    Some((cur, p + 1))
                } else if p + 1 < n && c[p] == 0x5C && c[p + 1] == 0x0A {
                    Some((cur, p + 2))
                } else {
                    seek(cur, &mut out, text_before, &|q| q < n && c[q] == 0x0A).map(|q| {
                        let mut s = q;
                        if kind == "hardbreak" && s > cur && c[s - 1] == 0x5C {
                            s -= 1;
                        }
                        while s > cur && c[s - 1] == 0x20 {
                            s -= 1;
                        }
                        if s > cur
                            && let Some(k) = text_before
                        {
                            out[k].1 = s;
                        }
                        (s, q + 1)
                    })
                };
                match found {
                    Some((a, b)) => {
                        let mut after = b;
                        while after < n && is_space(c[after]) {
                            after += 1;
                        }
                        next = Some(after);
                        (a, b)
                    }
                    None => (cur, cur),
                }
            }
            "em_open" | "em_close" | "strong_open" | "strong_close" | "s_open" | "s_close" => {
                let m = utf16(&t.markup);
                match seek(cur, &mut out, text_before, &|p| {
                    !m.is_empty() && c[p.min(n)..].starts_with(&m)
                }) {
                    Some(p) => (p, p + m.len()),
                    None => (cur, cur),
                }
            }
            "link_open" => {
                if t.markup == "autolink" {
                    match seek(cur, &mut out, text_before, &|p| p < n && c[p] == 0x3C) {
                        Some(p) => {
                            autolink_close = index_of(c, &[0x3E], p + 1);
                            (p, p + 1)
                        }
                        None => (cur, cur),
                    }
                } else if t.markup == "linkify" {
                    (cur, cur)
                } else {
                    links.push(
                        t.meta
                            .as_ref()
                            .and_then(|m| m.get("label"))
                            .is_some_and(|l| !l.is_null()),
                    );
                    match seek(cur, &mut out, text_before, &|p| p < n && c[p] == 0x5B) {
                        Some(p) => (p, p + 1),
                        None => (cur, cur),
                    }
                }
            }
            "link_close" => {
                if t.markup == "autolink" {
                    let gt = autolink_close.take();
                    match gt.filter(|&gt| gt >= cur) {
                        Some(gt) => (gt, gt + 1),
                        None => {
                            match seek(cur, &mut out, text_before, &|p| p < n && c[p] == 0x3E) {
                                Some(p) => (p, p + 1),
                                None => (cur, cur),
                            }
                        }
                    }
                } else if t.markup == "linkify" {
                    (cur, cur)
                } else {
                    let reference = links.pop().unwrap_or(false);
                    match seek(cur, &mut out, text_before, &|p| p < n && c[p] == 0x5D) {
                        Some(p) => (p, link_end(c, p, reference, false)),
                        None => (cur, cur),
                    }
                }
            }
            "image" => {
                let reference = t
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("label"))
                    .is_some_and(|l| !l.is_null());
                let label = utf16(&t.content).len();
                match seek(cur, &mut out, text_before, &|p| {
                    p + 1 < n && c[p] == 0x21 && c[p + 1] == 0x5B
                }) {
                    Some(p) => {
                        let mut close = p + 2 + label;
                        if !(close < n && c[close] == 0x5D) {
                            close = label_end(c, p + 1).unwrap_or(close.min(n));
                        }
                        if close >= n {
                            (p, n)
                        } else {
                            (p, link_end(c, close, reference, true))
                        }
                    }
                    None => (cur, cur),
                }
            }
            "code_inline" => {
                let len = t.markup.len();
                let run_at = |p: usize| c[p..].iter().take_while(|&&u| u == 0x60).count();
                // A run starts where the token before ends, or after anything but a backtick
                // of its own: an escaped one (`\``) is text, not part of the run.
                let starts_run =
                    |p: usize| p == cur || p == 0 || c[p - 1] != 0x60 || escaped(c, cur, p - 1);
                match seek(cur, &mut out, text_before, &|p| {
                    len > 0 && p < n && run_at(p) == len && starts_run(p)
                }) {
                    Some(p) => {
                        let mut q = p + len;
                        let mut end = None;
                        while q < n {
                            if c[q] == 0x60 {
                                let r = run_at(q);
                                if r == len {
                                    end = Some(q + r);
                                    break;
                                }
                                q += r;
                            } else {
                                q += 1;
                            }
                        }
                        (p, end.unwrap_or(p + len))
                    }
                    None => (cur, cur),
                }
            }
            "html_inline" => {
                let m = utf16(&t.content);
                match seek(cur, &mut out, text_before, &|p| {
                    !m.is_empty() && c[p.min(n)..].starts_with(&m)
                }) {
                    Some(p) => (p, p + m.len()),
                    None => (cur, cur),
                }
            }
            "comment" => {
                let open = utf16("<!--");
                match seek(cur, &mut out, text_before, &|p| {
                    c[p.min(n)..].starts_with(&open)
                }) {
                    Some(p) => {
                        let close = index_of(c, &utf16("-->"), p);
                        (p, close.map_or(p + 4, |e| e + 3))
                    }
                    None => (cur, cur),
                }
            }
            _ if t.kind.starts_with("tag")
                || matches!(kind, "annotation" | "variable" | "error") =>
            {
                let info = utf16(&t.info);
                let opens = |p: usize| p + 1 < n && c[p] == 0x7B && c[p + 1] == 0x25;
                let fits = |p: usize| {
                    opens(p)
                        && find_tag_end(c, p).is_some_and(|e| {
                            kind == "error"
                                || js::trim(c.get(p + 2..e).unwrap_or_default()) == info.as_slice()
                        })
                };
                let p = if opens(cur) && find_tag_end(c, cur).is_some() {
                    Some(cur)
                } else {
                    seek(cur, &mut out, text_before, &fits).or_else(|| {
                        seek(cur, &mut out, text_before, &|p| {
                            opens(p) && find_tag_end(c, p).is_some()
                        })
                    })
                };
                match p {
                    Some(p) => (p, find_tag_end(c, p).map_or(p + 2, |e| e + 2)),
                    None => (cur, cur),
                }
            }
            _ => (cur, cur),
        };
        let range = (range.0.min(n), range.1.min(n).max(range.0.min(n)));
        out.push(range);
        cur = next.unwrap_or(range.1).min(n);
        text_before = is_text.then_some(out.len() - 1);
    }
    // The last token ends the content (it is trimmed): a text token cut short by what could
    // not be reproduced reaches it.
    if let (Some(k), Some(last)) = (text_before, out.last_mut())
        && k + 1 == children.len()
        && last.1 < n
    {
        last.1 = n;
    }
    out
}

/// Each of a fence's children's range of its content: Markdoc's `parseTags` cuts the content
/// into text (verbatim slices) and tags (`info` is the tag's `{% %}` text).
fn fence_children(f: &[u16], children: &[Token]) -> Vec<(usize, usize)> {
    let mut cur = 0;
    let mut out = Vec::with_capacity(children.len());
    for t in children {
        let needle = if t.kind == "text" {
            utf16(&t.content)
        } else {
            utf16(&t.info)
        };
        let range = if needle.is_empty() {
            (cur, cur)
        } else if f[cur..].starts_with(&needle) {
            (cur, cur + needle.len())
        } else {
            match index_of(f, &needle, cur) {
                Some(p) => (p, p + needle.len()),
                None => (cur, cur),
            }
        };
        cur = range.1;
        out.push(range);
    }
    out
}
