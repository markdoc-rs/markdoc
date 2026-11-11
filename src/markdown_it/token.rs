//! Port of token.ts.

use super::js;

/// `TokenAttribute`'s value: `string | number`.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Str(String),
    Num(f64),
}

impl AttrValue {
    /// `${value}`: the string JS would interpolate.
    pub fn to_js_string(&self) -> String {
        match self {
            AttrValue::Str(s) => s.clone(),
            AttrValue::Num(n) => js::number_to_string(*n),
        }
    }
}

impl From<&str> for AttrValue {
    fn from(s: &str) -> Self {
        AttrValue::Str(s.to_owned())
    }
}

impl From<String> for AttrValue {
    fn from(s: String) -> Self {
        AttrValue::Str(s)
    }
}

impl From<f64> for AttrValue {
    fn from(n: f64) -> Self {
        AttrValue::Num(n)
    }
}

/// One item of the token stream. Field for field the JS `Token`, plus the two fields
/// Markdoc's plugins add at runtime (`errors`, `position`), which the engine never sets, and
/// [`Token::pseudo`] for the plain objects Markdoc puts in a fence's `children`.
///
/// Its `meta` (and `grammar_meta`) can hold a tag's values as deep as the grammar nests them
/// (node's limits, thousands of levels), and a hand-made token's `children` nest to any depth:
/// `Clone`, `PartialEq` and `Debug` are `#[derive]`'s, a level at a time on a stack with room
/// for it ([`crate::depth`]); `Debug` writes 400 levels of a value and `..` for what is
/// deeper.
pub struct Token {
    /// JS `type`, e.g. `"paragraph_open"`.
    pub kind: String,
    /// HTML tag name, e.g. `"p"`.
    pub tag: String,
    /// HTML attributes; `None` is JS `null`.
    pub attrs: Option<Vec<(String, AttrValue)>>,
    /// Source map `[line_begin, line_end]`; `None` is JS `null`.
    pub map: Option<[usize; 2]>,
    /// 1 opening, 0 self-closing, -1 closing.
    pub nesting: i8,
    /// Nesting level, as `state.level`. Negative when a plugin closes more than it opened.
    pub level: i32,
    /// Child tokens (inline and image tokens); `None` is JS `null`.
    pub children: Option<Vec<Token>>,
    pub content: String,
    pub markup: String,
    pub info: String,
    /// A place for plugins to store data; `None` is JS `null`.
    pub meta: Option<serde_json::Value>,
    /// What `meta` is the JSON of, on the tokens Markdoc's annotations plugin makes from a
    /// tag: the tag grammar's result itself, which holds what JSON cannot (an overlong number
    /// literal is `Infinity` here and `null` in `meta`). Not a field of the JS token, which
    /// holds the grammar's values in `meta` itself; `None` on every other token and on tokens
    /// read from JSON. Whoever sets it keeps `meta` its JSON: Markdoc's parser reads the
    /// values from here when it is set.
    pub grammar_meta: Option<Box<crate::grammar::Meta>>,
    pub block: bool,
    pub hidden: bool,
    /// Set by Markdoc's plugins (`token.errors`), never by the engine.
    pub errors: Option<Vec<serde_json::Value>>,
    /// Set by Markdoc (`token.position`), never by the engine. `Some(Null)` is a `position`
    /// that is `null` (only a hand-made token has one). Boxed: few tokens have one, and a
    /// smaller token is faster to make and move.
    pub position: Option<Box<serde_json::Value>>,
    /// `Some` for an object Markdoc's `parseTags` made (a fence's `children`): not a
    /// markdown-it token, but a plain object with fields of its own (see [`PseudoToken`]).
    /// `None` for every token markdown-it makes (boxed: few tokens have one).
    pub pseudo: Option<Box<PseudoToken>>,
    /// `content` as JS holds it where the engine made a lone surrogate there (see
    /// [`ExactContent`]); not part of the token's value.
    pub(crate) exact: ExactContent,
}

/// The exact UTF-16 of a token's `content` where it holds a lone surrogate the engine made
/// (an `xn--` label that decodes to one, linkify-it's `maxLength` cutting a pair, a `quotes`
/// option of astral characters): `content` is then its lossy form (U+FFFD for each lone
/// surrogate, the bytes node writes when it encodes the string as UTF-8). The rules that read
/// a token's content back as UTF-16 (linkify, replacements, smartquotes, and the two joins of
/// adjacent text tokens) read this instead, so they see the code unit node sees, and halves
/// that meet pair again as in JS. It is not part of the token's value: equality ignores it,
/// and it counts only while `content` is still its lossy form (a plugin that sets `content`
/// itself drops it).
///
/// A thin pointer (one word), so tokens, which the engine makes and moves by the hundred
/// thousand, grow by eight bytes only.
#[derive(Debug, Clone, Default)]
#[allow(clippy::box_collection)]
pub(crate) struct ExactContent(Option<Box<Vec<u16>>>);

impl PartialEq for ExactContent {
    fn eq(&self, _: &ExactContent) -> bool {
        true
    }
}

/// `units` as a Rust string, a lone surrogate as U+FFFD; and whether there was one. One
/// pass where there is none (the common case).
#[inline]
pub(crate) fn utf16_to_string(units: &[u16]) -> (String, bool) {
    match String::from_utf16(units) {
        Ok(s) => (s, false),
        Err(_) => (String::from_utf16_lossy(units), true),
    }
}

/// Whether `s` is the lossy form of `units`.
fn is_lossy_form(units: &[u16], s: &str) -> bool {
    char::decode_utf16(units.iter().copied())
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .eq(s.chars())
}

/// What an object `parseTags` makes holds beyond the [`Token`] fields it is read as:
/// `start` and `end` (UTF-16 offsets into the fence's content, `end` inclusive, so -1 for an
/// empty text at the start), and which of a token's fields it has. A text is `{type, start,
/// end, content}`; a tag is `{map, position, start, end, info, type, nesting, meta}`, where
/// `nesting` is there only when the tag grammar's result has one (a variable, an annotation
/// and a syntax error have none). The fields an object lacks read as a token's defaults
/// (`nesting` 0, as upstream's parser reads `nesting ?? 0`), and its JSON leaves them out.
#[derive(Debug, Clone, PartialEq)]
pub struct PseudoToken {
    pub start: i64,
    pub end: i64,
    pub shape: PseudoShape,
}

/// See [`PseudoToken`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoShape {
    /// `{type: 'text', start, end, content}`.
    Text,
    /// `{map, position, start, end, info, ...parseTag()}`: `nesting` says whether the
    /// result spread in has one.
    Tag { nesting: bool },
}

impl Clone for Token {
    fn clone(&self) -> Self {
        crate::depth::grow!(Token {
            kind: self.kind.clone(),
            tag: self.tag.clone(),
            attrs: self.attrs.clone(),
            map: self.map,
            nesting: self.nesting,
            level: self.level,
            children: self.children.clone(),
            content: self.content.clone(),
            markup: self.markup.clone(),
            info: self.info.clone(),
            meta: self.meta.as_ref().map(json_clone),
            grammar_meta: self.grammar_meta.clone(),
            block: self.block,
            hidden: self.hidden,
            errors: self.errors.clone(),
            position: self.position.as_ref().map(|p| Box::new(json_clone(p))),
            pseudo: self.pseudo.clone(),
            exact: self.exact.clone(),
        })
    }
}

impl PartialEq for Token {
    fn eq(&self, other: &Token) -> bool {
        let Token {
            kind,
            tag,
            attrs,
            map,
            nesting,
            level,
            children,
            content,
            markup,
            info,
            meta,
            grammar_meta,
            block,
            hidden,
            errors,
            position,
            pseudo,
            // Not part of the token's value (see [`ExactContent`]).
            exact: _,
        } = self;
        crate::depth::grow!({
            *kind == other.kind
                && *tag == other.tag
                && *attrs == other.attrs
                && *map == other.map
                && *nesting == other.nesting
                && *level == other.level
                && *content == other.content
                && *markup == other.markup
                && *info == other.info
                && *block == other.block
                && *hidden == other.hidden
                && *errors == other.errors
                && match (position, &other.position) {
                    (Some(a), Some(b)) => json_eq(a, b),
                    (a, b) => a.is_none() && b.is_none(),
                }
                && *pseudo == other.pseudo
                && match (meta, &other.meta) {
                    (Some(a), Some(b)) => json_eq(a, b),
                    (a, b) => a.is_none() && b.is_none(),
                }
                && *grammar_meta == other.grammar_meta
                && *children == other.children
        })
    }
}

impl std::fmt::Debug for Token {
    /// As `#[derive(Debug)]` writes it, a value in `meta` down to 400 levels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::depth::grow!({
            f.debug_struct("Token")
                .field("kind", &self.kind)
                .field("tag", &self.tag)
                .field("attrs", &self.attrs)
                .field("map", &self.map)
                .field("nesting", &self.nesting)
                .field("level", &self.level)
                .field("children", &self.children)
                .field("content", &self.content)
                .field("markup", &self.markup)
                .field("info", &self.info)
                .field("meta", &self.meta.as_ref().map(JsonDebug))
                .field("grammar_meta", &self.grammar_meta)
                .field("block", &self.block)
                .field("hidden", &self.hidden)
                .field("errors", &self.errors)
                .field("position", &self.position.as_deref().map(JsonDebug))
                .field("pseudo", &self.pseudo)
                .field("exact", &self.exact)
                .finish()
        })
    }
}

/// `value.clone()`, a level at a time on a stack with room for it.
fn json_clone(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    crate::depth::grow!(match value {
        Value::Array(items) => Value::Array(items.iter().map(json_clone).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_clone(v)))
                .collect(),
        ),
        other => other.clone(),
    })
}

/// `a == b`, a level at a time on a stack with room for it.
fn json_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    crate::depth::grow!(match (a, b) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_eq(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|w| json_eq(v, w)))
        }
        (a, b) => a == b,
    })
}

/// A JSON value as serde_json's `Debug` writes it, down to 400 levels.
struct JsonDebug<'a>(&'a serde_json::Value);

impl std::fmt::Debug for JsonDebug<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use serde_json::Value;
        crate::types::debug_level(f, |f| match self.0 {
            Value::Array(items) => {
                f.write_str("Array ")?;
                f.debug_list().entries(items.iter().map(JsonDebug)).finish()
            }
            Value::Object(map) => {
                f.write_str("Object ")?;
                f.debug_map()
                    .entries(map.iter().map(|(k, v)| (k, JsonDebug(v))))
                    .finish()
            }
            other => std::fmt::Debug::fmt(other, f),
        })
    }
}

impl Token {
    /// `new Token(type, tag, nesting)`.
    pub fn new(kind: &str, tag: &str, nesting: i8) -> Token {
        Token {
            kind: kind.to_owned(),
            tag: tag.to_owned(),
            attrs: None,
            map: None,
            nesting,
            level: 0,
            children: None,
            content: String::new(),
            markup: String::new(),
            info: String::new(),
            meta: None,
            grammar_meta: None,
            block: false,
            hidden: false,
            errors: None,
            position: None,
            pseudo: None,
            exact: ExactContent(None),
        }
    }

    /// The exact UTF-16 of `content` where the engine made a lone surrogate there and
    /// `content` is still its lossy form (see [`ExactContent`]).
    pub(crate) fn exact_content(&self) -> Option<&[u16]> {
        let units = self.exact.0.as_deref()?.as_slice();
        is_lossy_form(units, &self.content).then_some(units)
    }

    /// `content` as JS holds it: its UTF-16, exact where the engine made a lone surrogate.
    pub(crate) fn content_utf16(&self) -> Vec<u16> {
        match self.exact_content() {
            Some(units) => units.to_vec(),
            None => js::to_utf16(&self.content),
        }
    }

    /// `content.charCodeAt(0)` as JS reads it (`None` for an empty content).
    pub(crate) fn first_content_unit(&self) -> Option<u16> {
        match self.exact_content() {
            Some(units) => units.first().copied(),
            None => self.content.encode_utf16().next(),
        }
    }

    /// `content.charCodeAt(content.length - 1)` as JS reads it (`None` for an empty content).
    pub(crate) fn last_content_unit(&self) -> Option<u16> {
        match self.exact_content() {
            Some(units) => units.last().copied(),
            None => self.content.chars().next_back().map(|c| {
                let mut buf = [0u16; 2];
                let n = c.encode_utf16(&mut buf).len();
                buf[n - 1]
            }),
        }
    }

    /// `content = units`: the lossy form in `content`, the units themselves kept where they
    /// hold a lone surrogate.
    #[inline]
    pub(crate) fn set_content_utf16(&mut self, units: &[u16]) {
        let (content, lone) = utf16_to_string(units);
        self.content = content;
        self.exact = ExactContent(lone.then(|| Box::new(units.to_vec())));
    }

    /// `tokens[next].content = tokens[curr].content + tokens[next].content` where either
    /// holds a lone surrogate the engine made (so two halves that meet pair again, as in JS);
    /// `false`, and nothing done, where neither does.
    #[inline]
    pub(crate) fn join_exact_into(prev: &mut Token, next: &mut Token) -> bool {
        if prev.exact.0.is_none() && next.exact.0.is_none() {
            return false;
        }
        if prev.exact_content().is_none() && next.exact_content().is_none() {
            return false;
        }
        let mut units = prev.content_utf16();
        units.extend(next.content_utf16());
        prev.content.clear();
        prev.exact = ExactContent(None);
        next.set_content_utf16(&units);
        true
    }

    /// `attrIndex(name)`; `None` where JS returns `-1`.
    pub fn attr_index(&self, name: &str) -> Option<usize> {
        let attrs = self.attrs.as_ref()?;

        attrs.iter().position(|(n, _)| n == name)
    }

    /// `attrPush([name, value])`: add an attribute, creating the list if necessary.
    pub fn attr_push(&mut self, attr_data: (String, AttrValue)) {
        match &mut self.attrs {
            Some(attrs) => attrs.push(attr_data),
            None => self.attrs = Some(vec![attr_data]),
        }
    }

    /// `attrSet(name, value)`: set `name` to `value`, replacing an existing value.
    pub fn attr_set(&mut self, name: &str, value: impl Into<AttrValue>) {
        let idx = self.attr_index(name);
        let attr_data = (name.to_owned(), value.into());

        match idx {
            None => self.attr_push(attr_data),
            Some(idx) => self.attrs.as_mut().expect("attr_index found it")[idx] = attr_data,
        }
    }

    /// `attrGet(name)`; `None` where JS returns `null`.
    pub fn attr_get(&self, name: &str) -> Option<&AttrValue> {
        let idx = self.attr_index(name)?;
        Some(&self.attrs.as_ref().expect("attr_index found it")[idx].1)
    }

    /// `attrJoin(name, value)`: append `value` to an existing attribute with a space between
    /// (the result is a string, as in JS), or create the attribute.
    pub fn attr_join(&mut self, name: &str, value: impl Into<AttrValue>) {
        let value = value.into();
        match self.attr_index(name) {
            None => self.attr_push((name.to_owned(), value)),
            Some(idx) => {
                let attr = &mut self.attrs.as_mut().expect("attr_index found it")[idx];
                attr.1 = AttrValue::Str(format!(
                    "{} {}",
                    attr.1.to_js_string(),
                    value.to_js_string()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_token_has_js_defaults() {
        let t = Token::new("paragraph_open", "p", 1);
        assert_eq!(
            (t.kind.as_str(), t.tag.as_str(), t.nesting, t.level),
            ("paragraph_open", "p", 1, 0)
        );
        assert!(t.attrs.is_none() && t.map.is_none() && t.children.is_none() && t.meta.is_none());
        assert!(t.content.is_empty() && t.markup.is_empty() && t.info.is_empty());
        assert!(!t.block && !t.hidden);
    }

    #[test]
    fn attribute_helpers_follow_token_ts() {
        let mut t = Token::new("link_open", "a", 1);
        assert_eq!(t.attr_index("href"), None);
        assert_eq!(t.attr_get("href"), None);
        t.attr_set("href", "/x");
        t.attr_push(("title".into(), "T".into()));
        assert_eq!(t.attr_index("title"), Some(1));
        t.attr_set("href", "/y");
        assert_eq!(t.attr_get("href"), Some(&AttrValue::Str("/y".into())));
        // node: t.attrJoin('class','a'); t.attrJoin('class', 2) => 'a 2'; a number joins as String(n)
        t.attr_join("class", "a");
        t.attr_join("class", 2.0);
        assert_eq!(t.attr_get("class"), Some(&AttrValue::Str("a 2".into())));
        t.attr_set("start", 3.0);
        t.attr_join("start", 1.5);
        assert_eq!(t.attr_get("start"), Some(&AttrValue::Str("3 1.5".into())));
        assert_eq!(t.attrs.as_ref().map(Vec::len), Some(4));
    }
}
