//! Port of upstream Markdoc v0.5.11's `src/tokenizer/plugins/annotations.ts`.
//!
//! Owner: the plugins agent.
//!
//! Markdoc's tags in markdown-it: a block rule (before `paragraph`, interrupting paragraphs
//! and blockquotes) for a tag alone on its lines, an inline rule (`containers`, after every
//! other inline rule) for a tag anywhere in inline content, and a core rule that reads a tag
//! in a fence's info string into the fence's `meta` and splits the fence's content into text
//! and tags (`children`), unless the info string's tag says `process=false`.
//!
//! What upstream throws through `tokenize` is recorded on the environment; the rules then
//! stop parsing tags, and [`super::super::Tokenizer::tokenize`] returns the error. That is
//! the grammar's `RangeError`, and the `TypeError` a closing tag inside a link leads
//! markdown-it into: the tag pops the delimiter list `link_open` saved, so the `link_close`
//! after it pops an empty stack and leaves `state.delimiters` undefined until a later tag's
//! `if (!state.delimiters) state.delimiters = []`; in between, strikethrough or emphasis
//! pushing a delimiter (`reading 'push'`) or `balance_pairs` at the end (`reading
//! 'length'`) throws, which the engine records (see
//! [`crate::markdown_it::StateInline::push_delimiter`]).

use serde_json::{Map, Value};

use crate::grammar::{self, Meta, ParseError};
use crate::markdown_it::rules_inline::state_inline::new_delimiter_list;
use crate::markdown_it::{
    Env, MarkdownIt, RulerError, StateBlock, StateCore, StateInline, Token, js,
};

use super::super::utils::{CLOSE, OPEN, error_meta, find_tag_end, parse_tags};
use super::super::{range_error, throw, thrown};

/// What `createToken` pushes onto: a block or an inline state (`StateWithDelimiters`).
trait TagState {
    fn push(&mut self, kind: &str, tag: &str, nesting: i8) -> &mut Token;
    /// `if (!state.delimiters) state.delimiters = [];`. A block state has no delimiters (JS
    /// sets the property and nothing reads it); an inline one has none (`undefined`) after a
    /// closing token that had no opening one, as `_prev_delimiters.pop()` then gives.
    fn ensure_delimiters(&mut self);
    fn last_token(&mut self) -> &mut Token;
    fn env(&mut self) -> &mut Env;
}

impl TagState for StateBlock<'_> {
    fn push(&mut self, kind: &str, tag: &str, nesting: i8) -> &mut Token {
        StateBlock::push(self, kind, tag, nesting)
    }
    fn ensure_delimiters(&mut self) {}
    fn last_token(&mut self) -> &mut Token {
        self.tokens.last_mut().expect("a token was pushed")
    }
    fn env(&mut self) -> &mut Env {
        self.env
    }
}

impl TagState for StateInline<'_> {
    fn push(&mut self, kind: &str, tag: &str, nesting: i8) -> &mut Token {
        StateInline::push(self, kind, tag, nesting)
    }
    fn ensure_delimiters(&mut self) {
        if self.delimiters.is_none() {
            self.delimiters = Some(new_delimiter_list());
        }
    }
    fn last_token(&mut self) -> &mut Token {
        self.tokens.last_mut().expect("a token was pushed")
    }
    fn env(&mut self) -> &mut Env {
        self.env
    }
}

/// `createToken(state, content, contentStart?)`: the token for a tag's content, or an `error`
/// token where the grammar throws its SyntaxError (with the error's offsets moved by
/// `content_start` when that is given and not 0, else no location). `None` where upstream
/// throws (the grammar's RangeError, recorded on the environment), and once anything has
/// been recorded there.
fn create_token<'s, S: TagState>(
    state: &'s mut S,
    content: &[u16],
    content_start: Option<usize>,
) -> Option<&'s mut Token> {
    if thrown(state.env()) {
        return None;
    }
    match grammar::parse_utf16(content) {
        Ok(parsed) => {
            let token = state.push(parsed.kind.as_str(), "", parsed.nesting.unwrap_or(0));
            token.info = js::to_string(content);
            token.meta = Some(parsed.meta.to_json());
            token.grammar_meta = Some(Box::new(parsed.meta));

            state.ensure_delimiters();

            Some(state.last_token())
        }
        Err(ParseError::Range(error)) => {
            throw(state.env(), range_error(&error));
            None
        }
        Err(ParseError::Syntax(error)) => {
            // `contentStart ? {...} : null`: undefined (the inline rule) and 0 are falsy
            let location = match content_start {
                Some(content_start) if content_start != 0 => {
                    let offset = |offset: usize| {
                        let mut o = Map::new();
                        o.insert("offset".into(), (offset + content_start).into());
                        Value::Object(o)
                    };
                    let mut location = Map::new();
                    location.insert("start".into(), offset(error.location.start.offset));
                    location.insert("end".into(), offset(error.location.end.offset));
                    Value::Object(location)
                }
                _ => Value::Null,
            };

            let token = state.push("error", "", 0);
            token.meta = Some(error_meta(&error, location));
            Some(token)
        }
    }
}

/// The block rule: a tag that starts a line and ends at the end of a line (maybe a later
/// one), as one token; its lines are consumed.
fn block(state: &mut StateBlock, start_line: usize, _end_line: usize, silent: bool) -> bool {
    // Lines past the state's text (where JS reads `undefined` marks and goes on with NaN;
    // the parsers never call a rule there): no rule matches there.
    if start_line >= state.line_max {
        return false;
    }
    let start = state.b_marks[start_line] + state.t_shift[start_line];
    let finish = state.e_marks[start_line];

    if !js::starts_with(&state.src, OPEN, start) {
        return false;
    }

    let tag_end = find_tag_end(&state.src, start);
    // `state.src.slice(0, finish).trim().length`: the trim takes the whitespace at the start
    // of the whole source too (upstream's, kept: with enough leading blank lines, text after
    // the tag on its line is dropped).
    let last_possible = js::trim(js::slice_to(&state.src, 0, finish)).len() as i64;

    // `!tagEnd`: null, or 0
    let Some(tag_end) = tag_end.filter(|&end| end != 0) else {
        return false;
    };
    if (tag_end as i64) < last_possible - CLOSE.len() as i64 {
        return false;
    }

    let content_start = start + OPEN.len();
    // `.split('\n').length`
    let lines = 1 + js::slice_to(&state.src, start, tag_end + CLOSE.len())
        .iter()
        .filter(|&&c| c == b'\n' as u16)
        .count();
    let text = state.get_lines(start_line, start_line + lines, 0, false);
    let Some(content_end) = find_tag_end(&text, 0) else {
        return false;
    };

    let open_at = js::index_of(&text, OPEN, 0).map_or(-1, |i| i as i64);
    let content = js::trim(js::slice(
        &text,
        open_at + OPEN.len() as i64,
        Some(content_end as i64),
    ));

    if content.first() == Some(&(b'$' as u16)) {
        return false;
    }

    if silent {
        return true;
    }

    if let Some(token) = create_token(state, content, Some(content_start)) {
        token.map = Some([start_line, start_line + lines]);
    }
    state.line += lines;
    true
}

/// The inline rule (`containers`): a tag anywhere in inline content. The tag's end is looked
/// for in the rest of the inline source, not only up to `posMax`, as upstream does.
fn inline(state: &mut StateInline, silent: bool) -> bool {
    if !js::starts_with(&state.src, OPEN, state.pos) {
        return false;
    }

    let tag_end = find_tag_end(&state.src, state.pos);
    // `!tagEnd`: null, or 0
    let Some(tag_end) = tag_end.filter(|&end| end != 0) else {
        return false;
    };

    let content = js::slice_to(&state.src, state.pos + OPEN.len(), tag_end).to_vec();
    if !silent {
        create_token(state, js::trim(&content), None);
    }

    state.pos = tag_end + CLOSE.len();
    true
}

/// `attr.name === 'process' && !attr.value` over a fence's meta, as JSON (`?.` throughout:
/// a meta without an `attributes` array has no such attribute).
fn process_false_json(meta: &Value) -> bool {
    let Some(Value::Array(attributes)) = meta.get("attributes") else {
        return false;
    };
    attributes.iter().any(|attr| {
        attr.get("name") == Some(&Value::String("process".into()))
            && !attr.get("value").is_some_and(json_truthy)
    })
}

/// The same test over the grammar's own meta, which still holds what JSON cannot (a number
/// literal too long for a double is Infinity, truthy, and `null` in JSON).
fn process_false_meta(meta: &Meta) -> bool {
    let Some(Some(attributes)) = &meta.attributes else {
        return false;
    };
    attributes
        .iter()
        .any(|attr| attr.name == "process" && !attr.value.truthy())
}

/// JavaScript's ToBoolean of a JSON value.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The core rule: for each fence (with a map), a tag in its info string becomes its `meta`
/// (or a `fence-tag-error`), and unless that tag has a falsy `process` attribute, its
/// content is split into text and tags as its `children`.
fn core(state: &mut StateCore) {
    if thrown(state.env) {
        return;
    }
    for i in 0..state.tokens.len() {
        if state.tokens[i].kind != "fence" {
            continue;
        }

        let Some(map) = state.tokens[i].map else {
            continue;
        };

        let mut parsed_meta: Option<Meta> = None;
        let info = js::to_utf16(&state.tokens[i].info);
        if js::includes(&info, OPEN, 0) {
            let start = js::index_of(&info, OPEN, 0).expect("includes OPEN");
            let end = find_tag_end(&info, start);
            let content = js::slice(
                &info,
                (start + OPEN.len()) as i64,
                end.map(|end| end as i64),
            );

            match grammar::parse_utf16(js::trim(content)) {
                Ok(parsed) => {
                    state.tokens[i].meta = Some(parsed.meta.to_json());
                    state.tokens[i].grammar_meta = Some(Box::new(parsed.meta.clone()));
                    parsed_meta = Some(parsed.meta);
                }
                Err(ParseError::Range(error)) => {
                    throw(state.env, range_error(&error));
                    return;
                }
                Err(ParseError::Syntax(error)) => {
                    let mut e = Map::new();
                    e.insert("id".into(), "fence-tag-error".into());
                    e.insert("level".into(), "error".into());
                    e.insert(
                        "message".into(),
                        Value::String(format!(
                            "Syntax error in fence tag: {}",
                            error.message.to_string_lossy()
                        )),
                    );
                    state.tokens[i]
                        .errors
                        .get_or_insert_with(Vec::new)
                        .push(Value::Object(e));
                }
            }
        }

        let process_false = match &parsed_meta {
            Some(meta) => process_false_meta(meta),
            None => state.tokens[i]
                .meta
                .as_ref()
                .is_some_and(process_false_json),
        };
        if process_false {
            continue;
        }

        let content = js::to_utf16(&state.tokens[i].content);
        match parse_tags(&content, map[0]) {
            Ok(children) => {
                state.tokens[i].children = Some(children.into_iter().map(|t| t.token).collect());
            }
            Err(error) => {
                throw(state.env, range_error(&error));
                return;
            }
        }
    }
}

/// `plugin(md)`: errors where upstream throws (a markdown-it without a `paragraph` rule).
pub fn plugin(md: &mut MarkdownIt) -> Result<(), RulerError> {
    md.block.ruler.before(
        "paragraph",
        "annotations",
        block,
        &["paragraph", "blockquote"],
    )?;
    md.inline.ruler.push("containers", inline, &[]);
    md.core.ruler.push("annotations", core, &[]);
    Ok(())
}
