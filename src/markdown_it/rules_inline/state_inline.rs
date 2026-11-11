//! Port of rules_inline/state_inline.ts, plus the `Delimiter` type of types.ts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::markdown_it::common::utils::{is_md_ascii_punct, is_punct_char_code, is_white_space};
use crate::markdown_it::js;
use crate::markdown_it::token::Token;
use crate::markdown_it::{Env, JsError, MarkdownIt};

/// An emphasis-like delimiter (types.ts `Delimiter`), pushed by emphasis/strikethrough and
/// matched by balance_pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delimiter {
    /// Char code of the starting marker.
    pub marker: u16,
    /// Total length of these series of delimiters (0 disables the "rule of 3").
    pub length: usize,
    /// The index of the token this delimiter corresponds to.
    pub token: usize,
    /// If this delimiter is matched as a valid opener, the index of its closer, else -1.
    pub end: i64,
    /// Whether this delimiter could open an emphasis.
    pub open: bool,
    /// Whether this delimiter could close an emphasis.
    pub close: bool,
}

/// A delimiter list. JS shares these arrays between `state.delimiters`, `_prev_delimiters`
/// and `tokens_meta[i].delimiters` (a list pushed to while an opening token is current is
/// the same array its `tokens_meta` entry holds), so the port shares them too.
pub type DelimiterList = Rc<RefCell<Vec<Delimiter>>>;

/// A new, empty delimiter list (JS `[]`).
pub fn new_delimiter_list() -> DelimiterList {
    Rc::new(RefCell::new(Vec::new()))
}

/// A `tokens_meta` entry (`StateTokenMeta`): what `push` records for an opening token.
#[derive(Debug, Clone, Default)]
pub struct TokenMeta {
    pub delimiters: Option<DelimiterList>,
}

/// What `scanDelims` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScannedDelims {
    pub can_open: bool,
    pub can_close: bool,
    pub length: usize,
}

/// Mutable state passed to inline rules while tokenizing inline content.
#[derive(Debug)]
pub struct StateInline<'a> {
    /// The inline content, as UTF-16 code units (JS string).
    pub src: Vec<u16>,
    pub env: &'a mut Env,
    pub md: &'a MarkdownIt,
    pub tokens: Vec<Token>,
    /// One entry per token pushed with `push` (not `push_pending`), so it is not
    /// index-aligned with `tokens` (as in JS).
    pub tokens_meta: Vec<Option<TokenMeta>>,

    pub pos: usize,
    pub pos_max: usize,
    pub level: i32,
    /// Pending text, as UTF-16 code units: `tokenize` appends one code unit at a time.
    pub pending: Vec<u16>,
    pub pending_level: i32,

    /// Stores { start: end } pairs. Useful for backtrack
    /// optimization of pairs parse (emphasis, strikes).
    pub cache: HashMap<usize, usize>,

    /// Backtick run length => position of its last run, built lazily on first use.
    pub backticks: HashMap<usize, usize>,
    pub backticks_scanned: bool,

    /// Counter used to disable inline linkify-it execution
    /// inside `<a>` and markdown links.
    pub link_level: i32,

    /// List of emphasis-like delimiters for current tag. `None` is JS `undefined`: what
    /// `_prev_delimiters.pop()` gives when a closing token is pushed with no opening one
    /// (Markdoc's annotations then set `[]`; reading it otherwise throws, see
    /// [`StateInline::push_delimiter`] and [`StateInline::current_delimiters`]).
    pub delimiters: Option<DelimiterList>,

    /// Stack of delimiter lists for upper level tags (JS `_prev_delimiters`).
    pub prev_delimiters: Vec<Option<DelimiterList>>,
}

impl<'a> StateInline<'a> {
    pub fn new(
        src: Vec<u16>,
        md: &'a MarkdownIt,
        env: &'a mut Env,
        out_tokens: Vec<Token>,
    ) -> Self {
        let tokens_meta = vec![None; out_tokens.len()];
        let pos_max = src.len();
        StateInline {
            src,
            env,
            md,
            tokens: out_tokens,
            tokens_meta,
            pos: 0,
            pos_max,
            level: 0,
            pending: Vec::new(),
            pending_level: 0,
            cache: HashMap::new(),
            backticks: HashMap::new(),
            backticks_scanned: false,
            link_level: 0,
            delimiters: Some(new_delimiter_list()),
            prev_delimiters: Vec::new(),
        }
    }

    /// `pushPending()`: flush pending text as a `text` token.
    pub fn push_pending(&mut self) -> &mut Token {
        let mut token = Token::new("text", "", 0);
        token.set_content_utf16(&self.pending);
        token.level = self.pending_level;
        self.tokens.push(token);
        self.pending.clear();
        self.tokens.last_mut().expect("just pushed")
    }

    /// `push(type, tag, nesting)`: push a new token to the stream, flushing pending text
    /// first. An opening token gets a fresh delimiter list (recorded in `tokens_meta`); a
    /// closing one restores the previous list.
    pub fn push(&mut self, kind: &str, tag: &str, nesting: i8) -> &mut Token {
        if !self.pending.is_empty() {
            self.push_pending();
        }

        let mut token = Token::new(kind, tag, nesting);
        let mut token_meta = None;

        if nesting < 0 {
            // closing tag
            self.level -= 1;
            self.delimiters = self.prev_delimiters.pop().flatten();
        }

        token.level = self.level;

        if nesting > 0 {
            // opening tag
            self.level += 1;
            self.prev_delimiters.push(self.delimiters.take());
            let list = new_delimiter_list();
            self.delimiters = Some(Rc::clone(&list));
            token_meta = Some(TokenMeta {
                delimiters: Some(list),
            });
        }

        self.pending_level = self.level;
        self.tokens.push(token);
        self.tokens_meta.push(token_meta);
        self.tokens.last_mut().expect("just pushed")
    }

    /// `state.delimiters.push(delimiter)`. Where JS throws (`delimiters` undefined: a
    /// closing token was pushed with no opening one), the TypeError (`reading 'push'`) is
    /// recorded on the environment and the delimiter goes to a new list, so that the rule
    /// can finish; the parse then stops.
    pub fn push_delimiter(&mut self, delimiter: Delimiter) {
        self.delimiters_reading("push").borrow_mut().push(delimiter);
    }

    /// `state.delimiters` for a rule that reads or processes it (balance_pairs, the
    /// post-processing rules). Where JS throws (`delimiters` undefined), the TypeError
    /// (`reading 'length'`) is recorded and an empty list stands in.
    pub fn current_delimiters(&mut self) -> DelimiterList {
        self.delimiters_reading("length")
    }

    /// `state.delimiters`, or (undefined) the TypeError reading `property` of it, recorded,
    /// and a new list.
    fn delimiters_reading(&mut self, property: &str) -> DelimiterList {
        if let Some(list) = &self.delimiters {
            return Rc::clone(list);
        }
        self.env.throw(JsError::type_error(format!(
            "Cannot read properties of undefined (reading '{property}')"
        )));
        let list = new_delimiter_list();
        self.delimiters = Some(Rc::clone(&list));
        list
    }

    /// `scanDelims(start, canSplitWord)`: scan a sequence of emphasis-like markers, and
    /// determine whether it can start an emphasis sequence or end an emphasis sequence.
    ///
    ///  - start - position to scan from (it should point at a valid marker);
    ///  - canSplitWord - determine if these markers can be found inside a word
    pub fn scan_delims(&self, start: usize, can_split_word: bool) -> ScannedDelims {
        let max = self.pos_max;
        let marker = js::char_code_at(&self.src, start);

        // Astral characters below are combined manually, because .codePointAt()
        // does not guarantee numeric type output. And we don't wish JIT cache issues.
        // The broken surrogate pairs are evaluated as U+FFFD to prevent possible
        // crashes.
        //
        // `None` stands for JS's NaN (charCodeAt past the end): it is neither whitespace
        // nor punctuation.

        let last_char: Option<u32> = if start == 0 {
            // treat beginning of the line as a whitespace
            Some(0x20)
        } else if start == 1 {
            js::char_code_at(&self.src, 0).map(|c| {
                if (c & 0xF800) == 0xD800 {
                    0xFFFD
                } else {
                    u32::from(c)
                }
            })
        } else {
            js::char_code_at(&self.src, start - 1).map(|c| {
                let c = u32::from(c);
                if (c & 0xFC00) == 0xDC00 {
                    // low surrogate => add high one, replace broken pair with U+FFFD
                    match js::char_code_at(&self.src, start - 2).map(u32::from) {
                        Some(high_surr) if (high_surr & 0xFC00) == 0xD800 => {
                            0x10000 + ((high_surr - 0xD800) << 10) + (c - 0xDC00)
                        }
                        _ => 0xFFFD,
                    }
                } else if (c & 0xFC00) == 0xD800 {
                    0xFFFD
                } else {
                    c
                }
            })
        };

        let mut pos = start;
        while pos < max && marker.is_some() && js::char_code_at(&self.src, pos) == marker {
            pos += 1;
        }

        let count = pos - start;

        // treat end of the line as a whitespace
        let next_char: Option<u32> = if pos < max {
            js::char_code_at(&self.src, pos).map(u32::from)
        } else {
            Some(0x20)
        };
        let next_char = next_char.map(|c| {
            if (c & 0xFC00) == 0xD800 {
                // high surrogate => add low one, replace broken pair with U+FFFD
                match js::char_code_at(&self.src, pos + 1).map(u32::from) {
                    Some(low_surr) if (low_surr & 0xFC00) == 0xDC00 => {
                        0x10000 + ((c - 0xD800) << 10) + (low_surr - 0xDC00)
                    }
                    _ => 0xFFFD,
                }
            } else if (c & 0xFC00) == 0xDC00 {
                0xFFFD
            } else {
                c
            }
        });

        let is_punct =
            |c: Option<u32>| c.is_some_and(|c| is_md_ascii_punct(c) || is_punct_char_code(c));
        let is_last_punct_char = is_punct(last_char);
        let is_next_punct_char = is_punct(next_char);

        let is_last_white_space = last_char.is_some_and(is_white_space);
        let is_next_white_space = next_char.is_some_and(is_white_space);

        let left_flanking = !is_next_white_space
            && (!is_next_punct_char || is_last_white_space || is_last_punct_char);
        let right_flanking = !is_last_white_space
            && (!is_last_punct_char || is_next_white_space || is_next_punct_char);

        let can_open = left_flanking && (can_split_word || !right_flanking || is_last_punct_char);
        let can_close = right_flanking && (can_split_word || !left_flanking || is_next_punct_char);

        ScannedDelims {
            can_open,
            can_close,
            length: count,
        }
    }
}

#[cfg(test)]
#[allow(clippy::type_complexity)]
mod tests {
    use super::*;

    fn scan(
        src: &[u16],
        start: usize,
        can_split_word: bool,
        pos_max: Option<usize>,
    ) -> (bool, bool, usize) {
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(src.to_vec(), &md, &mut env, Vec::new());
        if let Some(m) = pos_max {
            st.pos_max = m;
        }
        let d = st.scan_delims(start, can_split_word);
        (d.can_open, d.can_close, d.length)
    }

    #[test]
    fn scan_delims_matches_state_inline_ts() {
        let u = js::to_utf16;
        // node: new md.inline.State(src, md, {}, []).scanDelims(start, canSplitWord)
        let cases: [(Vec<u16>, usize, bool, Option<usize>, (bool, bool, usize)); 17] = [
            (u("*a*"), 0, true, None, (true, false, 1)),
            (u("*a*"), 2, true, None, (false, true, 1)),
            (u("a**b"), 1, false, None, (false, false, 2)),
            (u(" _a"), 1, false, None, (true, false, 1)),
            (u("a_"), 1, false, None, (false, true, 1)),
            // astral punctuation (U+1F600 is a symbol) before and after the run
            (u("\u{1F600}*x"), 2, true, None, (true, false, 1)),
            (u("x*\u{1F600}"), 1, true, None, (false, true, 1)),
            // broken surrogates count as U+FFFD (a symbol)
            (vec![0xDE00, 0x2A, 0x78], 1, true, None, (true, false, 1)),
            (
                vec![0x61, 0xDE00, 0x2A, 0x78],
                2,
                true,
                None,
                (true, false, 1),
            ),
            (vec![0x78, 0x2A, 0xD83D], 1, true, None, (false, true, 1)),
            (vec![0x78, 0x2A, 0xDE00], 1, true, None, (false, true, 1)),
            (u("!**a"), 1, false, None, (true, false, 2)),
            (u("a**!"), 1, false, None, (false, true, 2)),
            // posMax ends the run and counts as whitespace
            (u("***"), 0, true, Some(2), (false, false, 2)),
            (u("\u{A0}*a"), 1, true, None, (true, false, 1)),
            (u("\u{20AC}*a"), 1, false, None, (true, false, 1)),
            (u("a*\u{20AC}"), 1, false, None, (false, true, 1)),
        ];
        for (src, start, split, pos_max, want) in cases {
            assert_eq!(scan(&src, start, split, pos_max), want, "{src:x?} {start}");
        }
    }

    #[test]
    fn push_shares_delimiter_lists_like_js() {
        // node: st.pending='ab'; push('link_open','a',1); st.delimiters.push(d); st.pending='c';
        // push('link_close','a',-1); push('x_close','',-1) (delimiters becomes undefined);
        // st.delimiters=[]; push('y_open','',1)
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(js::to_utf16("abc"), &md, &mut env, Vec::new());
        st.pending = js::to_utf16("ab");
        st.push("link_open", "a", 1);
        st.push_delimiter(Delimiter {
            marker: 0x2A,
            length: 1,
            token: 0,
            end: -1,
            open: true,
            close: false,
        });
        st.pending = js::to_utf16("c");
        st.push("link_close", "a", -1);
        st.push("x_close", "", -1);
        assert!(st.delimiters.is_none());
        st.delimiters = Some(new_delimiter_list());
        st.push("y_open", "", 1);
        let tokens: Vec<(&str, i32, &str)> = st
            .tokens
            .iter()
            .map(|t| (t.kind.as_str(), t.level, t.content.as_str()))
            .collect();
        assert_eq!(
            tokens,
            [
                ("text", 0, "ab"),
                ("link_open", 0, ""),
                ("text", 1, "c"),
                ("link_close", 0, ""),
                ("x_close", -1, ""),
                ("y_open", -1, ""),
            ]
        );
        let meta: Vec<Option<usize>> = st
            .tokens_meta
            .iter()
            .map(|m| {
                m.as_ref()
                    .map(|m| m.delimiters.as_ref().unwrap().borrow().len())
            })
            .collect();
        assert_eq!(meta, [Some(1), None, None, Some(0)]);
        assert_eq!(
            (st.level, st.pending_level, st.prev_delimiters.len()),
            (0, 0, 1)
        );
    }

    #[test]
    fn pushing_a_delimiter_to_undefined_throws_like_js() {
        // node: after push('x_close', '', -1) with nothing open, state.delimiters is
        // undefined and state.delimiters.push(...) throws this TypeError
        let md = MarkdownIt::default();
        let mut env = Env::default();
        let mut st = StateInline::new(Vec::new(), &md, &mut env, Vec::new());
        st.push("x_close", "", -1);
        assert!(st.delimiters.is_none());
        st.push_delimiter(Delimiter {
            marker: 0x2A,
            length: 1,
            token: 0,
            end: -1,
            open: true,
            close: true,
        });
        assert_eq!(
            env.thrown,
            Some(JsError::type_error(
                "Cannot read properties of undefined (reading 'push')"
            ))
        );
    }
}
