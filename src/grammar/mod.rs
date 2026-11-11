//! The parser for the content of a Markdoc tag: everything between `{%` and `%}`.
//!
//! This is a line-by-line port of upstream Markdoc 0.5.11's `src/grammar/tag.js`, the parser
//! PEG.js 0.11.0-master generated from `tag.pegjs`, and it returns what that parser returns
//! when Markdoc calls it as `parse(content, {Variable, Function})` with its `Ast.Variable`
//! and `Ast.Function` classes: the same result objects, and the same `SyntaxError`s with the
//! same message, expectations, found character and location. `tag.rs` holds the port;
//! this module holds the types the JavaScript builds, and `value.rs` the values.
//!
//! JavaScript strings are UTF-16, so every position here (`offset`, `column`) counts UTF-16
//! code units, as `peg$currPos` does.
//!
//! Both [`Parsed`] and [`SyntaxError`] render as JSON exactly as `JSON.stringify` renders the
//! JavaScript objects (`to_json_string`), and convert to the `serde_json::Value` that
//! `JSON.parse` of that text gives (`to_json`; see [`Value::to_json`] for what a
//! `serde_json::Value` cannot hold).

mod json;
mod tag;
mod value;

#[cfg(test)]
mod tests;

use std::fmt;

pub use crate::markdown_it::js::RangeError;
pub use tag::{MAX_NESTING, cost};
pub use value::{Function, Object, Prototype, Value, Variable};

use json::{utf16_to_json, write_str, write_utf16};

/// Parses the content of a tag, as `parse(content, {Variable, Function})` does upstream.
///
/// The error is what upstream's parser throws: its [`SyntaxError`], or, when values nest
/// more than [`MAX_NESTING`] levels deep, the `RangeError` V8 throws once its stack runs out
/// (see [`MAX_NESTING`] for where the two limits differ).
pub fn parse(content: &str) -> Result<Parsed, ParseError> {
    let input: Vec<u16> = content.encode_utf16().collect();
    tag::peg_parse(&input)
}

/// [`parse`] for content that is already UTF-16 (a slice of a JavaScript-model source).
/// String values are Rust strings, so a lone surrogate inside a string literal becomes
/// U+FFFD in them; positions, `found` and `message` keep every code unit.
pub fn parse_utf16(input: &[u16]) -> Result<Parsed, ParseError> {
    tag::peg_parse(input)
}

/// One whole tag, `{% ... %}`, read as Markdoc's tokenizer reads it: the body between the
/// delimiters, with its line breaks as markdown-it hands them over (`\r\n` and `\r` are
/// `\n`, NUL is U+FFFD), trimmed as JavaScript's `String.prototype.trim` trims, through
/// [`parse_utf16`]. `None` when `tag` does not start with `{%` and end with `%}`.
///
/// ```
/// use markdoc::grammar::{ParsedType, parse_tag};
///
/// let parsed = parse_tag("{%\r\n  callout type=\"note\" /%}").unwrap().unwrap();
/// assert_eq!(parsed.kind, ParsedType::Tag);
/// assert_eq!(parsed.meta.tag.as_deref(), Some("callout"));
/// assert!(parse_tag("{% callout x= %}").unwrap().is_err());
/// assert!(parse_tag("callout").is_none());
/// ```
pub fn parse_tag(tag: &str) -> Option<Result<Parsed, ParseError>> {
    let body = tag.strip_prefix("{%")?.strip_suffix("%}")?;
    let body: Vec<u16> = body
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "\u{FFFD}")
        .encode_utf16()
        .collect();
    Some(parse_utf16(crate::markdown_it::js::trim(&body)))
}

/// What upstream's parser throws.
///
/// Markdoc's tokenizer catches only the [`SyntaxError`] (it becomes an `error` token, or a
/// fence's `fence-tag-error`) and rethrows anything else, so a [`ParseError::Range`] escapes
/// `Tokenizer.tokenize`.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    /// `peg$SyntaxError`: the content is not a tag.
    Syntax(SyntaxError),
    /// `RangeError: Maximum call stack size exceeded`: values nested more than
    /// [`MAX_NESTING`] levels deep.
    Range(RangeError),
}

impl ParseError {
    /// The [`SyntaxError`], if this is one.
    pub fn as_syntax(&self) -> Option<&SyntaxError> {
        match self {
            ParseError::Syntax(error) => Some(error),
            ParseError::Range(_) => None,
        }
    }
}

impl From<SyntaxError> for ParseError {
    fn from(error: SyntaxError) -> ParseError {
        ParseError::Syntax(error)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Syntax(error) => fmt::Display::fmt(error, f),
            ParseError::Range(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for ParseError {}

/// The result of a successful parse: `{type, nesting?, meta}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// JavaScript's `type`.
    pub kind: ParsedType,
    /// 1 for `tag_open`, 0 for `tag`, -1 for `tag_close`; absent for `annotation` and
    /// `variable` (Markdoc's tokenizer then uses 0).
    pub nesting: Option<i8>,
    pub meta: Meta,
}

/// `type` of a [`Parsed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParsedType {
    /// `{% name ... %}`
    TagOpen,
    /// `{% /name %}`
    TagClose,
    /// `{% name ... /%}`
    Tag,
    /// `{% #id .class key=value %}`: attributes without a tag name.
    Annotation,
    /// `{% $variable %}` or `{% function() %}`
    Variable,
}

impl ParsedType {
    /// Every type, in the order of the enum.
    ///
    /// ```
    /// use markdoc::grammar::ParsedType;
    ///
    /// for kind in ParsedType::ALL {
    ///     assert_eq!(kind.as_str().parse::<ParsedType>(), Ok(kind));
    /// }
    /// assert!("tag-open".parse::<ParsedType>().is_err());
    /// ```
    pub const ALL: [ParsedType; 5] = [
        ParsedType::TagOpen,
        ParsedType::TagClose,
        ParsedType::Tag,
        ParsedType::Annotation,
        ParsedType::Variable,
    ];

    /// The name upstream's grammar gives the type (`tag_open`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            ParsedType::TagOpen => "tag_open",
            ParsedType::TagClose => "tag_close",
            ParsedType::Tag => "tag",
            ParsedType::Annotation => "annotation",
            ParsedType::Variable => "variable",
        }
    }
}

impl std::str::FromStr for ParsedType {
    type Err = String;

    /// The type [`ParsedType::as_str`] names.
    fn from_str(name: &str) -> std::result::Result<ParsedType, String> {
        ParsedType::ALL
            .into_iter()
            .find(|kind| kind.as_str() == name)
            .ok_or_else(|| format!("not a tag type: {name:?}"))
    }
}

/// `meta` of a [`Parsed`]. Which keys are present depends on the type, as in JavaScript:
/// `{tag, attributes}` (tag_open, tag), `{tag}` (tag_close), `{attributes}` (annotation),
/// `{variable}` (variable).
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub tag: Option<String>,
    /// `None`: no `attributes` key. `Some(None)`: `attributes: null` (a tag with neither a
    /// primary value nor attributes). `Some(Some(list))`: the attributes.
    pub attributes: Option<Option<Vec<Attribute>>>,
    /// A [`Value::Variable`], a [`Value::Function`], or for `@` paths a [`Value::Array`].
    pub variable: Option<Value>,
}

/// One attribute: `{type: "attribute", name, value}`, or `{type: "class", name, value: true}`
/// for `.name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub kind: AttributeType,
    pub name: String,
    pub value: Value,
}

/// `type` of an [`Attribute`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeType {
    /// `"attribute"`: `name=value`, `#id` (name `id`) and the primary value (name `primary`).
    Attribute,
    /// `"class"`: `.name`
    Class,
}

impl AttributeType {
    pub fn as_str(self) -> &'static str {
        match self {
            AttributeType::Attribute => "attribute",
            AttributeType::Class => "class",
        }
    }
}

/// A JavaScript string: UTF-16 code units, which may include a surrogate that is not half of
/// a pair (a `SyntaxError`'s `found` is one code unit, so it can be half of an astral
/// character, and its `message` quotes it).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct JsString(pub Vec<u16>);

impl JsString {
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    /// The string, with U+FFFD for a lone surrogate.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
}

impl From<&str> for JsString {
    fn from(s: &str) -> JsString {
        JsString(s.encode_utf16().collect())
    }
}

impl PartialEq<str> for JsString {
    fn eq(&self, other: &str) -> bool {
        self.0.iter().copied().eq(other.encode_utf16())
    }
}

impl PartialEq<&str> for JsString {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl fmt::Debug for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = String::new();
        write_utf16(&mut s, &self.0);
        f.write_str(&s)
    }
}

/// `peg$SyntaxError`: what the parser throws.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxError {
    /// `peg$SyntaxError.buildMessage(expected, found)`.
    pub message: JsString,
    /// The expectations recorded at the furthest position reached, in the order they were
    /// recorded, repeats included.
    pub expected: Vec<Expectation>,
    /// The code unit at that position, or `None` (`null`) at the end of the input.
    pub found: Option<JsString>,
    pub location: Location,
}

/// A PEG.js expectation object. This grammar records only `literal`, `other` and `end`;
/// the other kinds are here because `buildMessage` describes them.
#[derive(Clone, Debug, PartialEq)]
pub enum Expectation {
    /// `{type: "literal", text, ignoreCase}`
    Literal {
        text: &'static str,
        ignore_case: bool,
    },
    /// `{type: "class", parts, inverted, ignoreCase}`
    Class {
        parts: &'static [ClassPart],
        inverted: bool,
        ignore_case: bool,
    },
    /// `{type: "any"}`
    Any,
    /// `{type: "end"}`
    End,
    /// `{type: "other", description}`: a named rule.
    Other { description: &'static str },
    /// `{type: "not", expected}`
    Not(Box<Expectation>),
}

/// One part of a class expectation: a character, or a range `[from, to]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassPart {
    Char(&'static str),
    Range(&'static str, &'static str),
}

/// `{start, end}` of a [`SyntaxError`] (no `filename`: Markdoc passes none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub start: Position,
    pub end: Position,
}

/// `{offset, line, column}`: offset and column in UTF-16 code units, line and column from 1;
/// only `\n` starts a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.message, f)
    }
}

impl std::error::Error for SyntaxError {}

// --- JSON: the objects as JSON.stringify renders them ------------------------------------

fn obj(entries: Vec<(&str, serde_json::Value)>) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for (k, v) in entries {
        m.insert(k.to_string(), v);
    }
    serde_json::Value::Object(m)
}

impl Parsed {
    /// `JSON.parse(JSON.stringify(result))`: `{type, nesting?, meta}`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut entries = vec![("type", self.kind.as_str().into())];
        if let Some(n) = self.nesting {
            entries.push(("nesting", i64::from(n).into()));
        }
        entries.push(("meta", self.meta.to_json()));
        obj(entries)
    }

    /// `JSON.stringify(result)`, byte for byte.
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    pub fn write_json(&self, out: &mut String) {
        out.push_str("{\"type\":");
        write_str(out, self.kind.as_str());
        if let Some(n) = self.nesting {
            out.push_str(",\"nesting\":");
            out.push_str(&n.to_string());
        }
        out.push_str(",\"meta\":");
        self.meta.write_json(out);
        out.push('}');
    }
}

impl Meta {
    pub fn to_json(&self) -> serde_json::Value {
        let mut entries = Vec::new();
        if let Some(tag) = &self.tag {
            entries.push(("tag", tag.clone().into()));
        }
        if let Some(attributes) = &self.attributes {
            entries.push((
                "attributes",
                match attributes {
                    None => serde_json::Value::Null,
                    Some(list) => list.iter().map(Attribute::to_json).collect(),
                },
            ));
        }
        if let Some(variable) = &self.variable {
            entries.push(("variable", variable.to_json()));
        }
        obj(entries)
    }

    pub fn write_json(&self, out: &mut String) {
        out.push('{');
        let mut first = true;
        let mut key = |out: &mut String, k: &str| {
            if !first {
                out.push(',');
            }
            first = false;
            write_str(out, k);
            out.push(':');
        };
        if let Some(tag) = &self.tag {
            key(out, "tag");
            write_str(out, tag);
        }
        if let Some(attributes) = &self.attributes {
            key(out, "attributes");
            match attributes {
                None => out.push_str("null"),
                Some(list) => {
                    out.push('[');
                    for (i, a) in list.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        a.write_json(out);
                    }
                    out.push(']');
                }
            }
        }
        if let Some(variable) = &self.variable {
            key(out, "variable");
            variable.write_json(out);
        }
        out.push('}');
    }
}

impl Attribute {
    pub fn to_json(&self) -> serde_json::Value {
        obj(vec![
            ("type", self.kind.as_str().into()),
            ("name", self.name.clone().into()),
            ("value", self.value.to_json()),
        ])
    }

    pub fn write_json(&self, out: &mut String) {
        out.push_str("{\"type\":");
        write_str(out, self.kind.as_str());
        out.push_str(",\"name\":");
        write_str(out, &self.name);
        out.push_str(",\"value\":");
        self.value.write_json(out);
        out.push('}');
    }
}

impl Expectation {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Expectation::Literal { text, ignore_case } => obj(vec![
                ("type", "literal".into()),
                ("text", (*text).into()),
                ("ignoreCase", (*ignore_case).into()),
            ]),
            Expectation::Class {
                parts,
                inverted,
                ignore_case,
            } => obj(vec![
                ("type", "class".into()),
                (
                    "parts",
                    parts
                        .iter()
                        .map(|p| match p {
                            ClassPart::Char(c) => serde_json::Value::from(*c),
                            ClassPart::Range(a, b) => serde_json::Value::from(vec![*a, *b]),
                        })
                        .collect(),
                ),
                ("inverted", (*inverted).into()),
                ("ignoreCase", (*ignore_case).into()),
            ]),
            Expectation::Any => obj(vec![("type", "any".into())]),
            Expectation::End => obj(vec![("type", "end".into())]),
            Expectation::Other { description } => obj(vec![
                ("type", "other".into()),
                ("description", (*description).into()),
            ]),
            Expectation::Not(e) => obj(vec![("type", "not".into()), ("expected", e.to_json())]),
        }
    }

    pub fn write_json(&self, out: &mut String) {
        match self {
            Expectation::Literal { text, ignore_case } => {
                out.push_str("{\"type\":\"literal\",\"text\":");
                write_str(out, text);
                out.push_str(if *ignore_case {
                    ",\"ignoreCase\":true}"
                } else {
                    ",\"ignoreCase\":false}"
                });
            }
            Expectation::Class {
                parts,
                inverted,
                ignore_case,
            } => {
                out.push_str("{\"type\":\"class\",\"parts\":[");
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    match p {
                        ClassPart::Char(c) => write_str(out, c),
                        ClassPart::Range(a, b) => {
                            out.push('[');
                            write_str(out, a);
                            out.push(',');
                            write_str(out, b);
                            out.push(']');
                        }
                    }
                }
                out.push_str("],\"inverted\":");
                out.push_str(if *inverted { "true" } else { "false" });
                out.push_str(",\"ignoreCase\":");
                out.push_str(if *ignore_case { "true}" } else { "false}" });
            }
            Expectation::Any => out.push_str("{\"type\":\"any\"}"),
            Expectation::End => out.push_str("{\"type\":\"end\"}"),
            Expectation::Other { description } => {
                out.push_str("{\"type\":\"other\",\"description\":");
                write_str(out, description);
                out.push('}');
            }
            Expectation::Not(e) => {
                out.push_str("{\"type\":\"not\",\"expected\":");
                e.write_json(out);
                out.push('}');
            }
        }
    }
}

impl Position {
    pub fn to_json(&self) -> serde_json::Value {
        obj(vec![
            ("offset", self.offset.into()),
            ("line", self.line.into()),
            ("column", self.column.into()),
        ])
    }

    pub fn write_json(&self, out: &mut String) {
        out.push_str(&format!(
            "{{\"offset\":{},\"line\":{},\"column\":{}}}",
            self.offset, self.line, self.column
        ));
    }
}

impl Location {
    pub fn to_json(&self) -> serde_json::Value {
        obj(vec![
            ("start", self.start.to_json()),
            ("end", self.end.to_json()),
        ])
    }

    pub fn write_json(&self, out: &mut String) {
        out.push_str("{\"start\":");
        self.start.write_json(out);
        out.push_str(",\"end\":");
        self.end.write_json(out);
        out.push('}');
    }
}

impl SyntaxError {
    /// `JSON.parse(JSON.stringify(error))`: `{message, expected, found, location, name}`
    /// (the constructor's own enumerable properties; `name` is `"SyntaxError"`). A lone
    /// surrogate in `message` or `found` becomes U+FFFD (see [`JsString`]).
    pub fn to_json(&self) -> serde_json::Value {
        obj(vec![
            ("message", utf16_to_json(&self.message.0)),
            (
                "expected",
                self.expected.iter().map(Expectation::to_json).collect(),
            ),
            (
                "found",
                self.found
                    .as_ref()
                    .map_or(serde_json::Value::Null, |f| utf16_to_json(&f.0)),
            ),
            ("location", self.location.to_json()),
            ("name", "SyntaxError".into()),
        ])
    }

    /// `JSON.stringify(error)`, byte for byte.
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_fields_json(&mut out);
        out.insert(0, '{');
        out.push_str(",\"name\":\"SyntaxError\"}");
        out
    }

    /// `"message":…,"expected":[…],"found":…,"location":{…}`: the four fields PEG.js
    /// documents, in `JSON.stringify` order, without the surrounding braces.
    pub fn write_fields_json(&self, out: &mut String) {
        out.push_str("\"message\":");
        write_utf16(out, &self.message.0);
        out.push_str(",\"expected\":[");
        for (i, e) in self.expected.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            e.write_json(out);
        }
        out.push_str("],\"found\":");
        match &self.found {
            None => out.push_str("null"),
            Some(f) => write_utf16(out, &f.0),
        }
        out.push_str(",\"location\":");
        self.location.write_json(out);
    }
}
