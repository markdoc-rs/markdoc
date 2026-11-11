//! Port of upstream Markdoc v0.5.11's `src/formatter.ts`: [`fn@format`] writes an AST back out
//! as Markdoc source, as `Markdoc.format(ast, options)` does.
//!
//! Owner: the api agent.
//!
//! # How the generators are ported
//!
//! Upstream's formatter is a tree of generators. Each node's `formatNode` yields values,
//! nearly always strings (a table row yields the array of its cells, `thead` its first row,
//! and a fence the `language` attribute as it is), to whatever consumes it, and there are
//! three kinds of consumer:
//!
//! - `format` itself, which appends each value to the document (`doc += s`, JavaScript's
//!   `+`, so a value that is not a string becomes one, or throws);
//! - `trimStart`, which calls `trimStart()` on the values it sees until one is not empty
//!   (a value that is not a string throws), and passes that one and the rest on;
//! - a spread (`[...g]`), which collects every value for a `join`, a table or a row.
//!
//! Every consumer takes every value a generator yields, in order, so running each generator
//! eagerly into its consumer (a `Sink`) does what upstream does in the same order: what
//! throws first here is what throws first upstream, after the same values were consumed.
//! The values themselves are `Piece`s: strings, arrays of pieces, and attribute values
//! yielded as they are.
//!
//! Every JavaScript operation upstream applies to a value is made with JS's semantics:
//! `String()` and `+` (an object with an own `toString` throws `TypeError: Cannot convert
//! object to primitive value`), `.length`, `JSON.stringify`, `Object.entries`/`keys`/
//! `values`, `String.prototype.repeat` (`RangeError: Invalid count value: -1` for a negative
//! count, `RangeError: Invalid string length` past V8's longest string), the regular
//! expressions `formatter.ts` uses (over UTF-16, with JS's whitespace), and the `TypeError`s
//! V8 throws where upstream calls a method a value does not have (`n.attributes.content.match
//! is not a function` for a fence whose `content` an annotation made a number). The
//! messages are V8's, word for word.
//!
//! # Call stack depth
//!
//! Upstream's generators nest as the AST and its values nest, until V8's stack runs out:
//! node throws `RangeError: Maximum call stack size exceeded` at a depth that depends on the
//! shape (a level of a list costs more frames than a level of tags) and on how warm V8 is.
//! The port charges each level to a budget of node's stack ([`crate::depth`]), with
//! each recursion site's cost solved from node's fresh-process limits ([`NODE_LIMITS`]), and
//! returns the same error past it; it grows its own stack as it goes, so any depth node
//! takes fits. Where the stack cannot grow (the `stack-growth` feature off, or wasm), it stops
//! where a 2 MiB stack would run low in a debug build instead, below node's limits.
//!
//! # Objects that say they are nodes
//!
//! An attribute value that is an object whose `$$mdtype` is `"Node"` (only hand-made JSON makes
//! one: the tag grammar drops a `$$mdtype` key) is formatted as the node `Ast.fromJSON` revives
//! it as (`Object.assign(new Node(), value)`: a field it leaves out has the constructor's
//! default), with JavaScript's semantics for fields of any type, as upstream's `formatNode`
//! reads any object: `children: 5` throws `TypeError: a.children is not iterable` where
//! `formatChildren` iterates it, a string's characters are its children, a `type` that is not
//! a string formats nothing, `Object.entries` of such an object lists the constructor's fields
//! first (`findings/conformance/work/api/fuzz_api.py nodes` checks them against node).

use crate::ast::{Function, Node, PathSegment, Variable};
use crate::markdown_it::js;
use crate::next::schema::value::{self, quote_json_string, scalar_to_js_string};
use crate::next::stack;
use crate::next::tokenizer::utils::is_identifier;
use crate::types::{AttributeValue, MarkdocError, NodeType, Result, Scalar};
use indexmap::IndexMap;
use std::borrow::Cow;

const SPACE: &str = " ";
/// Value separator.
const SEP: &str = ", ";
/// Newline.
const NL: &str = "\n";
/// Ordered list.
const OL: &str = ".";
/// Unordered list.
const UL: &str = "-";
const OPEN: &str = "{%";
const CLOSE: &str = "%}";

const MAX_TAG_OPENING_WIDTH: f64 = 80.0;

const WRAPPING_TYPES: [&str; 3] = ["strong", "em", "s"];

/// V8's longest string, in UTF-16 code units (`String::kMaxLength` on a 64-bit host,
/// 2^29 - 24): a longer result throws `RangeError: Invalid string length`.
pub const MAX_STRING_LENGTH: usize = (1 << 29) - 24;

/// `orderedListMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderedListMode {
    /// `'increment'`: an ordered list's items are numbered up from its start.
    Increment,
    /// `'repeat'` (or anything but `'increment'`): the first item has the list's start and
    /// every other `1`.
    Repeat,
}

/// `format`'s options (formatter.ts's `Options`). Upstream reads each with JavaScript's
/// truthiness: `allowIndentation` and `maxTagOpeningWidth` that are falsy are the defaults.
#[derive(Clone, Debug, Default)]
pub struct FormatOptions<'a> {
    /// `allowIndentation`: a tag's children are indented two columns more than the tag.
    pub allow_indentation: bool,
    /// `maxTagOpeningWidth` (80 when `None`, `0` or `NaN`): a block tag whose opening would
    /// be wider is written one attribute per line.
    pub max_tag_opening_width: Option<f64>,
    /// `orderedListMode` (`None` is `'repeat'`).
    pub ordered_list_mode: Option<OrderedListMode>,
    /// `parent`: the node the formatted node is a child of (a text in `strong`, `em` or `s`
    /// escapes `*`, `_` and `~`; a table in a `table` tag is written as a list).
    pub parent: Option<&'a Node>,
    /// `indent`: the columns the node's lines are indented by.
    pub indent: Option<f64>,
}

/// `format(node, options)`: the Markdoc source of an AST node (a document, or any node).
/// `Err` where upstream throws (see the module's documentation), with upstream's error
/// (`index::thrown` gives `Name: message`).
pub fn format(node: &Node, options: &FormatOptions<'_>) -> Result<String> {
    let o = Opts::from(options);
    run(|out| format_node(NodeRef::Node(node), &o, out))
}

/// `format(nodes, options)`: each node formatted after the other (`formatValue` of an
/// array), the result's leading whitespace trimmed.
pub fn format_all(nodes: &[Node], options: &FormatOptions<'_>) -> Result<String> {
    let o = Opts::from(options);
    run(|out| {
        for node in nodes {
            format_node(NodeRef::Node(node), &o, out)?;
        }
        Ok(())
    })
}

/// `format(value, options)` of an AST value that is not a node: `null` and `undefined` are
/// `''`, a primitive its `toString()`, a `Variable` or `Function` its source (`$a.b[0]`,
/// `f(1, "x")`), an array its items one after the other; any other object throws `Error:
/// Unimplemented: "..."` (its `$$mdtype`).
pub fn format_value(value: &Scalar, options: &FormatOptions<'_>) -> Result<String> {
    let o = Opts::from(options);
    run(|out| format_scalar_value(value, &o, out))
}

// --- call stack depth ----------------------------------------------------------------------------

/// How deep node 22 (V8's default stack, `Markdoc.format` called from the top of a script)
/// formats each shape before `RangeError: Maximum call stack size exceeded`, in a fresh process
/// and in a warmed-up one: `(shape, fresh, warm)`. Each shape is one site nested in itself
/// (`findings/conformance/work/api/depth/shapes.cjs`): a node type over a paragraph (`lists` is
/// a list in each item, `tables` a `{% table %}` in a cell of the one before, `inline_tags`
/// inline tags in a paragraph), or a value in a tag's attribute (`arrays`, `objects`, `calls`:
/// `f(f(...))`). The port stops exactly at the fresh limit of each, the low end of the band;
/// a mixed shape adds its sites' costs (see [`crate::depth`]).
pub const NODE_LIMITS: [(&str, usize, usize); 22] = [
    ("documents", 655, 1068),
    ("headings", 684, 1067),
    ("paragraphs", 818, 1342),
    ("inlines", 818, 1342),
    ("items", 1031, 1920),
    ("tbodies", 782, 1342),
    ("trs", 702, 1198),
    ("tds", 704, 1200),
    ("ths", 704, 1200),
    ("theads", 704, 1200),
    ("s", 613, 1006),
    ("em", 613, 1006),
    ("strong", 613, 1006),
    ("links", 704, 1200),
    ("tags", 818, 1342),
    ("inline_tags", 818, 1342),
    ("blockquotes", 672, 1580),
    ("lists", 469, 885),
    ("tables", 146, 250),
    ("arrays", 3206, 8939),
    ("objects", 2016, 6105),
    ("calls", 983, 1507),
];

/// What a level of a recursion site costs: node's stack, in [`crate::depth::STACK`]'s units
/// (solved from [`NODE_LIMITS`] by `work/api/depth/solve.py`, so that each shape, its fixed sites
/// included, stops at its fresh limit), and the Rust stack it takes in a debug build, which
/// bounds the recursion where the stack cannot grow (measured by `examples/api_depth.rs`, with
/// a margin).
#[derive(Clone, Copy)]
struct Cost(u64, u64);

const KB: u64 = 1024;
const COST_DOCUMENT: Cost = Cost(1605469090, 21 * KB / 2);
const COST_HEADING: Cost = Cost(1537450678, 23 * KB / 2);
const COST_PARAGRAPH: Cost = Cost(1285748306, 19 * KB / 2);
const COST_INLINE: Cost = Cost(1285748306, 17 * KB / 2);
const COST_ITEM: Cost = Cost(1020247202, 17 * KB / 2);
const COST_TBODY: Cost = Cost(1344900944, 17 * KB / 2);
const COST_TR: Cost = Cost(1493914506, 17 * KB / 2);
const COST_TD: Cost = Cost(1493804100, 17 * KB / 2);
const COST_TH: Cost = Cost(1493804100, 17 * KB / 2);
const COST_THEAD: Cost = Cost(1493804100, 17 * KB / 2);
const COST_INLINE_FORMATTED: Cost = Cost(1715378955, 19 * KB / 2);
const COST_LINK: Cost = Cost(1493804100, 23 * KB / 2);
const COST_TAG: Cost = Cost(1285748306, 14 * KB);
const COST_INLINE_TAG: Cost = Cost(1285748306, 14 * KB);
const COST_BLOCKQUOTE: Cost = Cost(1564884742, 23 * KB / 2);
const COST_LIST: Cost = Cost(1221254372, 14 * KB);
const COST_TABLE: Cost = Cost(1565147425, 12 * KB);
/// A node that does not recurse (text, fence, ...): its frames are in the shapes' fixed part.
const COST_LEAF: Cost = Cost(0, 8 * KB);
/// `formatScalar` of an array (each item `formatScalar`ed), and `formatValue` of one.
const COST_ARRAY: Cost = Cost(328502178, 5 * KB / 2);
/// `formatScalar` of an object (each member's value `formatScalar`ed).
const COST_OBJECT: Cost = Cost(522524541, 3 * KB);
/// `formatScalar` of a function call (`format(f)`: its parameters `formatScalar`ed).
const COST_CALL: Cost = Cost(1070259763, 8 * KB);

/// The cost of formatting `n` (`formatValue` and `formatNode` of it, and what its type adds:
/// `formatChildren`, `formatInline`, `trimStart`, the `format()` of a list's items, ...).
#[inline]
fn node_cost(n: NodeRef<'_>, kind: Kind) -> Cost {
    match kind {
        Kind::Document => COST_DOCUMENT,
        Kind::Heading => COST_HEADING,
        Kind::Paragraph => COST_PARAGRAPH,
        Kind::Inline => COST_INLINE,
        Kind::Item => COST_ITEM,
        Kind::Tbody => COST_TBODY,
        Kind::Tr => COST_TR,
        Kind::Td => COST_TD,
        Kind::Th => COST_TH,
        Kind::Thead => COST_THEAD,
        Kind::Strong | Kind::Em | Kind::S => COST_INLINE_FORMATTED,
        Kind::Link => COST_LINK,
        Kind::Tag if n.inline() => COST_INLINE_TAG,
        Kind::Tag => COST_TAG,
        Kind::Blockquote => COST_BLOCKQUOTE,
        Kind::List => COST_LIST,
        Kind::Table => COST_TABLE,
        _ => COST_LEAF,
    }
}

/// The case of `formatNode`'s `switch (n.type)` a node takes (`None`: no case, or `error` and
/// `node`, which format nothing).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Document,
    Heading,
    Paragraph,
    Inline,
    Image,
    Link,
    Text,
    Blockquote,
    Hr,
    Fence,
    Tag,
    List,
    Item,
    Strong,
    Em,
    Code,
    S,
    Hardbreak,
    Softbreak,
    Table,
    Thead,
    Tr,
    Td,
    Th,
    Tbody,
    Comment,
    None,
}

impl Kind {
    #[inline]
    fn of(n: NodeRef<'_>) -> Kind {
        match n {
            NodeRef::Node(node) => Kind::of_type(&node.node_type),
            NodeRef::Object(_) => match n.type_name() {
                Some(name) => Kind::of_type(&NodeType::from_name(name)),
                // `switch (n.type)` of a value that is no string matches no case
                None => Kind::None,
            },
        }
    }

    #[inline]
    fn of_type(t: &NodeType) -> Kind {
        match t {
            NodeType::Document => Kind::Document,
            NodeType::Heading => Kind::Heading,
            NodeType::Paragraph => Kind::Paragraph,
            NodeType::Inline => Kind::Inline,
            NodeType::Image => Kind::Image,
            NodeType::Link => Kind::Link,
            NodeType::Text => Kind::Text,
            NodeType::Blockquote => Kind::Blockquote,
            NodeType::Hr => Kind::Hr,
            NodeType::Fence => Kind::Fence,
            NodeType::Tag => Kind::Tag,
            NodeType::List => Kind::List,
            NodeType::Item => Kind::Item,
            NodeType::Strong => Kind::Strong,
            NodeType::Em => Kind::Em,
            NodeType::Code => Kind::Code,
            NodeType::Strikethrough => Kind::S,
            NodeType::Hardbreak => Kind::Hardbreak,
            NodeType::Softbreak => Kind::Softbreak,
            NodeType::Table => Kind::Table,
            NodeType::Thead => Kind::Thead,
            NodeType::Tr => Kind::Tr,
            NodeType::Td => Kind::Td,
            NodeType::Th => Kind::Th,
            NodeType::Tbody => Kind::Tbody,
            NodeType::Comment => Kind::Comment,
            NodeType::Error | NodeType::Node | NodeType::Other(_) => Kind::None,
        }
    }
}

/// Opens a level costing `cost` (see [`crate::next::stack`]), or upstream's `RangeError`.
fn level(cost: Cost) -> Result<stack::Level> {
    stack::enter(cost.0, cost.1).ok_or_else(|| throw(value::RANGE_ERROR))
}

// --- errors -------------------------------------------------------------------------------------

fn throw(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

fn invalid_string_length() -> MarkdocError {
    throw("RangeError: Invalid string length")
}

/// What V8 throws for `receiver.method(...)` when `receiver` (written `expression` in the
/// source) has no such method: reading a property of `null` or `undefined`, or calling what
/// is not a function.
fn not_a_function(receiver: &Scalar, expression: &str, method: &str) -> MarkdocError {
    match receiver {
        Scalar::Undefined | Scalar::Null => throw(format!(
            "TypeError: Cannot read properties of {} (reading '{method}')",
            if matches!(receiver, Scalar::Undefined) {
                "undefined"
            } else {
                "null"
            }
        )),
        _ => throw(format!(
            "TypeError: {expression}.{method} is not a function"
        )),
    }
}

/// [`not_a_function`] of a piece.
fn piece_not_a_function(receiver: &Piece, expression: &str, method: &str) -> MarkdocError {
    match receiver {
        Piece::Value(v) => not_a_function(v, expression, method),
        _ => throw(format!(
            "TypeError: {expression}.{method} is not a function"
        )),
    }
}

// --- JavaScript strings ------------------------------------------------------------------------

/// `s.length`: UTF-16 code units.
fn len16(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn is_js_space(c: char) -> bool {
    js::is_js_space(c as u32)
}

/// `s.trimStart()`.
fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_space)
}

/// `s.trim()`.
fn trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// `Err` (V8's `RangeError: Invalid string length`) when `s` is longer than V8's longest
/// string. A UTF-8 byte is at most one UTF-16 unit, so only a string of more bytes than that
/// needs counting.
fn checked(s: String) -> Result<String> {
    if s.len() > MAX_STRING_LENGTH && len16(&s) > MAX_STRING_LENGTH {
        return Err(invalid_string_length());
    }
    Ok(s)
}

/// `a + b + ...` of strings.
fn concat(parts: &[&str]) -> Result<String> {
    checked(parts.concat())
}

/// `list.join(separator)` of strings.
fn join_strings(list: &[String], separator: &str) -> Result<String> {
    checked(list.join(separator))
}

/// `s.repeat(count)` (`String.prototype.repeat`): `count` as `ToIntegerOrInfinity` makes it;
/// a negative or infinite count is `RangeError: Invalid count value: <count>`, a result
/// longer than V8's longest string `RangeError: Invalid string length`.
fn repeat(s: &str, count: &Scalar) -> Result<String> {
    let n = to_number(count)?;
    let n = if n.is_nan() { 0.0 } else { n.trunc() };
    if n < 0.0 || n == f64::INFINITY {
        return Err(throw(format!(
            "RangeError: Invalid count value: {}",
            describe_count(count)
        )));
    }
    if n == 0.0 || s.is_empty() {
        return Ok(String::new());
    }
    if n * len16(s) as f64 > MAX_STRING_LENGTH as f64 {
        return Err(invalid_string_length());
    }
    Ok(s.repeat(n as usize))
}

/// [`repeat`] of a number of times.
fn repeat_n(s: &str, count: f64) -> Result<String> {
    repeat(s, &Scalar::Number(count))
}

/// How V8 writes a count in `Invalid count value: ...` (without calling anything): a number
/// or a string as it is, an array `[object Array]`, any other object `[object Object]`.
fn describe_count(count: &Scalar) -> String {
    match count {
        Scalar::Number(n) => js::number_to_string(*n),
        Scalar::String(s) => s.clone(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Undefined => "undefined".into(),
        Scalar::Null => "null".into(),
        Scalar::Array(_) => "[object Array]".into(),
        Scalar::Js(js) if matches!(js.value(), value::Value::Array(_)) => "[object Array]".into(),
        Scalar::Object(_)
        | Scalar::Variable(_)
        | Scalar::Function(_)
        | Scalar::Tag(_)
        | Scalar::Js(_) => "[object Object]".into(),
    }
}

// --- JavaScript values --------------------------------------------------------------------------

/// `String(value)` (also what `+` and a template literal make of it).
fn to_js_string(value: &Scalar) -> Result<String> {
    scalar_to_js_string(value).map_err(throw)
}

/// `Number(value)`.
fn to_number(value: &Scalar) -> Result<f64> {
    Ok(match value {
        Scalar::Undefined => f64::NAN,
        Scalar::Null => 0.0,
        Scalar::Boolean(b) => f64::from(u8::from(*b)),
        Scalar::Number(n) => *n,
        Scalar::String(s) => js::string_to_number(&js::to_utf16(s)),
        other => js::string_to_number(&js::to_utf16(&to_js_string(other)?)),
    })
}

/// JavaScript's truthiness.
fn truthy(value: &Scalar) -> bool {
    match value {
        Scalar::Undefined | Scalar::Null => false,
        Scalar::Boolean(b) => *b,
        Scalar::Number(n) => !(*n == 0.0 || n.is_nan()),
        Scalar::String(s) => !s.is_empty(),
        Scalar::Array(_)
        | Scalar::Object(_)
        | Scalar::Variable(_)
        | Scalar::Function(_)
        | Scalar::Tag(_)
        | Scalar::Js(_) => true,
    }
}

/// `value == null`.
fn nullish(value: &Scalar) -> bool {
    matches!(value, Scalar::Undefined | Scalar::Null)
}

/// `value.$$mdtype` of an object (`undefined` when it has none).
fn mdtype(value: &Scalar) -> Scalar {
    match value {
        Scalar::Variable(_) => Scalar::String("Variable".into()),
        Scalar::Function(_) => Scalar::String("Function".into()),
        Scalar::Tag(_) => Scalar::String("Tag".into()),
        Scalar::Object(o) => o.get("$$mdtype").cloned().unwrap_or(Scalar::Undefined),
        Scalar::Js(js) => js_get(js, "$$mdtype"),
        _ => Scalar::Undefined,
    }
}

/// `value[key]` of a JavaScript object the AST holds ([`Scalar::Js`]), read through its
/// prototype chain, as the plain value JSON keeps of it (`undefined` where it throws).
fn js_get(js: &crate::types::JsObjectValue, key: &str) -> Scalar {
    match value::get(js.value(), key) {
        Ok(value::Value::Undefined) | Err(_) => Scalar::Undefined,
        Ok(v) => v.to_scalar().unwrap_or(Scalar::Undefined),
    }
}

/// A JavaScript object the AST holds ([`Scalar::Js`]) as the plain value its JSON is (an
/// object whose `$$mdtype` names `Variable` or `Function` as that class's value), which is
/// how the formatter writes it.
fn js_plain(js: &crate::types::JsObjectValue) -> Scalar {
    js.to_json()
        .and_then(|json| Scalar::from_json(&json).ok())
        .unwrap_or(Scalar::Undefined)
}

/// `Ast.isAst(value)`: `!!value?.$$mdtype`.
fn is_ast(value: &Scalar) -> bool {
    truthy(&mdtype(value))
}

/// `value.length` of a value that is not `null` or `undefined`: a string's code units, an
/// array's items, an object's own `length` (any value), and `undefined` for the rest.
fn length_of(value: &Scalar) -> Result<Scalar> {
    Ok(match value {
        Scalar::Undefined | Scalar::Null => {
            return Err(not_a_property(value, "length"));
        }
        Scalar::String(s) => Scalar::Number(len16(s) as f64),
        Scalar::Array(items) => Scalar::Number(items.len() as f64),
        Scalar::Object(o) => o.get("length").cloned().unwrap_or(Scalar::Undefined),
        Scalar::Js(js) => js_get(js, "length"),
        Scalar::Boolean(_)
        | Scalar::Number(_)
        | Scalar::Variable(_)
        | Scalar::Function(_)
        | Scalar::Tag(_) => Scalar::Undefined,
    })
}

/// `TypeError: Cannot read properties of null (reading 'key')`.
fn not_a_property(value: &Scalar, key: &str) -> MarkdocError {
    throw(format!(
        "TypeError: Cannot read properties of {} (reading '{key}')",
        if matches!(value, Scalar::Null) {
            "null"
        } else {
            "undefined"
        }
    ))
}

/// `a + b` (ToPrimitive, then a string concatenation if either is a string, else a sum).
fn js_add(a: &Scalar, b: &Scalar) -> Result<Scalar> {
    let primitive = |v: &Scalar| -> Result<Scalar> {
        Ok(match v {
            Scalar::Array(_)
            | Scalar::Object(_)
            | Scalar::Variable(_)
            | Scalar::Function(_)
            | Scalar::Tag(_) => Scalar::String(to_js_string(v)?),
            other => other.clone(),
        })
    };
    let (a, b) = (primitive(a)?, primitive(b)?);
    if matches!(a, Scalar::String(_)) || matches!(b, Scalar::String(_)) {
        return Ok(Scalar::String(concat(&[
            &to_js_string(&a)?,
            &to_js_string(&b)?,
        ])?));
    }
    Ok(Scalar::Number(to_number(&a)? + to_number(&b)?))
}

/// `value.toString()` called as a method: the value's own `toString` when it has one (never
/// a function here: `TypeError: <expression>.toString is not a function`), else the
/// built-in one (`Object.prototype.toString` is `[object Object]` for an object).
fn call_to_string(value: &Scalar, expression: &str) -> Result<String> {
    match value {
        Scalar::Undefined | Scalar::Null => Err(not_a_property(value, "toString")),
        Scalar::Object(o) if o.contains_key("toString") => {
            Err(not_a_function(value, expression, "toString"))
        }
        Scalar::Object(_) | Scalar::Variable(_) | Scalar::Function(_) | Scalar::Tag(_) => {
            Ok("[object Object]".into())
        }
        other => to_js_string(other),
    }
}

/// `JSON.stringify(v)` of a primitive.
fn json_stringify_primitive(value: &Scalar) -> String {
    match value {
        Scalar::String(s) => {
            let mut out = String::new();
            quote_json_string(s, &mut out);
            out
        }
        Scalar::Number(n) if n.is_finite() => js::number_to_string(*n),
        Scalar::Number(_) | Scalar::Null => "null".into(),
        Scalar::Boolean(b) => b.to_string(),
        _ => unreachable!("JSON.stringify of a primitive"),
    }
}

/// `value[key]` of any value, as JavaScript reads a property: a string's `length` and code
/// units, an array's `length` and items, an object's own property (an object whose
/// `$$mdtype` is `"Node"` has `Ast.fromJSON`'s defaults: [`node_field`]), the fields of a
/// `Variable`, a `Function` and a tag; `undefined` for anything else; `TypeError` for `null`
/// and `undefined`.
fn get<'v>(value: &'v Scalar, key: &str) -> Result<Cow<'v, Scalar>> {
    const UNDEFINED: &Scalar = &Scalar::Undefined;
    Ok(match value {
        Scalar::Undefined | Scalar::Null => return Err(not_a_property(value, key)),
        Scalar::String(s) => Cow::Owned(if key == "length" {
            Scalar::Number(len16(s) as f64)
        } else if let Some(i) = crate::types::array_index(key) {
            let units = js::to_utf16(s);
            units.get(i as usize).map_or(Scalar::Undefined, |&u| {
                Scalar::String(String::from_utf16_lossy(&[u]))
            })
        } else {
            Scalar::Undefined
        }),
        Scalar::Array(items) => {
            if key == "length" {
                Cow::Owned(Scalar::Number(items.len() as f64))
            } else if let Some(i) = crate::types::array_index(key) {
                Cow::Borrowed(items.get(i as usize).unwrap_or(UNDEFINED))
            } else {
                Cow::Borrowed(UNDEFINED)
            }
        }
        Scalar::Object(o) => match o.get(key) {
            Some(v) => Cow::Borrowed(v),
            None if is_node_object(o) => node_field(o, key),
            None => Cow::Borrowed(UNDEFINED),
        },
        Scalar::Boolean(_) | Scalar::Number(_) => Cow::Borrowed(UNDEFINED),
        Scalar::Js(js) => Cow::Owned(js_get(js, key)),
        // `{$$mdtype, path}`, `{$$mdtype, name, parameters}`, `{$$mdtype, name, attributes,
        // children}`
        Scalar::Variable(_) | Scalar::Function(_) | Scalar::Tag(_) => {
            Cow::Owned(match value.to_json() {
                serde_json::Value::Object(o) => match (o.get(key), key) {
                    (Some(_), "$$mdtype") => match value {
                        Scalar::Variable(_) => Scalar::String("Variable".into()),
                        Scalar::Function(_) => Scalar::String("Function".into()),
                        _ => Scalar::String("Tag".into()),
                    },
                    (Some(v), _) => Scalar::from_json(v).unwrap_or(Scalar::Undefined),
                    (None, _) => Scalar::Undefined,
                },
                _ => Scalar::Undefined,
            })
        }
    })
}

/// Whether an object's `$$mdtype` is `"Node"`: what `Ast.fromJSON` revives as a `Node`.
fn is_node_object(o: &IndexMap<String, Scalar>) -> bool {
    matches!(o.get("$$mdtype"), Some(Scalar::String(t)) if t == "Node")
}

/// A field of an object `Ast.fromJSON` revives as a `Node` (`Object.assign(new Node(),
/// value)`): its own, or the constructor's default (`attributes` `{}`, `children`,
/// `annotations`, `errors` and `lines` `[]`, `inline` `false`, `type` `'node'`, `slots`
/// `{}`), else `undefined`.
fn node_field<'o>(o: &'o IndexMap<String, Scalar>, key: &str) -> Cow<'o, Scalar> {
    match o.get(key) {
        Some(v) => Cow::Borrowed(v),
        None => Cow::Owned(match key {
            "attributes" | "slots" => Scalar::Object(IndexMap::new()),
            "children" | "annotations" | "errors" | "lines" => Scalar::Array(Vec::new()),
            "inline" => Scalar::Boolean(false),
            "type" => Scalar::String("node".into()),
            _ => Scalar::Undefined,
        }),
    }
}

/// The own properties of a `Node` that `Ast.fromJSON` made of an object, in order
/// (`Object.entries`): index keys, then the constructor's fields (`$$mdtype`, `errors`,
/// `lines`, `inline`, `attributes`, `children`, `type`, `tag`, `annotations`, `slots`, each the
/// object's or its default; `tag` is `undefined` but present), then the object's other keys.
fn revived_entries(o: &IndexMap<String, Scalar>) -> Vec<(String, Scalar)> {
    const FIELDS: [&str; 10] = [
        "$$mdtype",
        "errors",
        "lines",
        "inline",
        "attributes",
        "children",
        "type",
        "tag",
        "annotations",
        "slots",
    ];
    let mut out: Vec<(String, Scalar)> = o
        .iter()
        .filter(|(k, _)| crate::types::is_array_index(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for field in FIELDS {
        out.push((field.to_string(), node_field(o, field).into_owned()));
    }
    out.extend(
        o.iter()
            .filter(|(k, _)| !crate::types::is_array_index(k) && !FIELDS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    out
}

/// `x.method(...)` of a value that has no such method, where `x` is written `expression`.
fn no_method(receiver: &Scalar, expression: &str, method: &str) -> MarkdocError {
    not_a_function(receiver, expression, method)
}

/// A node `formatNode` formats: one of the AST's, or an object whose `$$mdtype` is `"Node"`
/// inside a value (only hand-made JSON makes one), read as `Ast.fromJSON` revives it, with
/// JavaScript's semantics for fields of any type.
#[derive(Clone, Copy)]
enum NodeRef<'a> {
    Node(&'a Node),
    Object(&'a IndexMap<String, Scalar>),
}

/// What `formatChildren` hands `formatValue`: a node, or any value of a `Node` object's
/// `children`.
enum Child<'a> {
    Node(&'a Node),
    Value(Cow<'a, Scalar>),
}

/// An annotation `formatAnnotationValue` reads: one of a node's, or any value of a `Node`
/// object's `annotations`.
enum Annotation<'a> {
    Node(&'a AttributeValue),
    Value(Cow<'a, Scalar>),
}

impl<'a> NodeRef<'a> {
    /// `n.type` when it is a string (`switch (n.type)` matches no case otherwise).
    #[inline]
    fn type_name(self) -> Option<&'a str> {
        match self {
            NodeRef::Node(n) => Some(n.node_type.as_str()),
            NodeRef::Object(o) => match o.get("type") {
                None => Some("node"),
                Some(Scalar::String(t)) => Some(t.as_str()),
                Some(_) => None,
            },
        }
    }

    /// `n.tag` when it is a string (`o.parent.tag === 'table'` compares no other).
    #[inline]
    fn tag_str(self) -> Option<&'a str> {
        match self {
            NodeRef::Node(n) => n.tag.as_deref(),
            NodeRef::Object(o) => match o.get("tag") {
                Some(Scalar::String(t)) => Some(t.as_str()),
                _ => None,
            },
        }
    }

    /// `n.tag` as `open + n.tag` writes it.
    fn tag_string(self) -> Result<String> {
        match self {
            NodeRef::Node(n) => Ok(n.tag.clone().unwrap_or_else(|| "undefined".into())),
            NodeRef::Object(o) => to_js_string(&node_field(o, "tag")),
        }
    }

    /// `n.inline`'s truthiness.
    #[inline]
    fn inline(self) -> bool {
        match self {
            NodeRef::Node(n) => n.inline,
            NodeRef::Object(o) => truthy(&node_field(o, "inline")),
        }
    }

    /// `n.attributes[key]`.
    #[inline]
    fn attr(self, key: &str) -> Result<Cow<'a, Scalar>> {
        const UNDEFINED: &Scalar = &Scalar::Undefined;
        match self {
            NodeRef::Node(n) => Ok(Cow::Borrowed(n.attributes.get(key).unwrap_or(UNDEFINED))),
            NodeRef::Object(o) => match node_field(o, "attributes") {
                Cow::Borrowed(attributes) => get(attributes, key),
                Cow::Owned(attributes) => Ok(Cow::Owned(get(&attributes, key)?.into_owned())),
            },
        }
    }

    /// `const { content } = n.attributes;`.
    #[inline]
    fn destructured_content(self) -> Result<Cow<'a, Scalar>> {
        if let NodeRef::Object(o) = self {
            let attributes = node_field(o, "attributes");
            if nullish(&attributes) {
                return Err(throw(format!(
                    "TypeError: Cannot destructure property 'content' of 'n.attributes' as it \
                     is {}.",
                    if matches!(*attributes, Scalar::Null) {
                        "null"
                    } else {
                        "undefined"
                    }
                )));
            }
        }
        self.attr("content")
    }

    /// `Object.entries(n.attributes)`.
    fn attribute_entries(self) -> Result<Vec<(Cow<'a, str>, Cow<'a, Scalar>)>> {
        let borrowed = |a: &'a IndexMap<String, Scalar>| {
            a.iter()
                .map(|(k, v)| (Cow::Borrowed(k.as_str()), Cow::Borrowed(v)))
                .collect()
        };
        match self {
            NodeRef::Node(n) => Ok(borrowed(&n.attributes)),
            NodeRef::Object(o) => match node_field(o, "attributes") {
                Cow::Borrowed(Scalar::Object(a)) if !is_node_object(a) => Ok(borrowed(a)),
                Cow::Borrowed(Scalar::Array(items)) => Ok(items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (Cow::Owned(i.to_string()), Cow::Borrowed(v)))
                    .collect()),
                attributes => match &*attributes {
                    Scalar::Undefined | Scalar::Null => Err(throw(
                        "TypeError: Cannot convert undefined or null to object",
                    )),
                    Scalar::Object(a) if is_node_object(a) => Ok(revived_entries(a)
                        .into_iter()
                        .map(|(k, v)| (Cow::Owned(k), Cow::Owned(v)))
                        .collect()),
                    Scalar::String(s) => Ok(js::to_utf16(s)
                        .iter()
                        .enumerate()
                        .map(|(i, &u)| {
                            (
                                Cow::Owned(i.to_string()),
                                Cow::Owned(Scalar::String(String::from_utf16_lossy(&[u]))),
                            )
                        })
                        .collect()),
                    // an owned default (`{}`)
                    Scalar::Object(a) => Ok(a
                        .iter()
                        .map(|(k, v)| (Cow::Owned(k.clone()), Cow::Owned(v.clone())))
                        .collect()),
                    Scalar::Array(items) => Ok(items
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (Cow::Owned(i.to_string()), Cow::Owned(v.clone())))
                        .collect()),
                    Scalar::Boolean(_) | Scalar::Number(_) => Ok(Vec::new()),
                    other => match other.to_json() {
                        serde_json::Value::Object(a) => {
                            let mut entries = Vec::with_capacity(a.len());
                            for k in a.keys() {
                                let v = get(other, k)?.into_owned();
                                entries.push((Cow::Owned(k.clone()), Cow::Owned(v)));
                            }
                            Ok(entries)
                        }
                        _ => Ok(Vec::new()),
                    },
                },
            },
        }
    }

    /// `n.children` of a `Node` object.
    fn children_value(o: &'a IndexMap<String, Scalar>) -> Cow<'a, Scalar> {
        node_field(o, "children")
    }

    /// `for (const child of a.children)`: the children, or `TypeError: a.children is not
    /// iterable` (a string's are its characters).
    fn children(self) -> Result<Vec<Child<'a>>> {
        match self {
            NodeRef::Node(n) => Ok(n.children.iter().map(Child::Node).collect()),
            NodeRef::Object(o) => match Self::children_value(o) {
                Cow::Borrowed(Scalar::Array(items)) => Ok(items
                    .iter()
                    .map(|v| Child::Value(Cow::Borrowed(v)))
                    .collect()),
                // (`Scalar`'s drop is its own: take the items out rather than move them.)
                Cow::Owned(mut owned) if matches!(owned, Scalar::Array(_)) => {
                    let Scalar::Array(items) = &mut owned else {
                        unreachable!()
                    };
                    Ok(std::mem::take(items)
                        .into_iter()
                        .map(|v| Child::Value(Cow::Owned(v)))
                        .collect())
                }
                children => match &*children {
                    Scalar::String(s) => Ok(s
                        .chars()
                        .map(|c| Child::Value(Cow::Owned(Scalar::String(c.to_string()))))
                        .collect()),
                    _ => Err(throw("TypeError: a.children is not iterable")),
                },
            },
        }
    }

    /// `n.children.length`.
    #[inline]
    fn children_length(self) -> Result<Scalar> {
        match self {
            NodeRef::Node(n) => Ok(Scalar::Number(n.children.len() as f64)),
            NodeRef::Object(o) => Ok(get(&Self::children_value(o), "length")?.into_owned()),
        }
    }

    /// `n.children[i]`.
    #[inline]
    fn child_at(self, i: usize) -> Result<Child<'a>> {
        match self {
            NodeRef::Node(n) => Ok(Child::Node(&n.children[i])),
            NodeRef::Object(o) => match Self::children_value(o) {
                Cow::Borrowed(Scalar::Array(items)) => Ok(Child::Value(
                    items
                        .get(i)
                        .map_or(Cow::Owned(Scalar::Undefined), Cow::Borrowed),
                )),
                children => Ok(Child::Value(Cow::Owned(
                    get(&children, &i.to_string())?.into_owned(),
                ))),
            },
        }
    }

    /// `n.children.some((n2) => n2.children.some((c) => c.type === 'paragraph'))`.
    fn has_paragraph_grandchild(self) -> Result<bool> {
        match self {
            NodeRef::Node(n) => Ok(n.children.iter().any(|n| {
                n.children
                    .iter()
                    .any(|c| c.node_type == NodeType::Paragraph)
            })),
            NodeRef::Object(o) => {
                let children = Self::children_value(o);
                let Scalar::Array(items) = &*children else {
                    return Err(no_method(&children, "n.children", "some"));
                };
                for n2 in items {
                    let grandchildren = get(n2, "children")?;
                    let Scalar::Array(cs) = &*grandchildren else {
                        return Err(no_method(&grandchildren, "n2.children", "some"));
                    };
                    for c in cs {
                        if matches!(&*get(c, "type")?, Scalar::String(t) if t == "paragraph") {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
        }
    }

    /// `n.children.map(...)`'s items (an array's), or what V8 throws.
    fn children_to_map(self) -> Result<Vec<Child<'a>>> {
        match self {
            NodeRef::Node(n) => Ok(n.children.iter().map(Child::Node).collect()),
            NodeRef::Object(o) => match Self::children_value(o) {
                Cow::Borrowed(Scalar::Array(items)) => Ok(items
                    .iter()
                    .map(|v| Child::Value(Cow::Borrowed(v)))
                    .collect()),
                // (`Scalar`'s drop is its own: take the items out rather than move them.)
                Cow::Owned(mut owned) if matches!(owned, Scalar::Array(_)) => {
                    let Scalar::Array(items) = &mut owned else {
                        unreachable!()
                    };
                    Ok(std::mem::take(items)
                        .into_iter()
                        .map(|v| Child::Value(Cow::Owned(v)))
                        .collect())
                }
                children => Err(no_method(&children, "n.children", "map")),
            },
        }
    }

    /// `n.annotations.length`.
    #[inline]
    fn annotations_length(self) -> Result<Scalar> {
        match self {
            NodeRef::Node(n) => Ok(Scalar::Number(n.annotations.len() as f64)),
            NodeRef::Object(o) => Ok(get(&node_field(o, "annotations"), "length")?.into_owned()),
        }
    }

    /// `n.annotations.map(...)`'s items (an array's), or what V8 throws.
    fn annotations(self) -> Result<Vec<Annotation<'a>>> {
        match self {
            NodeRef::Node(n) => Ok(n.annotations.iter().map(Annotation::Node).collect()),
            NodeRef::Object(o) => match node_field(o, "annotations") {
                Cow::Borrowed(Scalar::Array(items)) => Ok(items
                    .iter()
                    .map(|v| Annotation::Value(Cow::Borrowed(v)))
                    .collect()),
                // (`Scalar`'s drop is its own: take the items out rather than move them.)
                Cow::Owned(mut owned) if matches!(owned, Scalar::Array(_)) => {
                    let Scalar::Array(items) = &mut owned else {
                        unreachable!()
                    };
                    Ok(std::mem::take(items)
                        .into_iter()
                        .map(|v| Annotation::Value(Cow::Owned(v)))
                        .collect())
                }
                annotations => Err(no_method(&annotations, "n.annotations", "map")),
            },
        }
    }
}

// --- what generators yield --------------------------------------------------------------------

/// A value a formatter generator yields.
#[derive(Clone, Debug)]
enum Piece {
    /// A string.
    Str(String),
    /// An array (a table row: its cells' values).
    Array(Vec<Piece>),
    /// Any other value, as an attribute holds it (never a string).
    Value(Scalar),
}

impl Piece {
    fn str(s: impl Into<String>) -> Piece {
        Piece::Str(s.into())
    }

    /// An attribute's value as a piece.
    fn of(value: &Scalar) -> Piece {
        match value {
            Scalar::String(s) => Piece::Str(s.clone()),
            other => Piece::Value(other.clone()),
        }
    }

    /// `String(piece)`.
    fn to_js_string(&self) -> Result<String> {
        match self {
            Piece::Str(s) => Ok(s.clone()),
            Piece::Array(items) => join(items, ","),
            Piece::Value(v) => to_js_string(v),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Piece::Str(s) => !s.is_empty(),
            Piece::Array(_) => true,
            Piece::Value(v) => truthy(v),
        }
    }

    /// `piece.length`.
    fn length(&self) -> Result<Scalar> {
        match self {
            Piece::Str(s) => Ok(Scalar::Number(len16(s) as f64)),
            Piece::Array(items) => Ok(Scalar::Number(items.len() as f64)),
            Piece::Value(v) => length_of(v),
        }
    }

    /// `piece[i]`: an item, a code unit (only its length is read: a piece of length 1), an
    /// object's member, or `undefined`.
    fn index(&self, i: usize) -> Piece {
        match self {
            Piece::Str(s) => {
                let units = js::to_utf16(s);
                match units.get(i) {
                    Some(&u) => Piece::Str(String::from_utf16_lossy(&[u])),
                    None => Piece::Value(Scalar::Undefined),
                }
            }
            Piece::Array(items) => items
                .get(i)
                .cloned()
                .unwrap_or(Piece::Value(Scalar::Undefined)),
            Piece::Value(Scalar::Array(items)) => items
                .get(i)
                .map(Piece::of)
                .unwrap_or(Piece::Value(Scalar::Undefined)),
            Piece::Value(Scalar::Object(o)) => o
                .get(&i.to_string())
                .map(Piece::of)
                .unwrap_or(Piece::Value(Scalar::Undefined)),
            Piece::Value(_) => Piece::Value(Scalar::Undefined),
        }
    }

    /// The items of an array piece (`Array.isArray`), as `map` and `for...of` see them.
    fn items(&self) -> Option<Vec<Piece>> {
        match self {
            Piece::Array(items) => Some(items.clone()),
            Piece::Value(Scalar::Array(items)) => Some(items.iter().map(Piece::of).collect()),
            _ => None,
        }
    }

    /// `for (const d of piece)`: an array's items, a string's characters; anything else
    /// throws `TypeError: <expression> is not iterable`.
    fn iterate(&self, expression: &str) -> Result<Vec<Piece>> {
        if let Some(items) = self.items() {
            return Ok(items);
        }
        match self {
            Piece::Str(s) => Ok(s.chars().map(|c| Piece::Str(c.to_string())).collect()),
            _ => Err(throw(format!("TypeError: {expression} is not iterable"))),
        }
    }

    /// `piece.map(f)`'s receiver: an array's items, or the `TypeError` for a value that has
    /// no `map`.
    fn map_receiver(&self, expression: &str) -> Result<Vec<Piece>> {
        self.items()
            .ok_or_else(|| piece_not_a_function(self, expression, "map"))
    }
}

/// `items.join(separator)`: `null` and `undefined` as `''`, everything else `String()`.
fn join(items: &[Piece], separator: &str) -> Result<String> {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(separator);
        }
        match item {
            Piece::Value(Scalar::Undefined | Scalar::Null) => {}
            other => out.push_str(&other.to_js_string()?),
        }
    }
    checked(out)
}

/// A generator's consumer.
trait Sink {
    /// Takes the next value the generator yields.
    fn put(&mut self, piece: Piece) -> Result<()>;
}

/// `[...g]`: every value, collected.
#[derive(Default)]
struct Collect(Vec<Piece>);

impl Sink for Collect {
    fn put(&mut self, piece: Piece) -> Result<()> {
        self.0.push(piece);
        Ok(())
    }
}

/// `format`'s `for (const s of formatValue(v, options)) doc += s;`.
#[derive(Default)]
struct Doc {
    doc: String,
    /// `doc.length`.
    units: usize,
}

impl Sink for Doc {
    fn put(&mut self, piece: Piece) -> Result<()> {
        let s = match piece {
            Piece::Str(s) => s,
            other => other.to_js_string()?,
        };
        self.units += len16(&s);
        if self.units > MAX_STRING_LENGTH {
            return Err(invalid_string_length());
        }
        self.doc.push_str(&s);
        Ok(())
    }
}

/// `trimStart(g)`: each value's `trimStart()` until one is not empty, which is passed on
/// with every value after it.
struct TrimStart<'s> {
    out: &'s mut dyn Sink,
    started: bool,
}

impl<'s> TrimStart<'s> {
    fn new(out: &'s mut dyn Sink) -> Self {
        TrimStart {
            out,
            started: false,
        }
    }
}

impl Sink for TrimStart<'_> {
    fn put(&mut self, piece: Piece) -> Result<()> {
        if self.started {
            return self.out.put(piece);
        }
        // `n = value.trimStart();`
        let Piece::Str(value) = &piece else {
            return Err(piece_not_a_function(&piece, "value", "trimStart"));
        };
        let n = trim_start(value);
        if n.is_empty() {
            return Ok(());
        }
        self.started = true;
        self.out.put(Piece::str(n))
    }
}

/// `format(v, options)`: the values of `formatValue(v, options)` appended to `doc`, then
/// `doc.trimStart()`.
fn run(format_value: impl FnOnce(&mut dyn Sink) -> Result<()>) -> Result<String> {
    let mut doc = Doc::default();
    format_value(&mut doc)?;
    let trimmed = trim_start(&doc.doc);
    if trimmed.len() == doc.doc.len() {
        return Ok(doc.doc);
    }
    Ok(trimmed.to_string())
}

// --- options --------------------------------------------------------------------------------------

/// The options object as the generators pass it on (`{...o, parent: n}`, `increment(o, n)`):
/// `indent` is whatever `(o.indent || 0) + n` made, a number unless a list marker's `length`
/// was a string.
#[derive(Clone)]
struct Opts<'a> {
    allow_indentation: bool,
    max_tag_opening_width: Option<f64>,
    increment: bool,
    parent: Option<NodeRef<'a>>,
    indent: Scalar,
}

impl<'a> From<&FormatOptions<'a>> for Opts<'a> {
    fn from(o: &FormatOptions<'a>) -> Self {
        Opts {
            allow_indentation: o.allow_indentation,
            max_tag_opening_width: o.max_tag_opening_width,
            increment: o.ordered_list_mode == Some(OrderedListMode::Increment),
            parent: o.parent.map(NodeRef::Node),
            indent: o.indent.map_or(Scalar::Undefined, Scalar::Number),
        }
    }
}

impl<'a> Opts<'a> {
    /// `{...o, parent: n}`.
    fn with_parent(&self, n: NodeRef<'a>) -> Self {
        Opts {
            parent: Some(n),
            ..self.clone()
        }
    }

    /// `o.indent || 0`.
    fn indent_or_zero(&self) -> Scalar {
        if truthy(&self.indent) {
            self.indent.clone()
        } else {
            Scalar::Number(0.0)
        }
    }

    /// `increment(o, n)`: `{...o, indent: (o.indent || 0) + n}`.
    fn increment(&self, n: &Scalar) -> Result<Self> {
        Ok(Opts {
            indent: js_add(&self.indent_or_zero(), n)?,
            ..self.clone()
        })
    }
}

// --- formatter.ts -------------------------------------------------------------------------------

/// `formatChildren(a, options)`.
fn format_children<'a>(a: NodeRef<'a>, options: &Opts<'a>, out: &mut dyn Sink) -> Result<()> {
    if let NodeRef::Node(n) = a {
        for child in &n.children {
            format_node(NodeRef::Node(child), options, out)?;
        }
        return Ok(());
    }
    for child in a.children()? {
        format_child(&child, options, out)?;
    }
    Ok(())
}

/// `formatValue(child, options)` of a child.
#[inline]
fn format_child<'a>(child: &Child<'a>, options: &Opts<'a>, out: &mut dyn Sink) -> Result<()> {
    match child {
        Child::Node(n) => format_node(NodeRef::Node(n), options, out),
        Child::Value(v) => format_scalar_value(v, options, out),
    }
}

/// `formatInline(g)`: `[...g].join('').trim()`.
fn format_inline(collected: Collect, out: &mut dyn Sink) -> Result<()> {
    let joined = join(&collected.0, "")?;
    out.put(Piece::str(trim(&joined)))
}

/// `formatTableRow(items)`: `| ${items.join(' | ')} |`.
fn format_table_row(items: &[Piece], out: &mut dyn Sink) -> Result<()> {
    out.put(Piece::Str(concat(&["| ", &join(items, " | ")?, " |"])?))
}

/// `formatScalar(v)`: an attribute value as the tag syntax writes it; `None` for
/// `undefined`.
fn format_scalar(v: &Scalar) -> Result<Option<String>> {
    if matches!(v, Scalar::Undefined) {
        return Ok(None);
    }
    if is_ast(v) {
        let _level = level(if matches!(v, Scalar::Function(_)) {
            COST_CALL
        } else {
            COST_LEAF
        })?;
        return stack::grow(|| format_value(v, &FormatOptions::default())).map(Some);
    }
    Ok(Some(match v {
        Scalar::Null => "null".into(),
        Scalar::Array(items) => {
            let _level = level(COST_ARRAY)?;
            stack::grow(|| -> Result<String> {
                let mut parts = Vec::with_capacity(items.len());
                for item in items {
                    parts.push(format_scalar(item)?.unwrap_or_default());
                }
                concat(&["[", &join_strings(&parts, SEP)?, "]"])
            })?
        }
        Scalar::Object(object) => {
            let _level = level(COST_OBJECT)?;
            stack::grow(|| -> Result<String> {
                let mut parts = Vec::with_capacity(object.len());
                for (key, value) in object {
                    let key = if is_identifier(key) {
                        key.clone()
                    } else {
                        format!("\"{key}\"")
                    };
                    let value = format_scalar(value)?.unwrap_or_else(|| "undefined".into());
                    parts.push(concat(&[&key, ": ", &value])?);
                }
                concat(&["{", &join_strings(&parts, SEP)?, "}"])
            })?
        }
        // A JavaScript object the plain variants cannot hold (a prototype, holes, other own
        // properties): `Array.isArray(v)`, then `Object.entries(v)`, as for the plain ones.
        Scalar::Js(js) => return format_js_scalar(js.value()),
        other => json_stringify_primitive(other),
    }))
}

/// A JavaScript value whose `$$mdtype` (read through its prototype chain) is `Variable` or
/// `Function`, as the formatter reads one (`v.path.map(...)`, `v.name` and
/// `Object.values(v.parameters)`, each through the chain): the plain AST value, or what V8
/// throws; `None` for any other value.
fn js_ast_value(v: &value::Value) -> Result<Option<Scalar>> {
    use value::Value as V;
    let mdtype = value::get(v, "$$mdtype").map_err(throw)?;
    let as_scalar = |item: &V| -> Scalar {
        match item {
            V::Array(_) | V::Object(_) | V::Tag(_) | V::Builtin(_) => {
                Scalar::Js(crate::types::JsObjectValue::new(item.clone()))
            }
            other => other.to_scalar().unwrap_or(Scalar::Undefined),
        }
    };
    match mdtype.as_str() {
        Some("Variable") => {
            let path = value::get(v, "path").map_err(throw)?;
            let V::Array(items) = &path else {
                return Err(throw(match path {
                    V::Undefined => {
                        "TypeError: Cannot read properties of undefined (reading 'map')"
                    }
                    V::Null => "TypeError: Cannot read properties of null (reading 'map')",
                    _ => "TypeError: v.path.map is not a function",
                }));
            };
            let mut steps = Vec::with_capacity(items.present_len());
            for (_, step) in items.present() {
                steps.push(match step {
                    V::Number(n) => PathSegment::Number(*n),
                    other => PathSegment::String(
                        other
                            .to_js_string()
                            .map_err(throw)?
                            .to_string_lossy()
                            .into_owned(),
                    ),
                });
            }
            Ok(Some(Scalar::Variable(Variable::new(steps))))
        }
        Some("Function") => {
            let name = value::get(v, "name")
                .map_err(throw)?
                .to_js_string()
                .map_err(throw)?
                .to_string_lossy()
                .into_owned();
            let parameters = value::get(v, "parameters").map_err(throw)?;
            if parameters.is_nullish() {
                return Err(throw(
                    "TypeError: Cannot convert undefined or null to object",
                ));
            }
            let mut out = IndexMap::new();
            for (k, item) in value::own_entries(&parameters) {
                out.insert(k, as_scalar(&item));
            }
            Ok(Some(Scalar::Function(Function::new(name, out))))
        }
        _ => Ok(None),
    }
}

/// [`format_scalar`] of a JavaScript value ([`Scalar::Js`] and what it holds): an array's
/// elements (`v.map(formatScalar).join(SEP)`: a hole, which `map` skips, is empty), an
/// object's own enumerable properties (`Object.entries(v)`), a function (a built-in a
/// document reached) `undefined` (`JSON.stringify` of a function), anything else as its
/// primitive or plain value is.
fn format_js_scalar(v: &value::Value) -> Result<Option<String>> {
    use value::Value as V;
    match v {
        V::Undefined => Ok(None),
        V::Null => Ok(Some("null".into())),
        V::Boolean(b) => Ok(Some(b.to_string())),
        V::Number(n) if n.is_finite() => Ok(Some(js::number_to_string(*n))),
        V::Number(_) => Ok(Some("null".into())),
        V::String(s) => Ok(Some(json_stringify_primitive(&Scalar::String(
            s.to_string_lossy().into_owned(),
        )))),
        V::Builtin(b) if b.is_callable() => Ok(None),
        _ if value::is_ast(v) => match js_ast_value(v)? {
            // A variable or a call read through its prototype chain.
            Some(ast) => format_scalar(&ast),
            // Any other AST value: as its plain JSON shows it.
            None => format_scalar(&v.to_scalar().map_err(throw)?),
        },
        V::Array(items) => {
            // `v.map(formatScalar)`: its `map` must be `Array.prototype.map` and its species
            // constructor a constructor (V8's `TypeError`s otherwise, as the HTML renderer's).
            value::array_method(v, "map", value::Method::ArrayMap, "v").map_err(throw)?;
            value::species_check(v).map_err(throw)?;
            let _level = level(COST_ARRAY)?;
            stack::grow(|| -> Result<Option<String>> {
                let mut parts = Vec::with_capacity(items.len());
                for i in 0..items.len() {
                    parts.push(match items.get(i) {
                        Some(item) => format_js_scalar(item)?.unwrap_or_default(),
                        None => String::new(),
                    });
                }
                Ok(Some(concat(&["[", &join_strings(&parts, SEP)?, "]"])?))
            })
        }
        other => {
            let _level = level(COST_OBJECT)?;
            stack::grow(|| -> Result<Option<String>> {
                let entries = value::own_entries(other);
                let mut parts = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key = if is_identifier(&key) {
                        key
                    } else {
                        format!("\"{key}\"")
                    };
                    let value = format_js_scalar(&value)?.unwrap_or_else(|| "undefined".into());
                    parts.push(concat(&[&key, ": ", &value])?);
                }
                Ok(Some(concat(&["{", &join_strings(&parts, SEP)?, "}"])?))
            })
        }
    }
}

/// `formatAnnotationValue(a)`.
fn format_annotation_value(kind: &str, name: &str, value: &Scalar) -> Result<Option<String>> {
    format_formatted_annotation(kind, name, value, format_scalar(value)?)
}

/// [`format_annotation_value`] once `formatScalar(a.value)` is made (`formatted`).
fn format_formatted_annotation(
    kind: &str,
    name: &str,
    value: &Scalar,
    formatted: Option<String>,
) -> Result<Option<String>> {
    let Some(formatted_value) = formatted else {
        return Ok(None);
    };
    if name == "primary" {
        return Ok(Some(formatted_value));
    }
    if let Scalar::String(id) = value
        && name == "id"
        && is_identifier(id)
    {
        return Ok(Some(concat(&["#", id])?));
    }
    if kind == "class" && is_identifier(name) {
        return Ok(Some(concat(&[".", name])?));
    }
    Ok(Some(concat(&[name, "=", &formatted_value])?))
}

/// `formatAttributes(n)`, collected (a value is `None` where upstream yields `undefined`).
fn format_attributes(n: NodeRef<'_>) -> Result<Vec<Option<String>>> {
    let mut out = Vec::new();
    for (key, value) in &n.attribute_entries()? {
        let value: &Scalar = value;
        // In cases where the class attribute is not a valid identifer, we treat it as a
        // regular attribute without the '.' sigil
        // (`typeof value === 'object'`: a JavaScript object the plain variants cannot hold
        // too, a function a document reached not.)
        let object = match value {
            Scalar::Null | Scalar::Array(_) | Scalar::Object(_) => true,
            Scalar::Js(js) => value::type_of(js.value()) == "object",
            _ => false,
        };
        if key == "class" && object && !is_ast(value) {
            // Upstream formats the whole value once per key; `formatScalar` is pure, so the
            // first key's result serves every key (made only once there is a key, as
            // upstream makes none for an object without keys).
            let mut formatted: Option<Option<String>> = None;
            for name in object_keys(value)? {
                let f = match &formatted {
                    Some(f) => f.clone(),
                    None => formatted.insert(format_scalar(value)?).clone(),
                };
                out.push(format_formatted_annotation("class", &name, value, f)?);
            }
        } else {
            out.push(format_annotation_value("attribute", key, value)?);
        }
    }
    Ok(out)
}

/// `Object.keys(value)` of an object (`null` throws).
fn object_keys(value: &Scalar) -> Result<Vec<String>> {
    match value {
        Scalar::Array(items) => Ok((0..items.len()).map(|i| i.to_string()).collect()),
        Scalar::Object(o) => Ok(o.keys().cloned().collect()),
        // Own enumerable string keys in a JS object's order (an array's present indices, then
        // its other properties).
        Scalar::Js(js) => Ok(value::own_entries(js.value())
            .into_iter()
            .map(|(k, _)| k)
            .collect()),
        _ => Err(throw(
            "TypeError: Cannot convert undefined or null to object",
        )),
    }
}

/// `formatAnnotations(n)`.
fn format_annotations(n: NodeRef<'_>, out: &mut dyn Sink) -> Result<()> {
    if truthy(&n.annotations_length()?) {
        out.put(Piece::Str(concat(&[OPEN, SPACE])?))?;
        let annotations = n.annotations()?;
        let mut parts = Vec::with_capacity(annotations.len());
        for a in &annotations {
            let formatted = match a {
                // A hand-made annotation that is not an attribute object: as it is.
                Annotation::Node(AttributeValue { raw: Some(raw), .. }) => {
                    format_annotation_object(raw)?
                }
                Annotation::Node(AttributeValue {
                    attr_type,
                    name,
                    value,
                    raw: None,
                }) => format_annotation_value(attr_type, name, value)?,
                Annotation::Value(a) => format_annotation_object(a)?,
            };
            parts.push(formatted.unwrap_or_default());
        }
        out.put(Piece::Str(join_strings(&parts, SPACE)?))?;
        out.put(Piece::Str(concat(&[SPACE, CLOSE])?))?;
    }
    Ok(())
}

/// `formatAnnotationValue(a)` of any value `a` (an item of a `Node` object's
/// `annotations`): `a.value`, `a.name` and `a.type` read as JavaScript reads them.
fn format_annotation_object(a: &Scalar) -> Result<Option<String>> {
    let value = get(a, "value")?;
    let Some(formatted_value) = format_scalar(&value)? else {
        return Ok(None);
    };
    let name = get(a, "name")?;
    if matches!(&*name, Scalar::String(n) if n == "primary") {
        return Ok(Some(formatted_value));
    }
    if let (Scalar::String(n), Scalar::String(id)) = (&*name, &*value)
        && n == "id"
        && is_identifier(id)
    {
        return Ok(Some(concat(&["#", id])?));
    }
    if let (Scalar::String(t), Scalar::String(n)) = (&*get(a, "type")?, &*name)
        && t == "class"
        && is_identifier(n)
    {
        return Ok(Some(concat(&[".", n])?));
    }
    Ok(Some(concat(&[
        &to_js_string(&name)?,
        "=",
        &formatted_value,
    ])?))
}

/// `formatVariable(v)`.
fn format_variable(v: &Variable, out: &mut dyn Sink) -> Result<()> {
    out.put(Piece::str("$"))?;
    let mut path = String::new();
    for (i, p) in v.path.iter().enumerate() {
        match p {
            PathSegment::String(s) if i == 0 => path.push_str(s),
            PathSegment::Number(n) if i == 0 => path.push_str(&js::number_to_string(*n)),
            PathSegment::String(s) if is_identifier(s) => {
                path.push('.');
                path.push_str(s);
            }
            PathSegment::Number(n) => {
                path.push('[');
                path.push_str(&js::number_to_string(*n));
                path.push(']');
            }
            PathSegment::String(s) => {
                path.push_str("[\"");
                path.push_str(s);
                path.push_str("\"]");
            }
        }
    }
    out.put(Piece::Str(checked(path)?))
}

/// `formatFunction(f)`.
fn format_function(f: &Function, out: &mut dyn Sink) -> Result<()> {
    out.put(Piece::str(f.name.as_str()))?;
    out.put(Piece::str("("))?;
    let mut parameters = Vec::with_capacity(f.parameters.len());
    for value in f.parameters.values() {
        parameters.push(format_scalar(value)?.unwrap_or_default());
    }
    out.put(Piece::Str(join_strings(&parameters, SEP)?))?;
    out.put(Piece::str(")"))
}

/// Which characters `escapeMarkdownCharacters` escapes.
#[derive(Clone, Copy)]
enum Escape {
    /// `/[()]/g` (a link's or an image's destination).
    Parentheses,
    /// `/[*_~]/g` (a text in `strong`, `em` or `s`).
    Emphasis,
    /// `/^\*|#+\s|^>/` (any other text): the first match only.
    Block,
}

/// `escapeMarkdownCharacters(s, characters)`: `s.replace(characters, '\\$&')`, then every
/// non-breaking space (U+00A0) as `&nbsp;`. `s` that is not a string throws.
fn escape_markdown_characters(s: &Scalar, characters: Escape, out: &mut dyn Sink) -> Result<()> {
    let Scalar::String(s) = s else {
        return Err(not_a_function(s, "s2", "replace"));
    };
    let escaped = match characters {
        Escape::Parentheses => escape_each(s, &['(', ')']),
        Escape::Emphasis => escape_each(s, &['*', '_', '~']),
        Escape::Block => escape_first_block_marker(s),
    };
    out.put(Piece::Str(checked(escaped.replace('\u{a0}', "&nbsp;"))?))
}

/// `s.replace(/[...]/g, '\\$&')`.
fn escape_each(s: &str, characters: &[char]) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if characters.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `s.replace(/^\*|#+\s|^>/, '\\$&')`: the leftmost match, the alternatives tried in order
/// at each position (`^` without the `m` flag is the start of the string).
fn escape_first_block_marker(s: &str) -> String {
    let bytes = s.as_bytes();
    let found = if s.starts_with('*') {
        Some((0, 1))
    } else {
        // `#+\s`: a run of `#` followed by JS whitespace, from the run's first `#`; a run
        // that is not followed by whitespace matches nowhere inside it either.
        let mut i = 0;
        let mut found = None;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let start = i;
                while i < bytes.len() && bytes[i] == b'#' {
                    i += 1;
                }
                if let Some(c) = s[i..].chars().next()
                    && is_js_space(c)
                {
                    found = Some((start, i + c.len_utf8()));
                    break;
                }
                continue;
            }
            if i == 0 && bytes[0] == b'>' {
                found = Some((0, 1));
                break;
            }
            i += 1;
        }
        found
    };
    match found {
        Some((start, end)) => [&s[..start], "\\", &s[start..end], &s[end..]].concat(),
        None => s.to_string(),
    }
}

/// `formatNode(n, o)`.
fn format_node<'a>(n: NodeRef<'a>, o: &Opts<'a>, out: &mut dyn Sink) -> Result<()> {
    let kind = Kind::of(n);
    let _level = level(node_cost(n, kind))?;
    stack::grow(|| format_node_at(n, kind, o, out))
}

/// [`format_node`] once its level is open.
fn format_node_at<'a>(n: NodeRef<'a>, kind: Kind, o: &Opts<'a>, out: &mut dyn Sink) -> Result<()> {
    let no = o.with_parent(n);
    let indent = repeat(SPACE, &no.indent_or_zero())?;
    let cx = Cx {
        n,
        o,
        no: &no,
        indent: &indent,
    };
    match kind {
        Kind::Document => cx.document(out),
        Kind::Heading => cx.heading(out),
        Kind::Paragraph => cx.paragraph(out),
        Kind::Inline => {
            out.put(Piece::str(indent.as_str()))?;
            format_children(n, &no, out)
        }
        Kind::Image => cx.image(out),
        Kind::Link => cx.link(out),
        Kind::Text => cx.text(out),
        Kind::Blockquote => cx.blockquote(out),
        Kind::Hr => {
            out.put(Piece::str(NL))?;
            out.put(Piece::str(indent.as_str()))?;
            out.put(Piece::str("---"))?;
            out.put(Piece::str(NL))
        }
        Kind::Fence => cx.fence(out),
        Kind::Tag => cx.tag(out),
        Kind::List => cx.list(out),
        Kind::Item => {
            // `for (let i = 0; i < n.children.length; i++)`
            let mut i = 0usize;
            while lt(i, &n.children_length()?)? {
                format_child(&n.child_at(i)?, &no, out)?;
                if i == 0 {
                    format_annotations(n, out)?;
                }
                i += 1;
            }
            Ok(())
        }
        Kind::Strong => cx.wrapped("**", out),
        Kind::Em => cx.wrapped("*", out),
        Kind::Code => {
            out.put(Piece::str("`"))?;
            let mut inner = Collect::default();
            format_scalar_value(&*n.attr("content")?, &no, &mut inner)?;
            format_inline(inner, out)?;
            out.put(Piece::str("`"))
        }
        Kind::S => {
            out.put(Piece::str("~~"))?;
            let mut inner = Collect::default();
            format_children(n, &no, &mut inner)?;
            format_inline(inner, out)?;
            out.put(Piece::str("~~"))
        }
        Kind::Hardbreak => {
            out.put(Piece::Str(concat(&["\\", NL])?))?;
            out.put(Piece::str(indent.as_str()))
        }
        Kind::Softbreak => {
            out.put(Piece::str(NL))?;
            out.put(Piece::str(indent.as_str()))
        }
        Kind::Table => cx.table(out),
        Kind::Thead => {
            let mut rows = Collect::default();
            format_children(n, &no, &mut rows)?;
            // `const [head] = [...]; yield head || [];`
            let head = rows.0.into_iter().next();
            out.put(match head {
                Some(head) if head.truthy() => head,
                _ => Piece::Array(Vec::new()),
            })
        }
        Kind::Tr => {
            let mut cells = Collect::default();
            format_children(n, &no, &mut cells)?;
            out.put(Piece::Array(cells.0))
        }
        Kind::Td | Kind::Th => {
            let mut cell = Collect::default();
            format_children(n, &no, &mut cell)?;
            format_annotations(n, &mut cell)?;
            let joined = join(&cell.0, "")?;
            out.put(Piece::str(trim(&joined)))
        }
        Kind::Tbody => format_children(n, &no, out),
        Kind::Comment => {
            let content = to_js_string(&*n.attr("content")?)?;
            out.put(Piece::Str(concat(&["<!-- ", &content, " -->\n"])?))
        }
        // "error", "node", any other name, and a `type` that is no string
        Kind::None => Ok(()),
    }
}

/// `i < x` with `x` a JavaScript value (`NaN` and `undefined` compare false).
#[inline]
fn lt(i: usize, x: &Scalar) -> Result<bool> {
    let x = to_number(x)?;
    Ok((i as f64).partial_cmp(&x) == Some(std::cmp::Ordering::Less))
}

/// What a `formatNode` call works with: the node, its options, `no` (`{...o, parent: n}`)
/// and `indent`. Each node type's code is a method, so that the frame a level of nesting
/// leaves on the stack is that one type's.
struct Cx<'a, 'c> {
    n: NodeRef<'a>,
    o: &'c Opts<'a>,
    no: &'c Opts<'a>,
    indent: &'c str,
}

impl<'a> Cx<'a, '_> {
    fn document(&self, out: &mut dyn Sink) -> Result<()> {
        let frontmatter = self.n.attr("frontmatter")?;
        if truthy(&frontmatter) && truthy(&length_of(&frontmatter)?) {
            let text = to_js_string(&frontmatter)?;
            out.put(Piece::Str(concat(&["---", NL, &text, NL, "---", NL, NL])?))?;
        }
        format_children(self.n, self.no, &mut TrimStart::new(out))
    }

    fn heading(&self, out: &mut dyn Sink) -> Result<()> {
        out.put(Piece::str(NL))?;
        out.put(Piece::str(self.indent))?;
        let level = self.n.attr("level")?;
        let count = if truthy(&level) {
            level
        } else {
            Cow::Owned(Scalar::Number(1.0))
        };
        out.put(Piece::Str(repeat("#", &count)?))?;
        out.put(Piece::str(SPACE))?;
        format_children(self.n, self.no, &mut TrimStart::new(out))?;
        format_annotations(self.n, out)?;
        out.put(Piece::str(NL))
    }

    fn paragraph(&self, out: &mut dyn Sink) -> Result<()> {
        out.put(Piece::str(NL))?;
        format_children(self.n, self.no, out)?;
        format_annotations(self.n, out)?;
        out.put(Piece::str(NL))
    }

    /// `SPACE + `"${title}"`` when the title is truthy, then `)`.
    fn title_and_close(&self, out: &mut dyn Sink) -> Result<()> {
        let title = self.n.attr("title")?;
        if truthy(&title) {
            let quoted = concat(&["\"", &to_js_string(&title)?, "\""])?;
            out.put(Piece::Str(concat(&[SPACE, &quoted])?))?;
        }
        out.put(Piece::str(")"))
    }

    /// `typeof dest === 'string' ? escapeMarkdownCharacters(dest, /[()]/g) :
    /// formatValue(dest, no)`.
    fn destination(&self, destination: &Scalar, out: &mut dyn Sink) -> Result<()> {
        match destination {
            Scalar::String(_) => escape_markdown_characters(destination, Escape::Parentheses, out),
            other => format_scalar_value(other, self.no, out),
        }
    }

    fn image(&self, out: &mut dyn Sink) -> Result<()> {
        out.put(Piece::str("!"))?;
        out.put(Piece::str("["))?;
        format_scalar_value(&*self.n.attr("alt")?, self.no, out)?;
        out.put(Piece::str("]"))?;
        out.put(Piece::str("("))?;
        self.destination(&*self.n.attr("src")?, out)?;
        self.title_and_close(out)
    }

    fn link(&self, out: &mut dyn Sink) -> Result<()> {
        let mut collected = Collect::default();
        format_children(self.n, self.no, &mut collected)?;
        let children = join(&collected.0, "")?;

        // https://spec.commonmark.org/0.31.2/#autolinks
        let href = self.n.attr("href")?;
        if matches!(&*href, Scalar::String(h) if *h == children) && !truthy(&*self.n.attr("title")?)
        {
            out.put(Piece::Str(concat(&["<", &children, ">"])?))?;
            return Ok(());
        }

        out.put(Piece::str("["))?;
        out.put(Piece::Str(children))?;
        out.put(Piece::str("]"))?;
        out.put(Piece::str("("))?;
        self.destination(&href, out)?;
        self.title_and_close(out)
    }

    fn text(&self, out: &mut dyn Sink) -> Result<()> {
        let content = self.n.destructured_content()?;
        if is_ast(&content) {
            out.put(Piece::Str(concat(&[OPEN, SPACE])?))?;
            format_scalar_value(&content, self.no, out)?;
            out.put(Piece::Str(concat(&[SPACE, CLOSE])?))
        } else if self
            .o
            .parent
            .and_then(NodeRef::type_name)
            .is_some_and(|t| WRAPPING_TYPES.contains(&t))
        {
            // Escape **strong**, _em_, and ~~s~~
            escape_markdown_characters(&content, Escape::Emphasis, out)
        } else {
            // Escape > blockquote, * list item, and heading
            escape_markdown_characters(&content, Escape::Block, out)
        }
    }

    fn blockquote(&self, out: &mut dyn Sink) -> Result<()> {
        let prefix = concat(&[NL, self.indent, ">", SPACE])?;
        let child_options = Opts {
            indent: Scalar::Number(0.0),
            ..self.no.clone()
        };
        let children = self.n.children_to_map()?;
        let mut inner = Vec::with_capacity(children.len());
        for child in &children {
            let formatted = run(|doc| format_child(child, &child_options, doc))?;
            inner.push(trim(&formatted).to_string());
        }
        let inner = join_strings(&inner, &[NL, NL].concat())?;
        let lines: Vec<String> = inner.split(NL).map(str::to_string).collect();
        let body = join_strings(&lines, &prefix)?;
        out.put(Piece::Str(concat(&[&prefix, &body, NL])?))
    }

    fn fence(&self, out: &mut dyn Sink) -> Result<()> {
        out.put(Piece::str(NL))?;
        out.put(Piece::str(self.indent))?;

        let content = self.n.attr("content")?;
        let Scalar::String(text) = &*content else {
            return Err(not_a_function(&content, "n.attributes.content", "match"));
        };
        // `content.match(/`{3,}/g) || []`, each match's length, `.reduce(max, 0)`.
        let mut inner_fence_length = 0;
        let mut run_length = 0;
        for c in text.chars().chain(std::iter::once('\0')) {
            if c == '`' {
                run_length += 1;
                continue;
            }
            if run_length >= 3 {
                inner_fence_length = inner_fence_length.max(run_length);
            }
            run_length = 0;
        }

        let boundary = repeat_n(
            "`",
            if inner_fence_length > 0 {
                inner_fence_length as f64 + 1.0
            } else {
                3.0
            },
        )?;
        let needs_nl_before_end_boundary = !text.ends_with(NL);

        out.put(Piece::Str(boundary.clone()))?;
        let language = self.n.attr("language")?;
        if truthy(&language) {
            out.put(Piece::of(&language))?;
        }
        if truthy(&self.n.annotations_length()?) {
            out.put(Piece::str(SPACE))?;
        }
        format_annotations(self.n, out)?;
        out.put(Piece::str(NL))?;
        out.put(Piece::str(self.indent))?;
        let lines: Vec<String> = text.split(NL).map(str::to_string).collect();
        out.put(Piece::Str(join_strings(
            &lines,
            &concat(&[NL, self.indent])?,
        )?))?; // yield* formatChildren(n, no);
        if needs_nl_before_end_boundary {
            out.put(Piece::str(NL))?;
        }
        out.put(Piece::Str(boundary))?;
        out.put(Piece::str(NL))
    }

    fn tag(&self, out: &mut dyn Sink) -> Result<()> {
        let n = self.n;
        if !n.inline() {
            out.put(Piece::str(NL))?;
            out.put(Piece::str(self.indent))?;
        }
        let open = concat(&[OPEN, SPACE])?;
        let attributes: Vec<String> = format_attributes(n)?.into_iter().flatten().collect();
        let name = n.tag_string()?;
        let mut tag = Vec::with_capacity(attributes.len() + 1);
        tag.push(concat(&[&open, &name])?);
        tag.extend(attributes);
        let inline_tag = join_strings(&tag, SPACE)?;

        let width = match self.o.max_tag_opening_width {
            Some(w) if w != 0.0 && !w.is_nan() => w,
            _ => MAX_TAG_OPENING_WIDTH,
        };
        let is_long_tag_opening = (len16(&inline_tag) + len16(&open) * 2) as f64 > width;

        // {% tag attributes={...} %}
        let opening = if !n.inline() && is_long_tag_opening {
            let separator = concat(&[NL, &repeat_n(SPACE, len16(&open) as f64)?, self.indent])?;
            join_strings(&tag, &separator)?
        } else {
            inline_tag
        };
        let has_children = truthy(&n.children_length()?);
        let slash = if has_children { "" } else { "/" };
        out.put(Piece::Str(concat(&[&opening, SPACE, slash, CLOSE])?))?;

        if truthy(&n.children_length()?) {
            let options = if self.no.allow_indentation {
                self.no.increment(&Scalar::Number(2.0))?
            } else {
                self.no.clone()
            };
            format_children(n, &options, out)?;
            if !n.inline() {
                out.put(Piece::str(self.indent))?;
            }
            // {% /tag %}
            out.put(Piece::Str(concat(&[
                OPEN,
                SPACE,
                "/",
                &n.tag_string()?,
                SPACE,
                CLOSE,
            ])?))?;
        }
        if !n.inline() {
            out.put(Piece::str(NL))?;
        }
        Ok(())
    }

    fn list(&self, out: &mut dyn Sink) -> Result<()> {
        let n = self.n;
        let is_loose = n.has_paragraph_grandchild()?;

        // `for (let i = 0; i < n.children.length; i++)`
        let mut i = 0usize;
        while lt(i, &n.children_length()?)? {
            let prefix = self.list_prefix(i)?;
            let options = self
                .no
                .increment(&js_add(&prefix.length()?, &Scalar::Number(1.0))?)?;
            let child = n.child_at(i)?;
            let formatted = run(|doc| format_child(&child, &options, doc))?;
            // `i === n.children.length - 1`
            let last = to_number(&n.children_length()?)? - 1.0 == i as f64;
            let d = if !is_loose || last {
                trim(&formatted)
            } else {
                &formatted
            };
            let prefix = prefix.to_js_string()?;
            out.put(Piece::Str(concat(&[NL, self.indent, &prefix, " ", d])?))?;
            i += 1;
        }
        out.put(Piece::str(NL))
    }

    /// The list's prefix for item `i`.
    fn list_prefix(&self, i: usize) -> Result<Piece> {
        let n = self.n;
        let marker = n.attr("marker")?.into_owned();
        if !truthy(&*n.attr("ordered")?) {
            return Ok(if nullish(&marker) {
                Piece::str(UL)
            } else {
                Piece::of(&marker)
            });
        }

        // Must be an ordered list now
        let mut number = "1".to_string();
        let start = n.attr("start")?;
        let start_number = if nullish(&start) {
            Cow::Owned(Scalar::Number(1.0))
        } else {
            start
        };
        if i == 0 {
            number = call_to_string(&start_number, "startNumber")?;
        }

        if self.o.increment {
            // `parseInt(startNumber)`: no radix (a `0x` prefix is hexadecimal).
            let start = js::parse_int(&js::to_utf16(&to_js_string(&start_number)?), 0);
            number = js::number_to_string(start + i as f64);
        }

        let marker = if nullish(&marker) {
            OL.to_string()
        } else {
            to_js_string(&marker)?
        };
        Ok(Piece::Str(concat(&[&number, &marker])?))
    }

    /// `strong` and `em`: the marker (or `fallback`), the children's text trimmed, the
    /// marker.
    fn wrapped(&self, fallback: &str, out: &mut dyn Sink) -> Result<()> {
        // `n.attributes.marker ?? fallback`
        let marker = || -> Result<Piece> {
            let marker = self.n.attr("marker")?.into_owned();
            Ok(if nullish(&marker) {
                Piece::str(fallback)
            } else {
                Piece::of(&marker)
            })
        };
        out.put(marker()?)?;
        let mut inner = Collect::default();
        format_children(self.n, self.no, &mut inner)?;
        format_inline(inner, out)?;
        out.put(marker()?)
    }

    fn table(&self, out: &mut dyn Sink) -> Result<()> {
        let mut rows = Collect::default();
        format_children(self.n, &self.no.increment(&Scalar::Number(2.0))?, &mut rows)?;
        let table = rows.0;
        let in_table_tag = self
            .o
            .parent
            .is_some_and(|p| p.type_name() == Some("tag") && p.tag_str() == Some("table"));
        if in_table_tag {
            for (i, row) in table.iter().enumerate() {
                // format tags like "if" in the middle of a table list
                if let Piece::Str(row) = row {
                    if !trim(row).is_empty() {
                        out.put(Piece::str(NL))?;
                        out.put(Piece::str(row.as_str()))?;
                    }
                } else {
                    if i != 0 {
                        out.put(Piece::str(NL))?;
                        out.put(Piece::Str(concat(&[self.indent, "---"])?))?;
                    }
                    for d in row.iterate("row")? {
                        let d = d.to_js_string()?;
                        out.put(Piece::Str(concat(&[NL, self.indent, UL, " ", &d])?))?;
                    }
                }
            }
            return out.put(Piece::str(NL));
        }

        // `widths[i]`: a JS array, holes `undefined`.
        let mut widths: Vec<Scalar> = Vec::new();
        for row in &table {
            let mut i = 0usize;
            loop {
                let length = to_number(&row.length()?)?;
                // `i < row.length` (false for NaN)
                if (i as f64).partial_cmp(&length) != Some(std::cmp::Ordering::Less) {
                    break;
                }
                let cell_length = row.index(i).length()?;
                let width = widths.get(i).cloned().unwrap_or(Scalar::Undefined);
                let value = if truthy(&width) {
                    Scalar::Number(js_max(to_number(&width)?, to_number(&cell_length)?))
                } else {
                    cell_length
                };
                if widths.len() <= i {
                    widths.resize(i + 1, Scalar::Undefined);
                }
                widths[i] = value;
                i += 1;
            }
        }
        let width = |i: usize| widths.get(i).cloned().unwrap_or(Scalar::Undefined);

        let mut table = table.into_iter();
        let head = table.next().unwrap_or(Piece::Value(Scalar::Undefined));
        let padded = |row: &Piece, expression: &str| -> Result<Vec<Piece>> {
            let mut cells = Vec::new();
            for (i, cell) in row.map_receiver(expression)?.iter().enumerate() {
                let count = Scalar::Number(to_number(&width(i))? - to_number(&cell.length()?)?);
                let pad = repeat(SPACE, &count)?;
                cells.push(Piece::Str(concat(&[&cell.to_js_string()?, &pad])?));
            }
            Ok(cells)
        };

        out.put(Piece::str(NL))?;
        format_table_row(&padded(&head, "head")?, out)?;
        out.put(Piece::str(NL))?;
        let mut rule = Vec::new();
        for i in 0..head.map_receiver("head")?.len() {
            rule.push(Piece::Str(repeat("-", &width(i))?));
        }
        format_table_row(&rule, out)?;
        out.put(Piece::str(NL))?;
        for row in table {
            format_table_row(&padded(&row, "row")?, out)?;
            out.put(Piece::str(NL))?;
        }
        Ok(())
    }
}

/// `Math.max(a, b)`: `NaN` if either is.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `formatValue(v, o)` of an AST value (a node's children are [`format_node`]'s).
fn format_scalar_value<'a>(v: &'a Scalar, o: &Opts<'a>, out: &mut dyn Sink) -> Result<()> {
    match v {
        Scalar::Undefined | Scalar::Null => Ok(()),
        Scalar::Boolean(b) => out.put(Piece::Str(b.to_string())),
        Scalar::Number(n) => out.put(Piece::Str(js::number_to_string(*n))),
        Scalar::String(s) => out.put(Piece::str(s.as_str())),
        Scalar::Array(items) => {
            let _level = level(COST_ARRAY)?;
            stack::grow(|| {
                for item in items {
                    format_scalar_value(item, o, out)?;
                }
                Ok(())
            })
        }
        Scalar::Function(f) => format_function(f, out),
        Scalar::Variable(v) => format_variable(v, out),
        Scalar::Js(js) => match js_ast_value(js.value())? {
            Some(ast) => format_scalar_value(&ast, o, out),
            None => format_scalar_value(&js_plain(js), o, out),
        },
        Scalar::Tag(_) | Scalar::Object(_) => match (v, &mdtype(v)) {
            (Scalar::Object(object), Scalar::String(t)) if t == "Node" => {
                format_node(NodeRef::Object(object), o, out)
            }
            (_, other) => Err(throw(format!(
                "Error: Unimplemented: \"{}\"",
                to_js_string(other)?
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    //! What node prints for the same input (`findings/conformance/work/api/unit_values.cjs`:
    //! `Markdoc.format(Markdoc.parse(src), options)` with `allowIndentation` and
    //! `allowComments` on). Upstream's own `formatter.test.ts` runs in `tests/next_suites.rs`.
    use super::*;
    use crate::ast::Variable;
    use crate::next::index::{parse_with, thrown};
    use crate::next::parser::ParserArgs;
    use crate::next::tokenizer::{Tokenizer, TokenizerOptions};

    fn fmt(src: &str, options: &FormatOptions<'_>) -> std::result::Result<String, String> {
        let tokenizer = Tokenizer::new(TokenizerOptions {
            allow_indentation: true,
            allow_comments: true,
            ..TokenizerOptions::default()
        });
        let ast = parse_with(&tokenizer, src, &ParserArgs::default()).unwrap();
        format(&ast, options).map_err(|e| thrown(&e).to_string())
    }

    fn ok(src: &str) -> String {
        fmt(src, &FormatOptions::default()).unwrap()
    }

    #[test]
    fn javascript_semantics_of_attribute_values() {
        // `'#'.repeat("abc")` is '', `.repeat(-1)` throws; a fence's `content` that is a
        // number has no `match`; a `language` is yielded as it is.
        assert_eq!(ok("# Hi {% level=\"abc\" %}\n"), "Hi {% level=\"abc\" %}\n");
        assert_eq!(
            fmt("# Hi {% level=-1 %}\n", &FormatOptions::default()).unwrap_err(),
            "RangeError: Invalid count value: -1"
        );
        assert_eq!(ok("# Hi {% level=3 %}\n"), "### Hi {% level=3 %}\n");
        assert_eq!(
            fmt(
                "```js {% content=5 %}\ncode\n```\n",
                &FormatOptions::default()
            )
            .unwrap_err(),
            "TypeError: n.attributes.content.match is not a function"
        );
        assert_eq!(
            ok("```js {% language=7 %}\ncode\n```\n"),
            "```7 {% language=7 %}\ncode\n```\n"
        );
        assert_eq!(ok("- a {% marker=5 %}\n- b\n"), "- a {% marker=5 %}\n- b\n");
    }

    #[test]
    fn blocks_and_escapes() {
        assert_eq!(
            ok("````\n```\nin\n```\n````\n"),
            "````\n```\nin\n```\n````\n"
        );
        assert_eq!(
            ok("\\* x\n\n\\# y\n\n\\> z\n\na\u{a0}b\n"),
            "\\* x\n\n\\# y\n\n\\> z\n\na&nbsp;b\n"
        );
        assert_eq!(ok("**a\\*b\\_c\\~d**\n"), "**a\\*b\\_c\\~d**\n");
        assert_eq!(
            ok("{% table %}\n- a\n- b\n---\n- c\n- d\n{% /table %}\n"),
            "{% table %}\n- a\n- b\n---\n- c\n- d\n{% /table %}\n"
        );
        assert_eq!(
            ok("| a | bb |\n|---|---|\n| ccc | d |\n"),
            "| a   | bb |\n| --- | -- |\n| ccc | d  |\n"
        );
        assert_eq!(ok("> - a\n> - b\n"), "> - a\n> - b\n");
        assert_eq!(ok("<http://a.b>\n"), "<http://a.b>\n");
        assert_eq!(ok("[a](u \"t\")\n"), "[a](u \"t\")\n");
        assert_eq!(ok("![i](a(b) \"t\")\n"), "![i](a\\(b\\) \"t\")\n");
        assert_eq!(ok("<!-- c -->\n"), "<!-- c -->\n");
        assert_eq!(ok("a\\\nb\n"), "a\\\nb\n");
        assert_eq!(ok("---\na: 1\n---\n# T\n"), "---\na: 1\n---\n\n# T\n");
    }

    #[test]
    fn tags_and_options() {
        assert_eq!(
            ok(
                "{% tag a=true b=\"My very long text well over 80 characters in total\" c=123456789 /%}\n"
            ),
            "{% tag\n   a=true\n   b=\"My very long text well over 80 characters in total\"\n   c=123456789 /%}\n"
        );
        let narrow = FormatOptions {
            max_tag_opening_width: Some(10.0),
            ..FormatOptions::default()
        };
        assert_eq!(
            fmt("{% tag a=1 b=2 /%}\n", &narrow).unwrap(),
            "{% tag\n   a=1\n   b=2 /%}\n"
        );
        let indented = FormatOptions {
            allow_indentation: true,
            ..FormatOptions::default()
        };
        assert_eq!(
            fmt("{% a %}\n{% b %}\nx\n{% /b %}\n{% /a %}\n", &indented).unwrap(),
            "{% a %}\n  {% b %}\n    x\n  {% /b %}\n{% /a %}\n"
        );
        let increment = FormatOptions {
            ordered_list_mode: Some(OrderedListMode::Increment),
            ..FormatOptions::default()
        };
        assert_eq!(fmt("3. a\n4. b\n", &increment).unwrap(), "3. a\n4. b\n");
        assert_eq!(ok("1. a\n2. b\n\n{% x /%}\n"), "1. a\n1. b\n\n{% x /%}\n");
        assert_eq!(
            ok(
                "{% x a=$v.b[\"c d\"][0] b=f(1, \"x\", [true, null]) c={k: {\"q r\": 1}} .cls #id /%}\n"
            ),
            "{% x a=$v.b[\"c d\"][0] b=f(1, \"x\", [true, null]) c={k: {\"q r\": 1}} .cls #id /%}\n"
        );
    }

    #[test]
    fn values_other_than_nodes() {
        let o = FormatOptions::default();
        assert_eq!(format_value(&Scalar::Null, &o).unwrap(), "");
        let path = vec![
            PathSegment::from("a"),
            PathSegment::from(1.0),
            PathSegment::from("b c"),
            PathSegment::from("d"),
        ];
        assert_eq!(
            format_value(&Scalar::Variable(Variable::new(path)), &o).unwrap(),
            "$a[1][\"b c\"].d"
        );
        let parameters = [
            ("0".to_string(), Scalar::Number(1.0)),
            ("x".to_string(), Scalar::String("y".into())),
        ];
        let f = Function::new("f".into(), parameters.into_iter().collect());
        assert_eq!(
            format_value(&Scalar::Function(f), &o).unwrap(),
            "f(1, \"y\")"
        );
        let tag = Scalar::Object([("$$mdtype".to_string(), Scalar::String("Tag".into()))].into());
        assert_eq!(
            thrown(&format_value(&tag, &o).unwrap_err()),
            "Error: Unimplemented: \"Tag\""
        );
        let fence = Node::new(
            NodeType::Fence,
            [("content".to_string(), Scalar::String("foo".into()))],
            vec![],
            None,
        );
        assert_eq!(format(&fence, &o).unwrap(), "```\nfoo\n```\n");
    }

    /// `n` block tags, each in the one before, over a paragraph.
    fn tags(n: usize) -> Node {
        let text = Node::new(
            NodeType::Text,
            [("content".to_string(), Scalar::String("x".into()))],
            vec![],
            None,
        );
        let inline = Node::new(NodeType::Inline, [], vec![text], None);
        let mut x = Node::new(NodeType::Paragraph, [], vec![inline], None);
        for _ in 0..n {
            x = Node::new(NodeType::Tag, [], vec![x], Some("a".into()));
        }
        Node::new(NodeType::Document, [], vec![x], None)
    }

    /// An attribute of `n` function calls, each the one before's argument.
    fn calls(n: usize) -> Node {
        let mut x = Scalar::Function(Function::new("f".into(), Default::default()));
        for _ in 0..n {
            x = Scalar::Function(Function::new("f".into(), [("0".to_string(), x)].into()));
        }
        let tag = Node::new(
            NodeType::Tag,
            [("b".to_string(), x)],
            vec![],
            Some("a".into()),
        );
        Node::new(NodeType::Document, [], vec![tag], None)
    }

    fn on_a_2_mib_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn nesting_stops_where_a_fresh_node_does() {
        // node 22, fresh: 818 tags and 983 calls format, one more throws RangeError.
        on_a_2_mib_thread(|| {
            let o = FormatOptions::default();
            let fresh = |shape| NODE_LIMITS.iter().find(|s| s.0 == shape).unwrap().1;
            for (shape, make) in [("tags", tags as fn(usize) -> Node), ("calls", calls)] {
                let n = fresh(shape);
                let at = make(n);
                let past = make(n + 1);
                if stack::can_grow() {
                    assert!(format(&at, &o).is_ok(), "{shape} {n}");
                } else {
                    // a fixed limit, below node's, keeps a 2 MiB stack from overflowing
                    assert!(format(&make(50), &o).is_ok(), "{shape} 50");
                }
                assert_eq!(
                    thrown(&format(&past, &o).unwrap_err()),
                    "RangeError: Maximum call stack size exceeded",
                    "{shape} {}",
                    n + 1
                );
            }
        });
    }

    /// A document whose one text holds `content` (a `Node` object, as JSON).
    fn holding(content: serde_json::Value) -> Node {
        let content = Scalar::from_json(&content).unwrap();
        let text = Node::new(
            NodeType::Text,
            [("content".to_string(), content)],
            vec![],
            None,
        );
        let inline = Node::new(NodeType::Inline, [], vec![text], None);
        let paragraph = Node::new(NodeType::Paragraph, [], vec![inline], None);
        Node::new(NodeType::Document, [], vec![paragraph], None)
    }

    #[test]
    fn node_objects_in_values_as_upstream_reads_them() {
        // node (work/api/node_objects_probe.cjs): Markdoc.format of a text whose content
        // Ast.fromJSON revived as a Node with fields of these types.
        use serde_json::json;
        let o = FormatOptions::default();
        let cases = [
            (
                json!({"$$mdtype": "Node", "type": "paragraph", "children": 5}),
                Err("TypeError: a.children is not iterable"),
            ),
            (
                json!({"$$mdtype": "Node", "type": "tr", "children": "ab"}),
                Ok("{% a,b %}\n"),
            ),
            (
                json!({"$$mdtype": "Node", "type": "tag", "attributes": "ab"}),
                Ok("{% \n{% undefined 0=\"a\" 1=\"b\" /%}\n %}\n"),
            ),
            (
                json!({"$$mdtype": "Node", "type": "text", "attributes": null}),
                Err(
                    "TypeError: Cannot destructure property 'content' of 'n.attributes' as it is null.",
                ),
            ),
            (
                json!({"$$mdtype": "Node", "type": "list", "children": [1, "x", null]}),
                Err("TypeError: Cannot read properties of undefined (reading 'some')"),
            ),
            (
                json!({"$$mdtype": "Node", "type": "item", "children": {"length": 2}}),
                Ok("{%  %}\n"),
            ),
            (json!({"$$mdtype": "Node", "type": 5}), Ok("{%  %}\n")),
        ];
        for (content, expected) in cases {
            let got = format(&holding(content.clone()), &o);
            match expected {
                Ok(text) => assert_eq!(got.unwrap(), text, "{content}"),
                Err(message) => assert_eq!(thrown(&got.unwrap_err()), message, "{content}"),
            }
        }
    }
}
