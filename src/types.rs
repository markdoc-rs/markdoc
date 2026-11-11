use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum MarkdocError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Transform error: {0}")]
    TransformError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
}

pub type Result<T> = std::result::Result<T, MarkdocError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeType {
    #[serde(rename = "document")]
    Document,
    #[serde(rename = "heading")]
    Heading,
    #[serde(rename = "paragraph")]
    Paragraph,
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "code")]
    Code,
    #[serde(rename = "fence")]
    Fence,
    #[serde(rename = "blockquote")]
    Blockquote,
    #[serde(rename = "list")]
    List,
    #[serde(rename = "item")]
    Item,
    #[serde(rename = "link")]
    Link,
    #[serde(rename = "image")]
    Image,
    #[serde(rename = "em")]
    Em,
    #[serde(rename = "strong")]
    Strong,
    #[serde(rename = "s")]
    Strikethrough,
    #[serde(rename = "hr")]
    Hr,
    #[serde(rename = "hardbreak")]
    Hardbreak,
    #[serde(rename = "softbreak")]
    Softbreak,
    #[serde(rename = "table")]
    Table,
    #[serde(rename = "thead")]
    Thead,
    #[serde(rename = "tbody")]
    Tbody,
    #[serde(rename = "tr")]
    Tr,
    #[serde(rename = "th")]
    Th,
    #[serde(rename = "td")]
    Td,
    #[serde(rename = "tag")]
    Tag,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "comment")]
    Comment,
    #[serde(rename = "inline")]
    Inline,
    #[serde(rename = "node")]
    Node,
    /// Any other type name. Upstream's parser names a node after its token
    /// (`token.type` without `_open`/`_close`), so a token type outside the
    /// list above -- `html_block` and `html_inline` when the engine's `html`
    /// option is on -- makes a node of that type. Never holds one of the
    /// names above: build a `NodeType` from a name with [`NodeType::from_name`].
    #[serde(untagged)]
    Other(String),
}

impl NodeType {
    /// The node type upstream calls `name`: the variant above that serializes
    /// as `name`, or `Other(name)`.
    pub fn from_name(name: &str) -> NodeType {
        match name {
            "document" => NodeType::Document,
            "heading" => NodeType::Heading,
            "paragraph" => NodeType::Paragraph,
            "text" => NodeType::Text,
            "code" => NodeType::Code,
            "fence" => NodeType::Fence,
            "blockquote" => NodeType::Blockquote,
            "list" => NodeType::List,
            "item" => NodeType::Item,
            "link" => NodeType::Link,
            "image" => NodeType::Image,
            "em" => NodeType::Em,
            "strong" => NodeType::Strong,
            "s" => NodeType::Strikethrough,
            "hr" => NodeType::Hr,
            "hardbreak" => NodeType::Hardbreak,
            "softbreak" => NodeType::Softbreak,
            "table" => NodeType::Table,
            "thead" => NodeType::Thead,
            "tbody" => NodeType::Tbody,
            "tr" => NodeType::Tr,
            "th" => NodeType::Th,
            "td" => NodeType::Td,
            "tag" => NodeType::Tag,
            "error" => NodeType::Error,
            "comment" => NodeType::Comment,
            "inline" => NodeType::Inline,
            "node" => NodeType::Node,
            other => NodeType::Other(other.to_string()),
        }
    }

    /// The type's name, as upstream spells it (`"s"` for `Strikethrough`).
    pub fn as_str(&self) -> &str {
        match self {
            NodeType::Document => "document",
            NodeType::Heading => "heading",
            NodeType::Paragraph => "paragraph",
            NodeType::Text => "text",
            NodeType::Code => "code",
            NodeType::Fence => "fence",
            NodeType::Blockquote => "blockquote",
            NodeType::List => "list",
            NodeType::Item => "item",
            NodeType::Link => "link",
            NodeType::Image => "image",
            NodeType::Em => "em",
            NodeType::Strong => "strong",
            NodeType::Strikethrough => "s",
            NodeType::Hr => "hr",
            NodeType::Hardbreak => "hardbreak",
            NodeType::Softbreak => "softbreak",
            NodeType::Table => "table",
            NodeType::Thead => "thead",
            NodeType::Tbody => "tbody",
            NodeType::Tr => "tr",
            NodeType::Th => "th",
            NodeType::Td => "td",
            NodeType::Tag => "tag",
            NodeType::Error => "error",
            NodeType::Comment => "comment",
            NodeType::Inline => "inline",
            NodeType::Node => "node",
            NodeType::Other(name) => name,
        }
    }
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Upstream's `Location` (`types.ts`), as upstream's parser builds a node's: `{file, start:
/// {line, character}, end: {line, character}}`, where `line` is the node's `lines[0]` and
/// `lines[1]` (0-based, from markdown-it's line maps) and `character` the token's `position`,
/// which only the tags Markdoc finds in a fence have. This is `Node::location`; this crate's
/// own positions (1-based lines, UTF-16 columns, byte offsets) are [`Span`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// `args.file`: `None` is `undefined`, which JSON leaves out. One string, which the
    /// parser's nodes share (as upstream's share the JS string).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "shared_str")]
    pub file: Option<std::sync::Arc<str>>,
    pub start: LocationEdge,
    pub end: LocationEdge,
}

/// Upstream's `LocationEdge`: `{line, character}`, each `undefined` (`None`, left out of
/// JSON) where upstream's is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationEdge {
    /// `node.lines[0]` (start) or `node.lines[1]` (end) when the node was made: `None` when
    /// its lines were shorter (a token without `map` under a parent without lines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// `token.position.start` (start) or `.end` (end), copied as it is: a tag in a fence has
    /// a number (UTF-16 units from the newline before its line); a hand-made token's
    /// `position` may hold any value, and `null` stays `null`. Boxed: most nodes have none,
    /// and a node holds two edges.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_boxed"
    )]
    pub character: Option<Box<serde_json::Value>>,
}

impl Location {
    /// The location as upstream's JSON writes it: `{"file"?, "start": {"line"?,
    /// "character"?}, "end": {...}}`, an `undefined` member left out.
    pub fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        if let Some(file) = &self.file {
            o.insert("file".into(), file.as_ref().into());
        }
        o.insert("start".into(), self.start.to_json());
        o.insert("end".into(), self.end.to_json());
        serde_json::Value::Object(o)
    }
}

impl LocationEdge {
    /// See [`Location::to_json`].
    pub fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        if let Some(line) = self.line {
            o.insert("line".into(), line.into());
        }
        if let Some(character) = &self.character {
            o.insert("character".into(), (**character).clone());
        }
        serde_json::Value::Object(o)
    }
}

/// A member that is present is `Some`, even when it is `null` (serde's own `Option` reads
/// `null` as `None`; upstream's `null` and `undefined` are two values).
fn present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// Serde for an `Option<Arc<str>>` (serde's own needs its `rc` feature): as a string.
mod shared_str {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    pub fn serialize<S: Serializer>(
        value: &Option<Arc<str>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_deref().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Arc<str>>, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map(Arc::from))
    }
}

/// [`present`], boxed.
fn present_boxed<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Box<serde_json::Value>>, D::Error> {
    serde_json::Value::deserialize(deserializer).map(|v| Some(Box::new(v)))
}

/// This crate's own position of a node or an error in the source: where it starts and ends,
/// each as a 1-based line, a UTF-16 column and a byte offset. Upstream has no such thing (its
/// `lines` and `location` are [`crate::ast::Node::lines`] and [`Location`]); the parser fills
/// it ([`crate::ast::Node::span`], [`ValidationError::span`]) when asked to, with
/// [`crate::parser::ParserArgs::spans`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub file: Option<String>,
    pub start: Position,
    pub end: Position,
}

/// One end of a [`Span`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// 1-based, as every editor counts lines.
    pub line: usize,
    /// 0-based, in **UTF-16 code units** -- what the Language Server
    /// Protocol counts, and not the same as bytes or as `char`s the moment
    /// a document contains an accent or an emoji.
    pub character: Option<usize>,
    /// Byte offset into the original file.
    ///
    /// Carried as well as the line/character pair because the two answer
    /// different questions: an editor wants line and column, and a lint rule
    /// wants to slice the source it is complaining about. Deriving one from
    /// the other at every call site means re-indexing the file at every call
    /// site.
    ///
    /// `#[serde(default)]` so an AST serialised before this field existed
    /// still deserialises; it reads back as 0, which is why nothing should
    /// treat 0 as meaningful without checking `line` too.
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValidationLevel {
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "critical")]
    Critical,
    /// Any other level: what upstream's validator writes for an attribute whose schema gives
    /// another `errorLevel` (`errorLevel || 'error'`, whatever value that is), and a hand-made
    /// token's error's `level`, as JSON holds it.
    #[serde(untagged)]
    Other(serde_json::Value),
}

impl ValidationLevel {
    /// The level a value names: one of the five by their names, any other as it is.
    pub fn from_json(value: &serde_json::Value) -> ValidationLevel {
        match value.as_str() {
            Some("debug") => ValidationLevel::Debug,
            Some("info") => ValidationLevel::Info,
            Some("warning") => ValidationLevel::Warning,
            Some("error") => ValidationLevel::Error,
            Some("critical") => ValidationLevel::Critical,
            _ => ValidationLevel::Other(value.clone()),
        }
    }

    /// The level as upstream's JSON writes it.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ValidationLevel::Debug => "debug".into(),
            ValidationLevel::Info => "info".into(),
            ValidationLevel::Warning => "warning".into(),
            ValidationLevel::Error => "error".into(),
            ValidationLevel::Critical => "critical".into(),
            ValidationLevel::Other(value) => value.clone(),
        }
    }
}

impl ValidationLevel {
    /// Upstream's name of the level (`"warning"`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            ValidationLevel::Debug => "debug",
            ValidationLevel::Info => "info",
            ValidationLevel::Warning => "warning",
            ValidationLevel::Error => "error",
            ValidationLevel::Critical => "critical",
            // A level that is not one of the five (see [`ValidationLevel::to_json`] for it).
            ValidationLevel::Other(_) => "other",
        }
    }

    /// The level upstream names `name`, if it is one of the five.
    pub fn from_name(name: &str) -> Option<ValidationLevel> {
        Some(match name {
            "debug" => ValidationLevel::Debug,
            "info" => ValidationLevel::Info,
            "warning" => ValidationLevel::Warning,
            "error" => ValidationLevel::Error,
            "critical" => ValidationLevel::Critical,
            _ => return None,
        })
    }
}

/// Upstream's `ValidationError`: `{id, level, message, location?}`, and this crate's
/// [`Span`] of what the error is about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub id: String,
    pub level: ValidationLevel,
    pub message: String,
    /// Upstream's `location`, as upstream's error holds it: `None` where it has none (most
    /// errors), else its value, which is a [`Location`] for a `table-syntax` error (the
    /// row's; [`Location::to_json`]) and for a syntax error what the tokenizer gave: `null`
    /// for a tag inline, `{"start": {"offset"}, "end": {"offset"}}` (UTF-16 offsets into the
    /// source) for a tag on its own lines, `{"start": {"line", "character"}, "end": {...}}`
    /// for a tag in a fence; a hand-made token's `meta.error.location` is copied as it is.
    /// `validate()` places an error by it when both its edges have a numeric `line`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub location: Option<serde_json::Value>,
    /// Upstream's error itself, when it is a value the fields above cannot give whole (only a
    /// hand-made token makes one): an entry of the token's `errors` that is not an object
    /// `{id, level, message, location?}` with those keys in that order, string `id` and
    /// `message` and one of the five levels (`5`, `null`, an object with other or more
    /// keys, ...), or the `parse-error` of a `meta.error` whose `message` is not a string.
    /// Upstream's JSON of the AST and of `validate()` writes it as it is (its objects' keys in
    /// a JS object's order); the fields above are then what `String()` makes of its `id`
    /// and `message` (empty when it has none), its `level` when that is one of the five (else
    /// `Error`) and its `location`. `None` for every error Markdoc makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Box<serde_json::Value>>,
    /// This crate's position of what the error is about (the tag, attribute or annotation
    /// that caused it, the node it is on), where the parser or validator knows it: only with
    /// [`crate::parser::ParserArgs::spans`] on. Left out of JSON when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

impl ValidationError {
    /// An error with no location, upstream's or this crate's.
    pub fn new(id: impl Into<String>, level: ValidationLevel, message: impl Into<String>) -> Self {
        ValidationError {
            id: id.into(),
            level,
            message: message.into(),
            location: None,
            raw: None,
            span: None,
        }
    }

    /// The error as upstream's JSON writes it: [`ValidationError::raw`] when it has one, else
    /// `{id, level, message, location?}` (`location` left out when it is `None`).
    pub fn to_json(&self) -> serde_json::Value {
        if let Some(raw) = &self.raw {
            return (**raw).clone();
        }
        let mut o = serde_json::Map::new();
        o.insert("id".into(), self.id.clone().into());
        o.insert("level".into(), self.level.name().into());
        o.insert("message".into(), self.message.clone().into());
        if let Some(location) = &self.location {
            o.insert("location".into(), location.clone());
        }
        serde_json::Value::Object(o)
    }
}

/// A value: what an attribute, an annotation, a variable or a rendered
/// attribute holds.
///
/// `Null` to `Object` are JSON's values. The other three are upstream's
/// objects that are not plain data:
///
/// - `Variable` (`$a.b[0]`) and `Function` (`f(1, x=2)`) are the AST classes
///   an attribute holds until the document is resolved (upstream's
///   `ast/variable.ts` and `ast/function.ts`). A resolved value never contains
///   them.
/// - `Tag` is a rendered tag held as a value: upstream renders a tag's slots
///   into its attributes, and a value a render tree holds (an array a
///   variable resolved to, an attribute) can contain tags.
///
/// Objects keep their keys in the order upstream's do: insertion order, with
/// array-index keys first (see [`js_insert`]). `PartialEq` compares objects
/// as maps, regardless of order.
///
/// Serialized, `Null` to `Object` are what they always were (an `f64`
/// number, as serde writes one), and the other three are upstream's JSON:
/// `{"$$mdtype":"Variable","path":[...]}`,
/// `{"$$mdtype":"Function","name":...,"parameters":{...}}` and
/// `{"$$mdtype":"Tag","name":...,"attributes":{...},"children":[...]}`.
/// Read back, an object whose `$$mdtype` is `"Variable"`, `"Function"` or
/// `"Tag"` is that value and must be well formed; any other object is an
/// `Object`. That is unambiguous for everything upstream parses: its tag
/// grammar drops a `$$mdtype` key from an object literal.
///
/// A value nests as deep as upstream's: the tag grammar's to node's limits
/// (thousands of arrays), a hand-made token's `meta` to any depth. Every
/// operation on one is safe at that depth on a small stack: dropping is a loop,
/// and cloning, comparing, (de)serializing and [`Scalar::to_json`] recurse on a
/// stack that grows ([`crate::depth`]; where it cannot grow, the parser keeps
/// values within a fixed depth that fits a 2 MiB stack). `Debug` writes 400
/// levels and `..` for what is deeper (`{:#?}` indents by the depth, so its
/// output grows with the square of it).
pub enum Scalar {
    /// JavaScript's `undefined`, where upstream's AST holds it as a value: a link node made
    /// of a closing token (`{href: attrs.href}` with no `attrs`), or an attribute a hand-made
    /// token gives no `value`. `JSON.stringify` (and so [`Scalar::to_json`] and `Serialize`)
    /// leaves such an object member out, and writes `null` for it in an array; `validate()`
    /// sees the key (`attribute-type-invalid`) and finds it undefined. Only
    /// `crate::next::parser` makes it; reading JSON never does.
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<Scalar>),
    Object(IndexMap<String, Scalar>),
    Variable(crate::ast::Variable),
    Function(crate::ast::Function),
    Tag(Box<Tag>),
    /// A JavaScript object the variants above cannot hold, as the semantics' value
    /// ([`crate::next::schema::value::Value`]): an object whose prototype is not
    /// `Object.prototype` (a `__proto__` key after an object literal's first, a named
    /// `__proto__` parameter), an array with holes or other own properties (`.4294967294` or
    /// `.b` on an array `class`), an instance of upstream's AST classes with extra or
    /// replaced properties (`.b` on a variable `class`), a hand-made token's `Node`. Only
    /// `crate::next::parser` makes one (reading JSON never does); it is compared by shape and
    /// written as `JSON.stringify` writes it (own enumerable properties; a hole is `null`).
    Js(JsObjectValue),
}

/// A [`crate::next::schema::value::Value`] the AST holds ([`Scalar::Js`]): compared by shape
/// ([`crate::next::schema::value::same_shape`]), written as `JSON.stringify` writes it.
#[derive(Clone)]
pub struct JsObjectValue(crate::next::schema::value::Value);

impl JsObjectValue {
    pub fn new(value: crate::next::schema::value::Value) -> Self {
        JsObjectValue(value)
    }

    /// The value.
    pub fn value(&self) -> &crate::next::schema::value::Value {
        &self.0
    }

    /// `JSON.parse(JSON.stringify(value))` as a `serde_json::Value`: `None` where
    /// `JSON.stringify` throws (a text past V8's string length, a `RangeError` for depth).
    pub fn to_json(&self) -> Option<serde_json::Value> {
        crate::next::schema::value::to_json_value(&self.0)
            .ok()
            .map(|v| v.unwrap_or(serde_json::Value::Null))
    }
}

impl PartialEq for JsObjectValue {
    fn eq(&self, other: &Self) -> bool {
        crate::next::schema::value::same_shape(&self.0, &other.0)
    }
}

impl fmt::Debug for JsObjectValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl Scalar {
    /// The value a JSON object stands for: a `Variable`, `Function` or `Tag`
    /// when its `$$mdtype` says so, else an `Object`.
    pub fn from_object(object: IndexMap<String, Scalar>) -> std::result::Result<Scalar, String> {
        match object.get("$$mdtype") {
            Some(Scalar::String(t)) if t == "Variable" => {
                crate::ast::Variable::from_object(&object).map(Scalar::Variable)
            }
            Some(Scalar::String(t)) if t == "Function" => {
                crate::ast::Function::from_object(&object).map(Scalar::Function)
            }
            Some(Scalar::String(t)) if t == "Tag" => {
                Tag::from_object(object).map(|tag| Scalar::Tag(Box::new(tag)))
            }
            _ => Ok(Scalar::Object(object)),
        }
    }

    /// A value from upstream's JSON (`$$mdtype` objects become `Variable`,
    /// `Function` and `Tag`; object keys are ordered as JS orders them).
    pub fn from_json(value: &serde_json::Value) -> std::result::Result<Scalar, String> {
        Scalar::deserialize(value).map_err(|e| e.to_string())
    }

    /// The value as upstream's `JSON.stringify` writes it: `$$mdtype` objects
    /// for `Variable`, `Function` and `Tag`, keys in this value's order, and
    /// numbers as JS writes them (an integral number without a fraction; a
    /// non-finite one as `null`).
    pub fn to_json(&self) -> serde_json::Value {
        crate::depth::grow!(self.to_json_here())
    }

    #[inline(always)]
    fn to_json_here(&self) -> serde_json::Value {
        use serde_json::Value;
        match self {
            Scalar::Undefined | Scalar::Null => Value::Null,
            Scalar::Boolean(b) => Value::Bool(*b),
            Scalar::Number(n) => json_number(*n),
            Scalar::String(s) => Value::String(s.clone()),
            Scalar::Array(items) => Value::Array(items.iter().map(Scalar::to_json).collect()),
            // A member that is `undefined` is left out.
            Scalar::Object(object) => Value::Object(
                object
                    .iter()
                    .filter(|(_, v)| !matches!(v, Scalar::Undefined))
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
            Scalar::Variable(variable) => variable.to_json(),
            Scalar::Function(function) => function.to_json(),
            Scalar::Tag(tag) => tag.to_json(),
            // What `JSON.stringify` cannot write (past V8's string length) is `null` here.
            Scalar::Js(js) => js.to_json().unwrap_or(Value::Null),
        }
    }
}

/// A number as `JSON.stringify` writes it: integral values that JS prints
/// without an exponent as JSON integers, other finite values as floats,
/// `NaN` and the infinities as `null`.
pub(crate) fn json_number(n: f64) -> serde_json::Value {
    if !n.is_finite() {
        serde_json::Value::Null
    } else if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        // |n| < 2^53: exactly an i64, and below 1e21, where JS switches to
        // an exponent. `-0` is `0`, as in JS.
        serde_json::Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

/// How deep `Debug` writes values: a value deeper is written `..`. (The tag grammar's own
/// fixed limit, where the stack cannot grow, is 300; `{:#?}` indents by the depth, so its
/// output grows with the square of it.)
const MAX_DEBUG_DEPTH: usize = 400;

thread_local! {
    /// How many `Debug::fmt` calls of values are open on this thread.
    static DEBUG_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs `write` one `Debug` level deeper, or writes `..` past [`MAX_DEBUG_DEPTH`].
pub(crate) fn debug_level(
    f: &mut fmt::Formatter<'_>,
    write: impl FnOnce(&mut fmt::Formatter<'_>) -> fmt::Result,
) -> fmt::Result {
    struct Open;
    impl Drop for Open {
        fn drop(&mut self) {
            DEBUG_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
    let depth = DEBUG_DEPTH.with(|d| {
        d.set(d.get() + 1);
        d.get()
    });
    let _open = Open;
    if depth > MAX_DEBUG_DEPTH {
        return f.write_str("..");
    }
    write(f)
}

impl fmt::Debug for Scalar {
    /// As `#[derive(Debug)]` writes it, down to 400 levels.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_level(f, |f| match self {
            Scalar::Undefined => f.write_str("Undefined"),
            Scalar::Null => f.write_str("Null"),
            Scalar::Boolean(b) => f.debug_tuple("Boolean").field(b).finish(),
            Scalar::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Scalar::String(s) => f.debug_tuple("String").field(s).finish(),
            Scalar::Array(items) => f.debug_tuple("Array").field(items).finish(),
            Scalar::Object(object) => f.debug_tuple("Object").field(object).finish(),
            Scalar::Variable(variable) => f.debug_tuple("Variable").field(variable).finish(),
            Scalar::Function(function) => f.debug_tuple("Function").field(function).finish(),
            Scalar::Tag(tag) => f.debug_tuple("Tag").field(tag).finish(),
            Scalar::Js(js) => f.debug_tuple("Js").field(js).finish(),
        })
    }
}

impl Clone for Scalar {
    /// A deep copy, as `#[derive(Clone)]` makes it, a level at a time on a stack with room
    /// for it ([`crate::depth`]).
    fn clone(&self) -> Self {
        crate::depth::grow!(match self {
            Scalar::Undefined => Scalar::Undefined,
            Scalar::Null => Scalar::Null,
            Scalar::Boolean(b) => Scalar::Boolean(*b),
            Scalar::Number(n) => Scalar::Number(*n),
            Scalar::String(s) => Scalar::String(s.clone()),
            Scalar::Array(items) => Scalar::Array(items.clone()),
            Scalar::Object(object) => Scalar::Object(object.clone()),
            Scalar::Variable(variable) => Scalar::Variable(variable.clone()),
            Scalar::Function(function) => Scalar::Function(function.clone()),
            Scalar::Tag(tag) => Scalar::Tag(tag.clone()),
            // (A semantic value: its `Arc`, shared.)
            Scalar::Js(js) => Scalar::Js(js.clone()),
        })
    }
}

impl PartialEq for Scalar {
    /// As `#[derive(PartialEq)]` compares, a level at a time on a stack with room for it.
    fn eq(&self, other: &Scalar) -> bool {
        crate::depth::grow!(match (self, other) {
            (Scalar::Undefined, Scalar::Undefined) | (Scalar::Null, Scalar::Null) => true,
            (Scalar::Boolean(a), Scalar::Boolean(b)) => a == b,
            (Scalar::Number(a), Scalar::Number(b)) => a == b,
            (Scalar::String(a), Scalar::String(b)) => a == b,
            (Scalar::Array(a), Scalar::Array(b)) => a == b,
            (Scalar::Object(a), Scalar::Object(b)) => a == b,
            (Scalar::Variable(a), Scalar::Variable(b)) => a == b,
            (Scalar::Function(a), Scalar::Function(b)) => a == b,
            (Scalar::Tag(a), Scalar::Tag(b)) => a == b,
            (Scalar::Js(a), Scalar::Js(b)) => a == b,
            _ => false,
        })
    }
}

impl Drop for Scalar {
    /// Takes nested values apart in a loop (a value can be nested deeper than a recursive
    /// drop has stack for); a value whose items hold no arrays, objects, calls or tags drops
    /// as the derived drop would.
    fn drop(&mut self) {
        if !holds_containers(self) {
            return;
        }
        let mut stack = Vec::new();
        take_items(self, &mut stack);
        while let Some(mut value) = stack.pop() {
            if holds_containers(&value) {
                take_items(&mut value, &mut stack);
            }
        }
    }
}

/// Is `value` an array, object, call or tag?
fn is_container(value: &Scalar) -> bool {
    matches!(
        value,
        Scalar::Array(_) | Scalar::Object(_) | Scalar::Function(_) | Scalar::Tag(_)
    )
}

/// Does `value` hold an array, object, call or tag (one its drop would recurse into)?
fn holds_containers(value: &Scalar) -> bool {
    match value {
        Scalar::Array(items) => items.iter().any(is_container),
        Scalar::Object(object) => object.values().any(is_container),
        Scalar::Function(function) => function.parameters.values().any(is_container),
        Scalar::Tag(_) => true,
        _ => false,
    }
}

/// Moves what `value` holds onto `stack` (a tag's children as values).
fn take_items(value: &mut Scalar, stack: &mut Vec<Scalar>) {
    match value {
        Scalar::Array(items) => stack.append(items),
        Scalar::Object(object) => stack.extend(std::mem::take(object).into_values()),
        Scalar::Function(function) => {
            stack.extend(std::mem::take(&mut function.parameters).into_values());
        }
        Scalar::Tag(tag) => {
            stack.extend(std::mem::take(&mut tag.attributes).into_values());
            stack.extend(
                std::mem::take(&mut tag.children)
                    .into_iter()
                    .map(RenderableTreeNode::into_scalar),
            );
        }
        _ => {}
    }
}

impl Serialize for Scalar {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        crate::depth::grow!(self.serialize_here(serializer))
    }
}

impl Scalar {
    /// [`Serialize::serialize`] of this level (its items each on a stack with room).
    #[inline(always)]
    fn serialize_here<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            // What `#[serde(untagged)]` wrote for each of these before.
            Scalar::Undefined | Scalar::Null => serializer.serialize_unit(),
            Scalar::Boolean(b) => serializer.serialize_bool(*b),
            Scalar::Number(n) => serializer.serialize_f64(*n),
            Scalar::String(s) => serializer.serialize_str(s),
            Scalar::Array(items) => items.serialize(serializer),
            // A member that is `undefined` is left out, as `JSON.stringify` leaves it out.
            Scalar::Object(object) => serializer.collect_map(
                object
                    .iter()
                    .filter(|(_, v)| !matches!(v, Scalar::Undefined)),
            ),
            Scalar::Variable(variable) => variable.serialize(serializer),
            Scalar::Function(function) => function.serialize(serializer),
            Scalar::Tag(tag) => UpstreamTag(tag).serialize(serializer),
            Scalar::Js(js) => match crate::next::schema::value::to_json_value(js.value()) {
                Ok(json) => json
                    .unwrap_or(serde_json::Value::Null)
                    .serialize(serializer),
                Err(e) => Err(serde::ser::Error::custom(e)),
            },
        }
    }
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        crate::depth::grow!(deserializer.deserialize_any(ScalarVisitor))
    }
}

struct ScalarVisitor;

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = Scalar;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> std::result::Result<Scalar, E> {
        Ok(Scalar::Boolean(v))
    }

    fn visit_i64<E>(self, v: i64) -> std::result::Result<Scalar, E> {
        Ok(Scalar::Number(v as f64))
    }

    fn visit_u64<E>(self, v: u64) -> std::result::Result<Scalar, E> {
        Ok(Scalar::Number(v as f64))
    }

    fn visit_f64<E>(self, v: f64) -> std::result::Result<Scalar, E> {
        Ok(Scalar::Number(v))
    }

    fn visit_str<E>(self, v: &str) -> std::result::Result<Scalar, E> {
        Ok(Scalar::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> std::result::Result<Scalar, E> {
        Ok(Scalar::String(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> std::result::Result<Scalar, E> {
        std::str::from_utf8(v)
            .map(|s| Scalar::String(s.to_owned()))
            .map_err(|_| E::invalid_value(de::Unexpected::Bytes(v), &self))
    }

    fn visit_unit<E>(self) -> std::result::Result<Scalar, E> {
        Ok(Scalar::Null)
    }

    fn visit_none<E>(self) -> std::result::Result<Scalar, E> {
        Ok(Scalar::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> std::result::Result<Scalar, D::Error> {
        Scalar::deserialize(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Scalar, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Scalar::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Scalar, A::Error> {
        // Set in a JsObject (no shifting, whatever order the keys come in), then laid out
        // in a JS object's order.
        let mut object = JsObject::new();
        while let Some((key, value)) = map.next_entry::<String, Scalar>()? {
            object.insert(key, value);
        }
        Scalar::from_object(object.into()).map_err(de::Error::custom)
    }
}

/// Is `key` an array index (the decimal form of an integer from 0 to
/// 2^32 - 2, no sign, no leading zero)? A JS object lists such keys before
/// its others, in ascending order.
pub fn is_array_index(key: &str) -> bool {
    array_index(key).is_some()
}

/// The integer an array-index key (see [`is_array_index`]) names.
pub fn array_index(key: &str) -> Option<u32> {
    match key.as_bytes() {
        [b'0'] => Some(0),
        [b'1'..=b'9', rest @ ..] if rest.len() < 10 && rest.iter().all(u8::is_ascii_digit) => key
            .parse::<u64>()
            .ok()
            .filter(|&n| n <= 4_294_967_294)
            .map(|n| n as u32),
        _ => None,
    }
}

/// `object[key] = value` on an object kept in a JS object's key order: an
/// existing key keeps its place; a new array-index key (see
/// [`is_array_index`]) goes after the smaller ones, in front of every other
/// key; any other new key goes last. What every JS object upstream builds
/// does -- `{b: 1, 0: 2}` lists `0` first, and so does a function's
/// parameters for `f(x=1, 2)` -- and so what `Object.entries`, a spread and
/// `JSON.stringify` see. Returns the value it replaced.
///
/// `object` must already be in that order (as this function keeps it): the
/// place of a new index key is found by binary search over the leading index
/// keys, in O(log n). A key that goes last (every new key that is not an
/// index, and an index above the others while no other key follows them, as
/// JSON written by `JSON.stringify` or the tag grammar gives them) is
/// appended; one that goes before others shifts them, as an `IndexMap` does.
/// To build an object from keys in any order without shifting, use
/// [`JsObject`] (two maps), which is what the semantic values hold.
pub fn js_insert<V>(
    object: &mut IndexMap<String, V>,
    key: impl Into<String>,
    value: V,
) -> Option<V> {
    let key = key.into();
    if let Some(slot) = object.get_mut(&key) {
        return Some(std::mem::replace(slot, value));
    }
    match array_index(&key) {
        Some(index) => {
            // The index keys are a prefix of the keys, ascending: the first key that is not
            // an index, or is a larger one, is where this one goes.
            let at = object.partition_point(|k, _| array_index(k).is_some_and(|n| n < index));
            if at == object.len() {
                object.insert(key, value);
            } else {
                object.shift_insert(at, key, value);
            }
        }
        None => {
            object.insert(key, value);
        }
    }
    None
}

/// A JavaScript object's own properties, in a JS object's key order: the
/// array-index keys (see [`is_array_index`]) ascending, then every other key
/// in the order it was first set (`OrdinaryOwnPropertyKeys`). What
/// `Object.entries`, a spread, `for...in` and `JSON.stringify` see of an
/// object upstream builds.
///
/// Held as two maps, as V8 holds an object's elements apart from its named
/// properties: the index keys in a `BTreeMap` by their number, the others in
/// an `IndexMap` in insertion order. Nothing ever shifts: a lookup or an
/// insert is O(log n) for an index key and O(1) for any other, whatever order
/// the keys come in.
///
/// The API is an ordered map's: [`get`](JsObject::get),
/// [`insert`](JsObject::insert) (`object[key] = value`, which keeps an
/// existing key's place), [`iter`](JsObject::iter) in the order above, and
/// conversions from and to an [`IndexMap`] in that order. Equality is a
/// map's (the same keys with equal values), as an `IndexMap`'s is.
#[derive(Clone)]
pub struct JsObject<V> {
    /// The array-index keys by number, each with its text.
    indexed: std::collections::BTreeMap<u32, (String, V)>,
    /// The other keys, in insertion order.
    named: IndexMap<String, V>,
}

impl<V> Default for JsObject<V> {
    fn default() -> Self {
        JsObject {
            indexed: std::collections::BTreeMap::new(),
            named: IndexMap::new(),
        }
    }
}

impl<V> JsObject<V> {
    /// `{}`.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of own properties.
    pub fn len(&self) -> usize {
        self.indexed.len() + self.named.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indexed.is_empty() && self.named.is_empty()
    }

    /// The own property `key`'s value.
    pub fn get(&self, key: &str) -> Option<&V> {
        match array_index(key) {
            Some(n) => self.indexed.get(&n).map(|(_, v)| v),
            None => self.named.get(key),
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        match array_index(key) {
            Some(n) => self.indexed.get_mut(&n).map(|(_, v)| v),
            None => self.named.get_mut(key),
        }
    }

    /// `Object.prototype.hasOwnProperty.call(object, key)`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// `object[key] = value` for a key that is a plain data property: an existing key
    /// keeps its place and gets the value (the old one is returned); a new key goes where a
    /// JS object lists it. (`__proto__` is a key like any other here; where JS's
    /// assignment calls the prototype setter instead, see
    /// [`crate::next::schema::value::PlainObject`].)
    pub fn insert(&mut self, key: impl Into<String>, value: V) -> Option<V> {
        let key = key.into();
        match array_index(&key) {
            Some(n) => match self.indexed.entry(n) {
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    Some(std::mem::replace(&mut slot.get_mut().1, value))
                }
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((key, value));
                    None
                }
            },
            None => self.named.insert(key, value),
        }
    }

    /// `delete object[key]`: the value, if the key was there. The keys after it keep their
    /// order (O(n) for a key that is not an index).
    pub fn remove(&mut self, key: &str) -> Option<V> {
        match array_index(key) {
            Some(n) => self.indexed.remove(&n).map(|(_, v)| v),
            None => self.named.shift_remove(key),
        }
    }

    /// The properties in order.
    pub fn iter(&self) -> JsObjectIter<'_, V> {
        JsObjectIter {
            indexed: self.indexed.values(),
            named: self.named.iter(),
        }
    }

    /// The properties in order, values mutable.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut V)> {
        self.indexed
            .values_mut()
            .map(|(k, v)| (&*k, v))
            .chain(self.named.iter_mut())
    }

    /// The keys in order.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.iter().map(|(k, _)| k)
    }

    /// The values in key order (`Object.values`).
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.iter_mut().map(|(_, v)| v)
    }

    /// The values in key order, owned.
    pub fn into_values(self) -> impl Iterator<Item = V> {
        self.indexed
            .into_values()
            .map(|(_, v)| v)
            .chain(self.named.into_values())
    }
}

/// The properties of a [`JsObject`] in order ([`JsObject::iter`]).
pub struct JsObjectIter<'a, V> {
    indexed: std::collections::btree_map::Values<'a, u32, (String, V)>,
    named: indexmap::map::Iter<'a, String, V>,
}

impl<'a, V> Iterator for JsObjectIter<'a, V> {
    type Item = (&'a String, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self.indexed.next() {
            Some((k, v)) => Some((k, v)),
            None => self.named.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.indexed.len() + self.named.len();
        (n, Some(n))
    }
}

impl<V> DoubleEndedIterator for JsObjectIter<'_, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self.named.next_back() {
            Some(entry) => Some(entry),
            None => self.indexed.next_back().map(|(k, v)| (k, v)),
        }
    }
}

impl<V> ExactSizeIterator for JsObjectIter<'_, V> {}

impl<'a, V> IntoIterator for &'a JsObject<V> {
    type Item = (&'a String, &'a V);
    type IntoIter = JsObjectIter<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<V> IntoIterator for JsObject<V> {
    type Item = (String, V);
    type IntoIter = std::iter::Chain<
        std::iter::Map<
            std::collections::btree_map::IntoValues<u32, (String, V)>,
            fn((String, V)) -> (String, V),
        >,
        indexmap::map::IntoIter<String, V>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        let same: fn((String, V)) -> (String, V) = |entry| entry;
        self.indexed.into_values().map(same).chain(self.named)
    }
}

impl<K: Into<String>, V> FromIterator<(K, V)> for JsObject<V> {
    /// `object[key] = value` for each entry in turn.
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> Self {
        let mut object = JsObject::new();
        object.extend(entries);
        object
    }
}

impl<K: Into<String>, V> Extend<(K, V)> for JsObject<V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, entries: I) {
        for (k, v) in entries {
            self.insert(k, v);
        }
    }
}

impl<K: Into<String>, V, const N: usize> From<[(K, V); N]> for JsObject<V> {
    fn from(entries: [(K, V); N]) -> Self {
        entries.into_iter().collect()
    }
}

impl<V> From<IndexMap<String, V>> for JsObject<V> {
    /// The map's entries set in its order: a map not in a JS object's order (index keys
    /// first, ascending) comes out in it.
    fn from(map: IndexMap<String, V>) -> Self {
        map.into_iter().collect()
    }
}

impl<V> From<JsObject<V>> for IndexMap<String, V> {
    /// The properties in a JS object's order (what [`js_insert`] keeps a map in).
    fn from(object: JsObject<V>) -> Self {
        let mut map = IndexMap::with_capacity(object.len());
        map.extend(object);
        map
    }
}

impl<V> std::ops::Index<&str> for JsObject<V> {
    type Output = V;

    fn index(&self, key: &str) -> &V {
        self.get(key).expect("no such key in the JsObject")
    }
}

impl<V: PartialEq> PartialEq for JsObject<V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl<V: fmt::Debug> fmt::Debug for JsObject<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeValue {
    #[serde(rename = "type")]
    pub attr_type: String,
    pub name: String,
    pub value: Scalar,
    /// The annotation as upstream holds it, when it is not the attribute object `{type, name,
    /// value}` these fields describe: upstream pushes whatever a token's `attributes` list
    /// holds onto `annotations` (a hand-made token's `5`, `"ab"`, `[1]`, or an object with
    /// other keys, another key order or no `value`). `attr_type`, `name` and `value` are then
    /// what the parser read from it (`String()` of a `type` or `name` that is no string, `""`
    /// and `undefined` for what it lacks). `None` for every annotation Markdoc's tokenizer
    /// makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Box<Scalar>>,
}

/// A render-tree tag (upstream's `Tag`: `name`, `attributes`, `children`).
///
/// Serialized on its own (and as a `RenderableTreeNode`) it is
/// `{"name","attributes","children"}`, as it always was; held in a
/// [`Scalar::Tag`] it is upstream's `{"$$mdtype":"Tag",...}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub attributes: IndexMap<String, Scalar>,
    pub children: Vec<RenderableTreeNode>,
}

impl Tag {
    /// A tag from upstream's JSON object (`$$mdtype` `"Tag"`, a `name`, an
    /// `attributes` object and a `children` array). A `null` name is the
    /// empty one: both make upstream's renderers render only the children.
    pub fn from_object(mut object: IndexMap<String, Scalar>) -> std::result::Result<Tag, String> {
        let name = match object.shift_remove("name").as_mut() {
            Some(Scalar::String(name)) => std::mem::take(name),
            Some(Scalar::Null) => String::new(),
            other => return Err(format!("a Tag's `name` is a string, not {other:?}")),
        };
        let attributes = match object.shift_remove("attributes").as_mut() {
            Some(Scalar::Object(attributes)) => std::mem::take(attributes),
            other => return Err(format!("a Tag's `attributes` is an object, not {other:?}")),
        };
        let children = match object.shift_remove("children").as_mut() {
            Some(Scalar::Array(children)) => std::mem::take(children)
                .into_iter()
                .map(RenderableTreeNode::from)
                .collect(),
            other => return Err(format!("a Tag's `children` is an array, not {other:?}")),
        };
        Ok(Tag {
            name,
            attributes,
            children,
        })
    }

    /// Upstream's JSON of the tag: `{"$$mdtype":"Tag","name","attributes","children"}`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut object = serde_json::Map::new();
        object.insert("$$mdtype".into(), "Tag".into());
        object.insert("name".into(), self.name.clone().into());
        object.insert(
            "attributes".into(),
            serde_json::Value::Object(
                self.attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
        );
        object.insert(
            "children".into(),
            serde_json::Value::Array(
                self.children
                    .iter()
                    .map(RenderableTreeNode::to_json)
                    .collect(),
            ),
        );
        serde_json::Value::Object(object)
    }
}

/// A [`Tag`] serialized as upstream's JSON, children included.
struct UpstreamTag<'a>(&'a Tag);

impl Serialize for UpstreamTag<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        /// The children as values: a tag as upstream's JSON, each on a stack with room.
        struct Children<'a>(&'a [RenderableTreeNode]);
        struct Child<'a>(&'a RenderableTreeNode);
        impl Serialize for Child<'_> {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                match self.0 {
                    RenderableTreeNode::Tag(tag) => {
                        crate::depth::grow!(UpstreamTag(tag).serialize(serializer))
                    }
                    RenderableTreeNode::Scalar(value) => value.serialize(serializer),
                }
            }
        }
        impl Serialize for Children<'_> {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_seq(self.0.iter().map(Child))
            }
        }
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("$$mdtype", "Tag")?;
        map.serialize_entry("name", &self.0.name)?;
        map.serialize_entry("attributes", &self.0.attributes)?;
        map.serialize_entry("children", &Children(&self.0.children))?;
        map.end()
    }
}

/// A node of a render tree read from JSON: a tag, or any other value.
///
/// Its `Clone`, `PartialEq`, `Debug` and serde are `#[derive]`'s (serde's `untagged`), a
/// level at a time on a stack with room for it ([`crate::depth`]), so a tree as deep as
/// a value may be is safe on a small stack (see [`Scalar`]); `Debug` writes 400 levels.
pub enum RenderableTreeNode {
    Tag(Box<Tag>),
    Scalar(Scalar),
}

/// What `#[derive(Serialize)] #[serde(untagged)]` writes for a [`RenderableTreeNode`].
#[derive(Serialize)]
#[serde(untagged)]
enum RenderableTreeNodeRef<'a> {
    Tag(&'a Tag),
    Scalar(&'a Scalar),
}

/// What `#[derive(Deserialize)] #[serde(untagged)]` reads for a [`RenderableTreeNode`].
#[derive(Deserialize)]
#[serde(untagged)]
enum RenderableTreeNodeOwned {
    Tag(Box<Tag>),
    Scalar(Scalar),
}

impl Serialize for RenderableTreeNode {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        crate::depth::grow!(match self {
            RenderableTreeNode::Tag(tag) => RenderableTreeNodeRef::Tag(tag).serialize(serializer),
            RenderableTreeNode::Scalar(value) => {
                RenderableTreeNodeRef::Scalar(value).serialize(serializer)
            }
        })
    }
}

impl<'de> Deserialize<'de> for RenderableTreeNode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        crate::depth::grow!({
            Ok(match RenderableTreeNodeOwned::deserialize(deserializer)? {
                RenderableTreeNodeOwned::Tag(tag) => RenderableTreeNode::Tag(tag),
                RenderableTreeNodeOwned::Scalar(value) => RenderableTreeNode::Scalar(value),
            })
        })
    }
}

impl Clone for RenderableTreeNode {
    fn clone(&self) -> Self {
        crate::depth::grow!(match self {
            RenderableTreeNode::Tag(tag) => RenderableTreeNode::Tag(tag.clone()),
            RenderableTreeNode::Scalar(value) => RenderableTreeNode::Scalar(value.clone()),
        })
    }
}

impl PartialEq for RenderableTreeNode {
    fn eq(&self, other: &RenderableTreeNode) -> bool {
        crate::depth::grow!(match (self, other) {
            (RenderableTreeNode::Tag(a), RenderableTreeNode::Tag(b)) => a == b,
            (RenderableTreeNode::Scalar(a), RenderableTreeNode::Scalar(b)) => a == b,
            _ => false,
        })
    }
}

impl fmt::Debug for RenderableTreeNode {
    /// As `#[derive(Debug)]` writes it, down to 400 levels.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_level(f, |f| match self {
            RenderableTreeNode::Tag(tag) => f.debug_tuple("Tag").field(tag).finish(),
            RenderableTreeNode::Scalar(value) => f.debug_tuple("Scalar").field(value).finish(),
        })
    }
}

impl RenderableTreeNode {
    /// The node as a value: a tag is [`Scalar::Tag`].
    pub fn into_scalar(self) -> Scalar {
        match self {
            RenderableTreeNode::Tag(tag) => Scalar::Tag(tag),
            RenderableTreeNode::Scalar(value) => value,
        }
    }

    /// Upstream's JSON of the node: a tag as `{"$$mdtype":"Tag",...}`, a
    /// value as [`Scalar::to_json`] writes it.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            RenderableTreeNode::Tag(tag) => tag.to_json(),
            RenderableTreeNode::Scalar(value) => value.to_json(),
        }
    }

    /// A render-tree node from upstream's JSON: `{"$$mdtype":"Tag",...}` is
    /// a `Tag`, anything else a `Scalar` (which may itself hold tags).
    pub fn from_json(value: &serde_json::Value) -> std::result::Result<RenderableTreeNode, String> {
        Scalar::from_json(value).map(RenderableTreeNode::from)
    }
}

impl From<Scalar> for RenderableTreeNode {
    /// `Scalar::Tag` becomes `RenderableTreeNode::Tag`; any other value stays a value.
    fn from(mut value: Scalar) -> Self {
        match &mut value {
            Scalar::Tag(tag) => RenderableTreeNode::Tag(std::mem::take(tag)),
            _ => RenderableTreeNode::Scalar(value),
        }
    }
}

#[cfg(test)]
mod representation_tests {
    use super::*;
    use crate::ast::{Function, Node, PathSegment, Variable};
    use std::collections::HashMap;

    #[test]
    fn js_insert_orders_keys_as_a_js_object_does() {
        // node: const o = {b: 1}; o["4294967295"] = 2; o["4294967294"] = 3; o["01"] = 4;
        // o[0] = 5; o["-1"] = 6; o["1.5"] = 7; o["10"] = 8; o["9"] = 9; Object.keys(o)
        let mut o = IndexMap::new();
        for (k, v) in [
            ("b", 1),
            ("4294967295", 2),
            ("4294967294", 3),
            ("01", 4),
            ("0", 5),
            ("-1", 6),
            ("1.5", 7),
            ("10", 8),
            ("9", 9),
        ] {
            js_insert(&mut o, k, v);
        }
        let keys: Vec<&str> = o.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "0",
                "9",
                "10",
                "4294967294",
                "b",
                "4294967295",
                "01",
                "-1",
                "1.5"
            ]
        );
        // An existing key keeps its place.
        assert_eq!(js_insert(&mut o, "b", 10), Some(1));
        assert_eq!(o.get_index_of("b"), Some(4));
    }

    #[test]
    fn js_insert_finds_its_place_by_binary_search() {
        // The same order as one insertion at a time from the front.
        let keys = [
            "7",
            "b",
            "3",
            "10",
            "a",
            "0",
            "4294967294",
            "9",
            "c",
            "3",
            "2",
        ];
        let mut o = IndexMap::new();
        for (i, k) in keys.iter().enumerate() {
            js_insert(&mut o, *k, i);
        }
        let got: Vec<(&str, usize)> = o.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        // node: const o = {}; keys.forEach((k, i) => o[k] = i); Object.entries(o)
        assert_eq!(
            got,
            [
                ("0", 5),
                ("2", 10),
                ("3", 9),
                ("7", 0),
                ("9", 7),
                ("10", 3),
                ("4294967294", 6),
                ("b", 1),
                ("a", 4),
                ("c", 8)
            ]
        );
        // 20,000 index keys in order (the parser's `class={} .0 ... .19999`) are appended.
        let mut big = IndexMap::new();
        for i in 0..20_000 {
            js_insert(&mut big, i.to_string(), ());
        }
        assert!(big.keys().enumerate().all(|(i, k)| *k == i.to_string()));
    }

    #[test]
    fn a_js_object_keeps_index_keys_apart_from_the_others() {
        // node: const o = {}; for (const k of ['b', '1', '0', 'a', '4294967295',
        //   '4294967294', '01', '-0']) o[k] = k; o.b = 'B'; Object.keys(o)
        let mut o = JsObject::new();
        for k in ["b", "1", "0", "a", "4294967295", "4294967294", "01", "-0"] {
            assert_eq!(o.insert(k, k.to_string()), None);
        }
        assert_eq!(o.insert("b", "B".to_string()), Some("b".to_string()));
        assert_eq!(
            o.keys().map(String::as_str).collect::<Vec<_>>(),
            ["0", "1", "4294967294", "b", "a", "4294967295", "01", "-0"]
        );
        assert_eq!(o.len(), 8);
        assert_eq!(o.get("b").map(String::as_str), Some("B"));
        assert_eq!(o["4294967294"], "4294967294");
        assert!(o.contains_key("01") && !o.contains_key("1.0"));
        // `delete o[1]; delete o.a`: the others keep their order.
        assert_eq!(o.remove("1").as_deref(), Some("1"));
        assert_eq!(o.remove("a").as_deref(), Some("a"));
        assert_eq!(o.remove("a"), None);
        let back: IndexMap<String, String> = o.clone().into();
        assert_eq!(
            back.keys().map(String::as_str).collect::<Vec<_>>(),
            ["0", "4294967294", "b", "4294967295", "01", "-0"]
        );
        // Read from a map in any order, it comes out in a JS object's order.
        let from: JsObject<i32> = IndexMap::from([
            ("z".to_string(), 1),
            ("10".to_string(), 2),
            ("2".to_string(), 3),
        ])
        .into();
        assert_eq!(
            from.iter()
                .map(|(k, v)| (k.as_str(), *v))
                .collect::<Vec<_>>(),
            [("2", 3), ("10", 2), ("z", 1)]
        );
        assert_eq!(from.iter().next_back(), Some((&"z".to_string(), &1)));
        assert_eq!(from.iter().len(), 3);
        // Equality is a map's.
        let other: JsObject<i32> = [("2", 3), ("z", 1), ("10", 2)].into();
        assert_eq!(from, other);
        assert_eq!(format!("{from:?}"), r#"{"2": 3, "10": 2, "z": 1}"#);
    }

    #[test]
    fn a_js_object_takes_index_keys_in_any_order_without_shifting() {
        // 80,000 index keys descending, after as many named ones: two maps, no shifting.
        let mut o = JsObject::new();
        for i in 0..80_000 {
            o.insert(format!("k{i}"), i);
        }
        for i in (0..80_000).rev() {
            o.insert(i.to_string(), i);
        }
        let keys: Vec<&String> = o.keys().collect();
        assert_eq!(keys.len(), 160_000);
        assert!(
            keys[..80_000]
                .iter()
                .enumerate()
                .all(|(i, k)| **k == i.to_string())
        );
        assert_eq!(keys[80_000], "k0");
    }

    #[test]
    fn upstream_values_serialize_as_upstream_writes_them() {
        let variable = Scalar::Variable(Variable::new(vec!["a".into(), PathSegment::Number(0.0)]));
        assert_eq!(
            serde_json::to_string(&variable).unwrap(),
            r#"{"$$mdtype":"Variable","path":["a",0.0]}"#
        );
        assert_eq!(
            crate::next::compare::to_json_string(&variable.to_json()),
            r#"{"$$mdtype":"Variable","path":["a",0]}"#
        );
        let function = Scalar::Function(Function::new(
            "f".into(),
            IndexMap::from([
                ("0".to_string(), Scalar::Number(1.0)),
                ("x".to_string(), variable.clone()),
            ]),
        ));
        let tag = Scalar::Tag(Box::new(Tag {
            name: "p".into(),
            attributes: IndexMap::new(),
            children: vec![RenderableTreeNode::Tag(Box::new(Tag {
                name: "b".into(),
                attributes: IndexMap::new(),
                children: vec![RenderableTreeNode::Scalar(Scalar::String("x".into()))],
            }))],
        }));
        assert_eq!(
            serde_json::to_string(&tag).unwrap(),
            r#"{"$$mdtype":"Tag","name":"p","attributes":{},"children":[{"$$mdtype":"Tag","name":"b","attributes":{},"children":["x"]}]}"#
        );
        for value in [variable, function, tag] {
            let back: Scalar =
                serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
            assert_eq!(back, value);
        }
    }

    #[test]
    fn the_old_values_serialize_as_before() {
        let object = Scalar::Object(IndexMap::from([
            ("b".to_string(), Scalar::Number(1.0)),
            (
                "a".to_string(),
                Scalar::Array(vec![Scalar::Null, Scalar::Boolean(true)]),
            ),
        ]));
        assert_eq!(
            serde_json::to_string(&object).unwrap(),
            r#"{"b":1.0,"a":[null,true]}"#
        );
        assert_eq!(
            serde_json::to_string(&Scalar::String("s".into())).unwrap(),
            r#""s""#
        );
        let read: Scalar = serde_json::from_str(r#"{"x": 1, "y": "z", "n": null}"#).unwrap();
        assert_eq!(
            read,
            Scalar::Object(IndexMap::from([
                ("x".to_string(), Scalar::Number(1.0)),
                ("y".to_string(), Scalar::String("z".into())),
                ("n".to_string(), Scalar::Null),
            ]))
        );
        // YAML (frontmatter, schema files) reads the same way.
        let yaml: Scalar = serde_yaml::from_str("a: [1, two]\nb: ~").unwrap();
        assert_eq!(
            serde_json::to_string(&yaml).unwrap(),
            r#"{"a":[1.0,"two"],"b":null}"#
        );
    }

    #[test]
    fn deserialization_is_unambiguous() {
        // An object is a Variable, Function or Tag only by its $$mdtype.
        let plain: Scalar = serde_json::from_str(r#"{"path": ["a"]}"#).unwrap();
        assert!(matches!(plain, Scalar::Object(_)));
        let node: Scalar = serde_json::from_str(r#"{"$$mdtype": "Node"}"#).unwrap();
        assert!(matches!(node, Scalar::Object(_)));
        let variable: Scalar =
            serde_json::from_str(r#"{"path": ["a", 1], "$$mdtype": "Variable"}"#).unwrap();
        assert!(matches!(variable, Scalar::Variable(_)));
        // ... and then must be well formed.
        assert!(serde_json::from_str::<Scalar>(r#"{"$$mdtype": "Variable"}"#).is_err());
        assert!(
            serde_json::from_str::<Scalar>(r#"{"$$mdtype": "Function", "name": "f"}"#).is_err()
        );
        // JSON's object order is kept, read as a JS object reads it.
        let object: Scalar = serde_json::from_str(r#"{"b": 1, "2": 2, "1": 3}"#).unwrap();
        assert_eq!(
            crate::next::compare::to_json_string(&object.to_json()),
            r#"{"1":3,"2":2,"b":1}"#
        );
    }

    #[test]
    fn node_types_outside_the_list_are_other() {
        assert_eq!(NodeType::from_name("s"), NodeType::Strikethrough);
        assert_eq!(
            NodeType::from_name("html_block"),
            NodeType::Other("html_block".into())
        );
        assert_eq!(
            NodeType::Other("html_inline".into()).to_string(),
            "html_inline"
        );
        let read: NodeType = serde_json::from_str(r#""html_block""#).unwrap();
        assert_eq!(read, NodeType::Other("html_block".into()));
        assert_eq!(serde_json::to_string(&read).unwrap(), r#""html_block""#);
        let known: NodeType = serde_json::from_str(r#""heading""#).unwrap();
        assert_eq!(known, NodeType::Heading);
    }

    #[test]
    fn node_new_takes_any_pairs() {
        let from_hash = Node::new(
            NodeType::Paragraph,
            HashMap::from([("a".to_string(), Scalar::Null)]),
            Vec::new(),
            None,
        );
        let from_vec = Node::new(
            NodeType::Paragraph,
            vec![
                ("b".to_string(), Scalar::Null),
                ("a".to_string(), Scalar::Null),
            ],
            Vec::new(),
            None,
        );
        assert_eq!(from_hash.attributes.len(), 1);
        assert_eq!(from_vec.attributes.keys().collect::<Vec<_>>(), ["b", "a"]);
        let empty = Node::new(NodeType::Document, HashMap::new(), Vec::new(), None);
        assert!(empty.attributes.is_empty() && empty.slots.is_empty());
    }
}
