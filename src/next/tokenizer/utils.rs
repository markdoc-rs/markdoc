//! Port of upstream Markdoc v0.5.11's `src/utils.ts`.
//!
//! Owner: the plugins agent.
//!
//! `findTagEnd`, `parseTags`, `containsMarkdocTagInUrl`, `OPEN`/`CLOSE`, `isIdentifier`.
//! `isPromise` is not ported: nothing in the Rust port is a promise.
//!
//! Strings are JavaScript strings here, as in the engine ([`crate::markdown_it::js`]): the
//! functions take UTF-16 code units and every position they return or record is a UTF-16
//! index, as upstream's are.

use serde_json::{Map, Value};

use crate::grammar::{self, Meta, ParseError, RangeError};
use crate::markdown_it::js;
use crate::markdown_it::{PseudoShape, PseudoToken, Token};

/// `OPEN = '{%'`
pub const OPEN: &[u16] = &[b'{' as u16, b'%' as u16];
/// `CLOSE = '%}'`
pub const CLOSE: &[u16] = &[b'%' as u16, b'}' as u16];

const NEWLINE: u16 = b'\n' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// `isIdentifier(s)`: `typeof s === 'string' && /^[a-zA-Z0-9_-]+$/.test(s)` (`$` without the
/// `m` flag is the end of the string, so a trailing newline does not match).
pub fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `enum STATES`
#[derive(Clone, Copy, PartialEq, Eq)]
enum States {
    Normal,
    String,
    Escape,
}

/// `findTagEnd(content, start = 0)`: the position of the `%}` that closes the tag opened at or
/// after `start`, skipping `%}` inside double-quoted strings; `None` where JS returns `null`.
pub fn find_tag_end(content: &[u16], start: usize) -> Option<usize> {
    let mut state = States::Normal;
    let mut pos = start;
    while pos < content.len() {
        let char = content[pos];

        match state {
            States::String => match char {
                QUOTE => state = States::Normal,
                BACKSLASH => state = States::Escape,
                _ => {}
            },
            States::Escape => state = States::String,
            States::Normal => {
                if char == QUOTE {
                    state = States::String;
                } else if js::starts_with(content, CLOSE, pos) {
                    return Some(pos);
                }
            }
        }
        pos += 1;
    }

    None
}

/// `containsMarkdocTagInUrl(content, allowedProtocols)`: whether the first closed tag in
/// `content` directly follows (no whitespace between) text containing `<protocol>://`.
/// Only the first closed tag is looked at, as upstream does.
pub fn contains_markdoc_tag_in_url(content: &[u16], allowed_protocols: &[String]) -> bool {
    let Some(before) = text_before_first_closed_tag(content) else {
        return false;
    };
    // Check if the content contains the URL.
    allowed_protocols.iter().any(|protocol| {
        let needle = js::to_utf16(&format!("{protocol}://"));
        js::includes(before, &needle, 0)
    })
}

/// Where `containsMarkdocTagInUrl` iterates the protocols: the text before the first closed
/// tag in `content`, back to the first whitespace (`content.slice(start, pos)`); `None` where
/// it returns false before (no closed tag).
pub fn text_before_first_closed_tag(content: &[u16]) -> Option<&[u16]> {
    let mut pos = 0;
    while pos < content.len() {
        if !js::starts_with(content, OPEN, pos) {
            pos += 1;
            continue;
        }

        let end = find_tag_end(content, pos);
        if end.is_none() {
            // If we cannot find the closing tag, we skip over it
            pos += OPEN.len() - 1;
            pos += 1;
            continue;
        }

        // Locate the start of the content from the tag start (stop tracing the starting
        // position when it hits the first whitespace); `/\s/` is JS's whitespace class.
        let mut start = pos;
        while start > 0 && !js::is_js_space(u32::from(content[start - 1])) {
            start -= 1;
        }

        return Some(js::slice_to(content, start, pos));
    }

    None
}

/// What `parseTag` returns, spread into a `parseTags` token: the grammar's `{type, nesting?,
/// meta}`, or `{type: 'error', meta: {error: {message, location}}}` for a syntax error.
struct TagResult {
    kind: String,
    nesting: Option<i8>,
    meta: Value,
    /// The grammar's own result `meta` is the JSON of (`None` for an error).
    grammar_meta: Option<Meta>,
}

/// `parseTag(content, line, contentStart)`. A `RangeError` from the grammar is rethrown, as
/// upstream rethrows anything that is not its `SyntaxError`.
fn parse_tag(content: &[u16], line: usize, content_start: i64) -> Result<TagResult, RangeError> {
    match grammar::parse_utf16(content) {
        Ok(parsed) => Ok(TagResult {
            kind: parsed.kind.as_str().to_owned(),
            nesting: parsed.nesting,
            meta: parsed.meta.to_json(),
            grammar_meta: Some(parsed.meta),
        }),
        Err(ParseError::Range(error)) => Err(error),
        Err(ParseError::Syntax(error)) => {
            let point = |line: usize, character: i64| {
                let mut o = Map::new();
                o.insert("line".into(), line.into());
                o.insert("character".into(), character.into());
                Value::Object(o)
            };
            let mut location = Map::new();
            location.insert(
                "start".into(),
                point(line, error.location.start.offset as i64 + content_start),
            );
            location.insert(
                "end".into(),
                point(
                    line.saturating_add(1),
                    error.location.end.offset as i64 + content_start,
                ),
            );
            Ok(TagResult {
                kind: "error".to_owned(),
                nesting: None,
                meta: error_meta(&error, Value::Object(location)),
                grammar_meta: None,
            })
        }
    }
}

/// `{error: {message, location}}`: the `meta` of an error token. The message is a JS string
/// (UTF-16); a surrogate in it that is not half of a pair (the grammar quotes the one code
/// unit it failed at, which can be half of an astral character) becomes U+FFFD, the one thing
/// a `serde_json::Value` cannot hold.
pub(crate) fn error_meta(error: &grammar::SyntaxError, location: Value) -> Value {
    let mut inner = Map::new();
    inner.insert(
        "message".into(),
        Value::String(error.message.to_string_lossy()),
    );
    inner.insert("location".into(), location);
    let mut meta = Map::new();
    meta.insert("error".into(), Value::Object(inner));
    Value::Object(meta)
}

/// One item of what [`parse_tags`] returns. Upstream's are plain objects, not markdown-it
/// tokens: a text `{type: 'text', start, end, content}`, or a tag `{map, position, start, end,
/// info, ...parseTag()}`. The fields a [`Token`] has are in `token` (a field upstream's object
/// lacks is the token's default: `nesting` 0 for a text, an error, an annotation or a variable;
/// `level` 0; `block` false), whose [`Token::pseudo`] says which fields the object has and
/// holds `start` and `end` too. `start` and `end` are UTF-16 offsets into the content, `end`
/// inclusive (so -1 for an empty text at the very start).
#[derive(Clone, Debug, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub start: usize,
    pub end: i64,
}

/// The two newline searches `parseTags` makes for each tag, `content.lastIndexOf('\n', pos)`
/// and `content.indexOf('\n', end)`, answered in one pass over the content: the tags come in
/// order, so both positions only grow. Upstream scans the tag's whole line twice per tag,
/// which is quadratic in the number of tags on a line (40,000 tags on one 280 KB line took
/// seconds); the answers are the same.
struct NewlineCursor<'a> {
    content: &'a [u16],
    /// Every position below this one has been looked at by `last_at_or_before`.
    scanned: usize,
    /// The last newline below `scanned`.
    last: Option<usize>,
    /// The answer to the last `first_at_or_after(from)`: `(from, newline)`.
    next: Option<(usize, Option<usize>)>,
}

impl<'a> NewlineCursor<'a> {
    fn new(content: &'a [u16]) -> Self {
        NewlineCursor {
            content,
            scanned: 0,
            last: None,
            next: None,
        }
    }

    /// `content.lastIndexOf('\n', pos)`; `pos` must not be below the one asked before.
    fn last_at_or_before(&mut self, pos: usize) -> Option<usize> {
        let upto = pos.saturating_add(1).min(self.content.len());
        if upto > self.scanned {
            if let Some(i) = self.content[self.scanned..upto]
                .iter()
                .rposition(|&c| c == NEWLINE)
            {
                self.last = Some(self.scanned + i);
            }
            self.scanned = upto;
        }
        self.last
    }

    /// `content.indexOf('\n', from)`; `from` must not be below the one asked before.
    fn first_at_or_after(&mut self, from: usize) -> Option<usize> {
        if let Some((asked, found)) = self.next {
            // No newline lies in `asked..found`, so none lies in `from..found` either.
            if asked <= from && found.is_none_or(|f| f >= from) {
                return found;
            }
        }
        let start = from.min(self.content.len());
        let found = self.content[start..]
            .iter()
            .position(|&c| c == NEWLINE)
            .map(|i| start + i);
        self.next = Some((from, found));
        found
    }
}

/// `parseTags(content, firstLine = 0)`: split the content of a fence into text and the tags
/// in it. Errors where upstream throws (the grammar's `RangeError`).
///
/// Line numbers are JS numbers upstream; here they stop at `usize::MAX` (a `first_line` that
/// far is no fence's).
pub fn parse_tags(content: &[u16], first_line: usize) -> Result<Vec<SpannedToken>, RangeError> {
    let mut line = first_line.saturating_add(1);
    let mut output = Vec::new();
    let mut start: usize = 0;
    let mut newlines = NewlineCursor::new(content);

    let mut pos: usize = 0;
    while pos < content.len() {
        if content[pos] == NEWLINE {
            line = line.saturating_add(1);
            pos += 1;
            continue;
        }

        if !js::starts_with(content, OPEN, pos) {
            pos += 1;
            continue;
        }

        let end = find_tag_end(content, pos);

        let Some(end) = end else {
            // If we cannot find the closing tag, we skip over it
            pos += OPEN.len();
            pos += 1;
            continue;
        };

        let text = js::slice_to(content, pos, end + CLOSE.len());
        let inner = js::slice_to(content, pos + OPEN.len(), end);
        // lastIndexOf and indexOf give -1 when there is no newline; `slice` then counts a
        // negative index from the end (upstream's arithmetic, kept).
        let line_start: i64 = newlines.last_at_or_before(pos).map_or(-1, |i| i as i64);
        let line_end: i64 = newlines.first_at_or_after(end).map_or(-1, |i| i as i64);
        let line_content = js::slice(content, line_start, Some(line_end));
        let tag = parse_tag(js::trim(inner), line, pos as i64 - line_start)?;

        // Throw away excess whitespace introduced by block-level tags
        let preceding_text_end: i64 = if js::trim(line_content) == text {
            line_start
        } else {
            pos as i64
        };
        let preceding_text = js::slice(content, start as i64, Some(preceding_text_end));

        let mut text_token = Token::new("text", "", 0);
        text_token.content = js::to_string(preceding_text);
        text_token.pseudo = Some(Box::new(PseudoToken {
            start: start as i64,
            end: pos as i64 - 1,
            shape: PseudoShape::Text,
        }));
        output.push(SpannedToken {
            token: text_token,
            start,
            end: pos as i64 - 1,
        });

        let mut tag_token = Token::new(&tag.kind, "", tag.nesting.unwrap_or(0));
        tag_token.map = Some([line, line.saturating_add(1)]);
        let mut position = Map::new();
        position.insert("start".into(), (pos as i64 - line_start).into());
        position.insert(
            "end".into(),
            (pos as i64 - line_start + text.len() as i64).into(),
        );
        tag_token.position = Some(Box::new(Value::Object(position)));
        tag_token.info = js::to_string(text);
        tag_token.meta = Some(tag.meta);
        tag_token.grammar_meta = tag.grammar_meta.map(Box::new);
        tag_token.pseudo = Some(Box::new(PseudoToken {
            start: pos as i64,
            end: (pos + text.len()) as i64 - 1,
            shape: PseudoShape::Tag {
                nesting: tag.nesting.is_some(),
            },
        }));
        output.push(SpannedToken {
            token: tag_token,
            start: pos,
            end: (pos + text.len()) as i64 - 1,
        });

        start = end + CLOSE.len();
        pos = start; // `pos = start - 1`, then the loop's `pos++`
    }

    let mut text_token = Token::new("text", "", 0);
    text_token.content = js::to_string(js::slice_to(content, start, content.len()));
    text_token.pseudo = Some(Box::new(PseudoToken {
        start: start as i64,
        end: content.len() as i64 - 1,
        shape: PseudoShape::Text,
    }));
    output.push(SpannedToken {
        token: text_token,
        start,
        end: content.len() as i64 - 1,
    });

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        js::to_utf16(s)
    }

    /// upstream's utils.test.ts
    #[test]
    fn parse_tags_simple_example() {
        let example = u("this is a {% foo blah=\"asdf\" %}test{% /foo %} of template parsing");
        let output = parse_tags(&example, 0).unwrap();
        let summary: Vec<(String, String, String, usize, i64)> = output
            .iter()
            .map(|t| {
                (
                    t.token.kind.clone(),
                    t.token.content.clone(),
                    t.token.info.clone(),
                    t.start,
                    t.end,
                )
            })
            .collect();
        let s = |kind: &str, content: &str, info: &str, start: usize, end: i64| {
            (kind.into(), content.into(), info.into(), start, end)
        };
        assert_eq!(
            summary,
            vec![
                s("text", "this is a ", "", 0, 9),
                s("tag_open", "", "{% foo blah=\"asdf\" %}", 10, 30),
                s("text", "test", "", 31, 34),
                s("tag_close", "", "{% /foo %}", 35, 44),
                s("text", " of template parsing", "", 45, 64),
            ]
        );
        assert_eq!(output[1].token.nesting, 1);
        assert_eq!(
            output[1].token.meta,
            Some(serde_json::json!({
                "tag": "foo",
                "attributes": [{"type": "attribute", "name": "blah", "value": "asdf"}]
            }))
        );
        assert_eq!(output[3].token.nesting, -1);
        assert_eq!(
            output[3].token.meta,
            Some(serde_json::json!({"tag": "foo"}))
        );
    }

    #[test]
    fn find_tag_end_inline_tags() {
        let at = |s: &str, start: usize| find_tag_end(&u(s), start);
        assert_eq!(at("# Testing {% #foo.bar baz=1 %}", 0), Some(28));
        assert_eq!(
            at("# Testing {% #foo.bar baz=\"example\" test=true %}", 0),
            Some(46)
        );
        assert_eq!(
            at(
                "# Testing {% #foo.bar baz={test: 1, foo: {test: \"asdf{\"}} %}",
                0
            ),
            Some(58)
        );
        assert_eq!(at("{% foo %}", 0), Some(7));
        assert_eq!(at("{% foo .bar.baz#test %}", 0), Some(21));
        assert_eq!(at("{% foo test=\"this is a test\" %}", 0), Some(29));
        assert_eq!(at("{% foo .bar#baz", 0), None);
        assert_eq!(
            at(
                "{% #foo .bar .baz test=\"this} is \\\"{test}\\\" a test\" %} this is a test",
                0
            ),
            Some(52)
        );
    }

    #[test]
    fn find_tag_end_multiline_tags() {
        let at = |s: &str| find_tag_end(&u(s), 0);
        let simple = "\n        {% test #foo.bar\n              baz=1 %}\n        ";
        assert_eq!(at(simple), Some(46));
        let with_string = "\n        {% test #foo.bar\n              baz=\"this is a test\"\n              example=1 %}\n        ";
        assert_eq!(at(with_string), Some(85));
        let escaped = "\n        {% test #foo.bar\n              baz=\"this \\\"is a test\"\n              example=1 %}\n        ";
        assert_eq!(at(escaped), Some(87));
        let brace = "\n        {% test #foo.bar\n              baz=\"this {is a test\"\n              example=1 %}\n        ";
        assert_eq!(at(brace), Some(86));
        let escapes = "\n        {% test #foo.bar\n              baz=\"th\\\"is {is a \\\\te\\\"st\"\n              example=1 %}\n        ";
        assert_eq!(at(escapes), Some(92));
        let object = "\n        {% test #foo.bar\n              foo={testing: \"this } is a test\", bar: {baz: 1}}\n              example=1 another=\"test}\" %}\n        ";
        assert_eq!(at(object), Some(129));
        let invalid = "\n        {test #foo.bar\n              foo={testing: \"this } is a test\", bar: {baz: 1}\n              example=1 another=\"test}\"}\n        ";
        assert_eq!(at(invalid), None);
    }

    #[test]
    fn parse_tags_does_not_hang_on_an_open_brace_in_a_code_block() {
        let example = u("```\n{%a %b %c}\n```");
        let output = parse_tags(&example, 0).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].token.kind, "text");
        assert_eq!(output[0].token.content, "```\n{%a %b %c}\n```");
        assert_eq!((output[0].start, output[0].end), (0, 17));
    }

    #[test]
    fn the_newline_cursor_answers_as_last_index_of_and_index_of() {
        // every content of up to 7 units over {a, \n}, and every increasing pair of queries
        for len in 0..8usize {
            for bits in 0..(1u32 << len) {
                let content: Vec<u16> = (0..len)
                    .map(|i| if bits >> i & 1 == 1 { NEWLINE } else { 0x61 })
                    .collect();
                for step in 1..4 {
                    let mut cursor = NewlineCursor::new(&content);
                    let mut pos = 0;
                    while pos < len + 2 {
                        let end = pos + step - 1;
                        assert_eq!(
                            cursor.last_at_or_before(pos),
                            js::last_index_of(&content, &[NEWLINE], Some(pos)),
                            "{content:?} lastIndexOf at {pos}"
                        );
                        assert_eq!(
                            cursor.first_at_or_after(end),
                            js::index_of(&content, &[NEWLINE], end),
                            "{content:?} indexOf from {end}"
                        );
                        pos += step;
                    }
                }
            }
        }
    }

    #[test]
    fn many_tags_on_one_line_are_linear() {
        // 40,000 tags on one 280 KB line: upstream (and the port before) scanned the whole
        // line twice per tag. Here the content is split in one pass, with no allocation per
        // tag beyond the tokens.
        let tags = 40_000;
        let content = u(&format!("a\n{}\nb", "{% x %}".repeat(tags)));
        let output = parse_tags(&content, 3).unwrap();
        assert_eq!(output.len(), 2 * tags + 1);
        // positions count from the newline before the line (upstream's arithmetic); a line
        // holding more than the tag keeps the text before the tag
        assert_eq!(output[0].token.content, "a\n");
        assert_eq!(output[1].token.info, "{% x %}");
        assert_eq!(
            output[1].token.position.as_deref(),
            Some(&serde_json::json!({"start": 1, "end": 8}))
        );
        assert_eq!(
            output[2 * tags - 1].token.position.as_deref(),
            Some(&serde_json::json!({"start": 1 + 7 * (tags - 1), "end": 1 + 7 * tags}))
        );
        assert_eq!(output[2 * tags].token.content, "\nb");
        assert!(
            output[1..2 * tags]
                .iter()
                .step_by(2)
                .all(|t| t.token.map == Some([5, 6]))
        );
    }

    #[test]
    fn is_identifier_is_the_regex() {
        for (s, ok) in [
            ("a", true),
            ("a-b_c9", true),
            ("", false),
            ("a b", false),
            ("a\n", false),
            ("é", false),
            ("a.b", false),
        ] {
            assert_eq!(is_identifier(s), ok, "{s:?}");
        }
    }

    #[test]
    fn contains_markdoc_tag_in_url_looks_at_the_first_closed_tag_only() {
        let http = ["http".to_owned(), "https".to_owned()];
        let check = |s: &str| contains_markdoc_tag_in_url(&u(s), &http);
        assert!(check("https://example.com/{% tag %}"));
        assert!(check("[Link](https://{% $variable.custom_value %})"));
        assert!(!check("The link is https://example.com. {% tag /%})"));
        assert!(!check("[Link]({% tag %})"));
        // an unclosed tag is skipped (a quote keeps this one open); only the first closed one
        // counts, and a tag at the start has no text before it
        assert!(check("{% x \"https://a{% y %}"));
        assert!(!check("{% x https://a{% y %}"));
        assert!(!check("{% a %} https://x{% b %}"));
        // the word before the tag stops at JS whitespace (NBSP included)
        assert!(!check("https://a\u{a0}{% b %}"));
    }
}
