//! The Rust half of the next layer's comparison harness: this crate's values
//! as upstream's JSON, and this crate's values from upstream's JSON.
//!
//! Owner: the layer (harness). Not API: `examples/next_compare/` and the
//! porters' tests use it; it goes away with `next` at the switch-over.
//!
//! Every JSON here is what `JSON.stringify` writes of upstream's value, whole and with its
//! keys in upstream's order; nothing is left out or normalised on either side:
//!
//! - [`node_json`]: an [`ast::Node`](crate::ast::Node) as upstream's Node JSON, `lines`
//!   and `location` included.
//! - [`render_json`]: a render tree as upstream's (`{"$$mdtype":"Tag",...}` or a value).
//! - [`node_from_json`]: an `ast::Node` from the oracle's AST, so transform and
//!   validate can run on upstream's own ASTs.
//! - [`render_from_json`]: a render tree from the oracle's, so the HTML renderer
//!   can run on upstream's own trees.
//! - [`tokens_from_json`] / [`tokens_json`]: tokens from and to upstream's JSON of them
//!   (markdown-it's tokens with every field, `parseTags`' objects with theirs).
//! - [`errors_json`]: `validate()`'s output, `[{type, lines, location, error}]`.
//! - [`first_difference`]: the first place two such JSON values differ, object
//!   keys in order.
//! - [`to_json_string`]: JSON as `JSON.stringify` writes it (numbers included).
//!
//! `tests/next/compare.py` compares the same JSON in Python, as it is; `tests/next_compare.rs`
//! checks without node that upstream's values come back from the readers and writers here
//! as they were.

use crate::ast::Node;
use crate::markdown_it::{AttrValue, PseudoShape, PseudoToken, Token, js};
use crate::types::{
    AttributeValue, JsObject, Location, LocationEdge, NodeType, RenderableTreeNode, Scalar,
    ValidationError, ValidationLevel, json_number,
};
use indexmap::IndexMap;
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// (a) Upstream-shaped JSON
// ---------------------------------------------------------------------------

/// The keys of upstream's Node JSON, in the order `JSON.stringify` writes them: the class's
/// fields, then `location`, which the parser sets last (`tag` is left out when it is
/// undefined, `location` when the node has none).
pub const NODE_KEYS: [&str; 11] = [
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
    "location",
];

/// A node as upstream's JSON of it: `{"$$mdtype":"Node","errors":[{id,level,message,
/// location?}],"lines","inline","attributes","children","type","tag"?,"annotations",
/// "slots","location"?}`. Values are written by [`Scalar::to_json`]; this crate's own
/// [`Node::span`] is not upstream's and is not written.
///
/// The nodes are visited from a stack on the heap, so a tree of any depth costs no stack
/// here (the [`Value`] made is serde_json's, whose own drop and serialization recurse).
pub fn node_json(node: &Node) -> Value {
    node_json_with(node, &mut None)
}

/// [`node_json`] as text, as upstream's `JSON.stringify` writes it ([`to_json_string`]),
/// each value the AST holds as [`Scalar::Js`] written straight from the value
/// (`schema::value::json_stringify`), never held as a `serde_json::Value`: a sparse array's
/// holes cost their text (`null,`), as in node, not a JSON value each.
///
/// `Err` with what `JSON.stringify` throws where node's cannot write it: a value whose own
/// `JSON.stringify` throws (the first, in the order the text is written: a sparse array of
/// 2^32 - 1 holes is `RangeError: Invalid string length`), or a text past V8's longest
/// string.
pub fn node_json_text(node: &Node) -> Result<String, String> {
    let mut texts = Some(Vec::new());
    let json = node_json_with(node, &mut texts);
    let texts = texts.expect("kept");
    if let Some(error) = texts.iter().find_map(|t| t.strip_prefix(UNWRITABLE)) {
        return Err(error.to_string());
    }
    let mut out = String::new();
    write_json_with(&mut out, &json, &texts);
    if out.len() > crate::next::schema::value::MAX_STRING_LENGTH
        && out.encode_utf16().count() > crate::next::schema::value::MAX_STRING_LENGTH
    {
        return Err(crate::next::schema::value::INVALID_STRING_LENGTH.into());
    }
    Ok(out)
}

/// Marks a [`Scalar::Js`] text in [`node_json_text`] that `JSON.stringify` could not write:
/// the error follows.
const UNWRITABLE: &str = "\u{0}unwritable:";

/// What stands for the `k`th [`Scalar::Js`] in [`node_json_text`]'s JSON: an integer no
/// number of the AST is (they are `f64`s, and an integral one past 2^53 is written as a
/// float).
fn placeholder(k: usize) -> Value {
    Value::from(u64::MAX - k as u64)
}

/// [`placeholder`]'s index, for a number that is one.
fn placeholder_index(n: &serde_json::Number, count: usize) -> Option<usize> {
    let n = n.as_u64()?;
    let k = u64::MAX - n;
    ((k as usize) < count).then_some(k as usize)
}

/// A value's JSON (as [`Scalar::to_json`]), each [`Scalar::Js`] a [`placeholder`] when
/// `texts` collects their text.
fn scalar_json(value: &Scalar, texts: &mut Option<Vec<String>>) -> Value {
    let Some(collected) = texts else {
        return value.to_json();
    };
    match value {
        Scalar::Js(js) => {
            let text = match crate::next::schema::value::json_stringify(js.value(), "") {
                Ok(text) => text.unwrap_or_else(|| "null".into()),
                Err(error) => format!("{UNWRITABLE}{error}"),
            };
            collected.push(text);
            placeholder(collected.len() - 1)
        }
        Scalar::Array(items) => Value::Array(items.iter().map(|v| scalar_json(v, texts)).collect()),
        Scalar::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(_, v)| !matches!(v, Scalar::Undefined))
                .map(|(k, v)| (k.clone(), scalar_json(v, texts)))
                .collect(),
        ),
        Scalar::Function(f) => {
            let mut o = Map::new();
            o.insert("$$mdtype".into(), "Function".into());
            o.insert("name".into(), f.name.clone().into());
            o.insert(
                "parameters".into(),
                Value::Object(
                    f.parameters
                        .iter()
                        .filter(|(_, v)| !matches!(v, Scalar::Undefined))
                        .map(|(k, v)| (k.clone(), scalar_json(v, texts)))
                        .collect(),
                ),
            );
            Value::Object(o)
        }
        other => other.to_json(),
    }
}

fn node_json_with(node: &Node, texts: &mut Option<Vec<String>>) -> Value {
    /// A node whose JSON is being made: the JSON of its slots and children so far.
    struct Frame<'a> {
        node: &'a Node,
        slots: Map<String, Value>,
        children: Vec<Value>,
        /// How many of its slots, then children, are being or have been made.
        next: usize,
    }
    fn open(node: &Node) -> Frame<'_> {
        Frame {
            node,
            slots: Map::new(),
            children: Vec::with_capacity(node.children.len()),
            next: 0,
        }
    }
    let mut stack = vec![open(node)];
    loop {
        let top = stack.last_mut().expect("the node's own frame");
        let (slots, i) = (top.node.slots.len(), top.next);
        if i < slots + top.node.children.len() {
            top.next += 1;
            let next = match top.node.slots.get_index(i) {
                Some((_, slot)) => slot,
                None => &top.node.children[i - slots],
            };
            stack.push(open(next));
            continue;
        }
        let done = stack.pop().expect("the frame just looked at");
        let json = node_object(done.node, done.children, done.slots, texts);
        let Some(parent) = stack.last_mut() else {
            return json;
        };
        match parent.node.slots.get_index(parent.next - 1) {
            Some((name, _)) => {
                parent.slots.insert(name.clone(), json);
            }
            None => parent.children.push(json),
        }
    }
}

/// [`node_json`] of one node, given the JSON of its children and slots.
fn node_object(
    node: &Node,
    children: Vec<Value>,
    slots: Map<String, Value>,
    texts: &mut Option<Vec<String>>,
) -> Value {
    let mut o = Map::new();
    o.insert("$$mdtype".into(), "Node".into());
    o.insert(
        "errors".into(),
        Value::Array(node.errors.iter().map(error_json).collect()),
    );
    o.insert(
        "lines".into(),
        Value::Array(node.lines.iter().map(|&l| Value::from(l)).collect()),
    );
    o.insert("inline".into(), node.inline.into());
    o.insert("attributes".into(), object_json(&node.attributes, texts));
    o.insert("children".into(), Value::Array(children));
    o.insert("type".into(), node.node_type.as_str().into());
    if let Some(tag) = &node.tag {
        o.insert("tag".into(), tag.clone().into());
    }
    o.insert(
        "annotations".into(),
        Value::Array(
            node.annotations
                .iter()
                .map(|a| annotation_json(a, texts))
                .collect(),
        ),
    );
    o.insert("slots".into(), Value::Object(slots));
    if let Some(location) = &node.location {
        o.insert("location".into(), location.to_json());
    }
    Value::Object(o)
}

/// A render tree as upstream's JSON: a tag as `{"$$mdtype":"Tag","name",
/// "attributes","children"}`, a value as [`Scalar::to_json`] writes it.
pub fn render_json(node: &RenderableTreeNode) -> Value {
    node.to_json()
}

/// One entry of upstream's `validate()` output (`ValidateError`): the node's type, the
/// lines and location, and the error.
#[derive(Debug, Clone)]
pub struct ValidateRecord {
    pub node_type: String,
    pub lines: Vec<f64>,
    pub location: Option<Value>,
    pub error: ValidationError,
}

/// `validate()`'s output as `JSON.stringify` writes it: `[{type, lines, location?, error:
/// {id, level, message, location?}}]`.
pub fn errors_json(records: &[ValidateRecord]) -> Value {
    Value::Array(
        records
            .iter()
            .map(|r| {
                let mut o = Map::new();
                o.insert("type".into(), r.node_type.clone().into());
                o.insert(
                    "lines".into(),
                    Value::Array(r.lines.iter().map(|&l| json_number(l)).collect()),
                );
                if let Some(location) = &r.location {
                    o.insert("location".into(), location.clone());
                }
                o.insert("error".into(), error_json(&r.error));
                Value::Object(o)
            })
            .collect(),
    )
}

/// An error as upstream's JSON of it ([`ValidationError::to_json`]): `{id, level, message,
/// location?}` (`location` left out when the error has none, and `null` when it is `null`),
/// or the value a hand-made token gave ([`ValidationError::raw`]) as it is.
pub fn error_json(e: &ValidationError) -> Value {
    e.to_json()
}

/// An annotation's JSON: a `value` that is `undefined` (a hand-made token's) is left out; an
/// annotation that is not an attribute object (a hand-made token's) is written as it is.
fn annotation_json(a: &AttributeValue, texts: &mut Option<Vec<String>>) -> Value {
    if let Some(raw) = &a.raw {
        return scalar_json(raw, texts);
    }
    let mut o = Map::new();
    o.insert("type".into(), a.attr_type.clone().into());
    o.insert("name".into(), a.name.clone().into());
    if !matches!(a.value, Scalar::Undefined) {
        o.insert("value".into(), scalar_json(&a.value, texts));
    }
    Value::Object(o)
}

/// An object's JSON (see [`Scalar::to_json`]: a member that is `undefined` is left out).
fn object_json(object: &IndexMap<String, Scalar>, texts: &mut Option<Vec<String>>) -> Value {
    Value::Object(
        object
            .iter()
            .filter(|(_, v)| !matches!(v, Scalar::Undefined))
            .map(|(k, v)| (k.clone(), scalar_json(v, texts)))
            .collect(),
    )
}

/// Upstream's name for a level ([`ValidationLevel::name`]; `"other"` for a level that is not
/// one of the five: see [`ValidationLevel::to_json`] for the value).
pub fn level_name(level: &ValidationLevel) -> &'static str {
    level.name()
}

// ---------------------------------------------------------------------------
// (b) A Node from upstream's AST JSON
// ---------------------------------------------------------------------------

/// An `ast::Node` from upstream's Node JSON (the oracle's `ast`), so
/// transform and validate can run on upstream's own ASTs.
///
/// Every field upstream sets is read: `type`, `tag`, `attributes` (values as
/// `Ast.fromJSON` revives them: [`crate::next::parser::revive`]), `children`,
/// `inline`, `annotations` (likewise), `slots`, `errors` (each with its
/// `location` as it is), `lines` and `location`, all as upstream's. The node has no
/// [`Node::span`] (upstream's JSON has none).
pub fn node_from_json(value: &Value) -> Result<Node, String> {
    let o = value
        .as_object()
        .ok_or_else(|| format!("a Node is an object, not {}", short(value)))?;
    match o.get("$$mdtype") {
        None => {}
        Some(Value::String(t)) if t == "Node" => {}
        Some(other) => return Err(format!("expected a Node, found $$mdtype {other}")),
    }
    let node_type = match o.get("type") {
        Some(Value::String(t)) => NodeType::from_name(t),
        // `new Node()` is a 'node'.
        None => NodeType::Node,
        Some(other) => return Err(format!("a Node's type is a string, not {other}")),
    };
    let tag = match o.get("tag") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) => Some(t.clone()),
        Some(other) => return Err(format!("a Node's tag is a string, not {other}")),
    };
    // Set as JSON.parse sets them, in a JS object's order (index keys first, ascending),
    // through a JsObject: O(n log n) in any order.
    let mut attributes = JsObject::new();
    for (k, v) in object_field(o, "attributes")? {
        attributes.insert(k.clone(), crate::next::parser::revive(v)?);
    }
    let attributes: IndexMap<String, Scalar> = attributes.into();
    let children = array_field(o, "children")?
        .iter()
        .map(node_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    let mut node = Node::new(node_type, attributes, children, tag);
    node.inline = match o.get("inline") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => return Err(format!("a Node's inline is a boolean, not {other}")),
    };
    for a in array_field(o, "annotations")? {
        node.annotations
            .push(crate::next::parser::revive_annotation(a)?);
    }
    let mut slots = JsObject::new();
    for (name, slot) in object_field(o, "slots")? {
        slots.insert(name.clone(), node_from_json(slot)?);
    }
    node.slots = slots.into();
    for e in array_field(o, "errors")? {
        node.errors.push(error_from_json(e)?);
    }
    node.lines = array_field(o, "lines")?
        .iter()
        .map(|l| {
            l.as_u64()
                .map(|l| l as usize)
                .ok_or_else(|| format!("a line is a number, not {l}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    node.location = match o.get("location") {
        None => None,
        Some(l) => Some(location_from_json(l)?),
    };
    Ok(node)
}

/// An error of upstream's AST JSON, as the parser reads a token's errors
/// ([`crate::next::parser::error_from_json`]): `{id, level, message, location?}` into the
/// fields, any other value kept whole as [`ValidationError::raw`].
fn error_from_json(value: &Value) -> Result<ValidationError, String> {
    crate::next::parser::error_from_json(value).map_err(|e| e.to_string())
}

/// A node's location as upstream's parser makes it: `{file?, start: {line?, character?},
/// end: {...}}`, keys in that order (any other shape is not one the parser makes).
fn location_from_json(value: &Value) -> Result<Location, String> {
    let l = value
        .as_object()
        .ok_or_else(|| format!("a location is an object, not {}", short(value)))?;
    let keys: Vec<&str> = l.keys().map(String::as_str).collect();
    if keys != ["file", "start", "end"] && keys != ["start", "end"] {
        return Err(format!(
            "a node's location is {{file?, start, end}}, not {}",
            short(value)
        ));
    }
    let edge = |name: &str| -> Result<LocationEdge, String> {
        let e = l[name]
            .as_object()
            .ok_or_else(|| format!("a location's {name} is an object, not {}", l[name]))?;
        let keys: Vec<&str> = e.keys().map(String::as_str).collect();
        if !["line", "character"]
            .iter()
            .filter(|k| e.contains_key(**k))
            .eq(keys.iter())
        {
            return Err(format!(
                "a location edge is {{line?, character?}}, not {}",
                l[name]
            ));
        }
        Ok(LocationEdge {
            line: match e.get("line") {
                None => None,
                Some(n) => Some(
                    n.as_u64()
                        .ok_or_else(|| format!("a location's line is a number, not {n}"))?
                        as usize,
                ),
            },
            character: e.get("character").cloned().map(Box::new),
        })
    };
    let file = match l.get("file") {
        Some(Value::String(f)) => Some(f.as_str().into()),
        None => None,
        Some(other) => return Err(format!("a location's file is a string, not {other}")),
    };
    Ok(Location {
        file,
        start: edge("start")?,
        end: edge("end")?,
    })
}

/// A render tree from upstream's JSON (the oracle's `transformed`).
pub fn render_from_json(value: &Value) -> Result<RenderableTreeNode, String> {
    RenderableTreeNode::from_json(value)
}

// ---------------------------------------------------------------------------
// (c) Tokens from and to upstream's JSON
// ---------------------------------------------------------------------------

/// The fields of markdown-it's `Token`, in the order its class defines them (the fields with
/// an initializer, then those its constructor sets), which is the order `JSON.stringify`
/// writes them; Markdoc's plugins add `errors` after them.
pub const TOKEN_FIELDS: [&str; 13] = [
    "map", "level", "children", "content", "markup", "info", "block", "hidden", "type", "tag",
    "attrs", "nesting", "meta",
];

/// Tokens from upstream's JSON of them (the oracle's `tokens`), so the parser can parse
/// upstream's own token streams: markdown-it's tokens with their fields (`null` read as
/// `None`), and the objects `parseTags` makes for a fence's content (an object with `start`
/// and no `level`), which become tokens with [`Token::pseudo`] set. A field left out reads as
/// a token's default (`""`, `None`, 0, `false`), so the shorter forms other tools write
/// (`plain()`'s) read too.
pub fn tokens_from_json(value: &Value) -> Result<Vec<Token>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("tokens are an array, not {}", short(value)))?
        .iter()
        .map(token_from_json)
        .collect()
}

/// One token from upstream's JSON of it. See [`tokens_from_json`].
pub fn token_from_json(value: &Value) -> Result<Token, String> {
    let o = value
        .as_object()
        .ok_or_else(|| format!("a token is an object, not {}", short(value)))?;
    let text = |name: &str| -> Result<String, String> {
        match o.get(name) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(other) => Err(format!("a token's {name} is a string, not {other}")),
        }
    };
    let mut t = Token::new(&text("type")?, &text("tag")?, 0);
    t.content = text("content")?;
    t.markup = text("markup")?;
    t.info = text("info")?;
    if let Some(n) = o.get("nesting").filter(|v| !v.is_null()) {
        t.nesting = n
            .as_i64()
            .filter(|n| (-1..=1).contains(n))
            .ok_or_else(|| format!("a token's nesting is -1, 0 or 1, not {n}"))?
            as i8;
    }
    if let Some(n) = o.get("level").filter(|v| !v.is_null()) {
        t.level = n
            .as_i64()
            .ok_or_else(|| format!("a token's level is an integer, not {n}"))?
            as i32;
    }
    t.block = flag(o, "block")?;
    t.hidden = flag(o, "hidden")?;
    if let Some(attrs) = o.get("attrs").filter(|v| !v.is_null()) {
        let attrs = attrs
            .as_array()
            .ok_or_else(|| format!("a token's attrs are an array, not {attrs}"))?;
        let mut out = Vec::with_capacity(attrs.len());
        for a in attrs {
            let (name, value) = match a.as_array().map(Vec::as_slice) {
                Some([Value::String(name), value]) => (name.clone(), value),
                _ => return Err(format!("an attr is [name, value], not {a}")),
            };
            let value = match value {
                Value::String(s) => AttrValue::Str(s.clone()),
                Value::Number(n) => AttrValue::Num(n.as_f64().expect("a JSON number")),
                other => {
                    return Err(format!(
                        "an attr's value is a string or a number, not {other}"
                    ));
                }
            };
            out.push((name, value));
        }
        t.attrs = Some(out);
    }
    if let Some(map) = o.get("map").filter(|v| !v.is_null()) {
        t.map = match map.as_array().map(Vec::as_slice) {
            Some([a, b]) => match (a.as_u64(), b.as_u64()) {
                (Some(a), Some(b)) => Some([a as usize, b as usize]),
                _ => return Err(format!("a token's map is two lines, not {map}")),
            },
            _ => return Err(format!("a token's map is two lines, not {map}")),
        };
    }
    t.meta = o.get("meta").filter(|v| !v.is_null()).cloned();
    if let Some(errors) = o.get("errors").filter(|v| !v.is_null()) {
        t.errors = Some(
            errors
                .as_array()
                .ok_or_else(|| format!("a token's errors are an array, not {errors}"))?
                .clone(),
        );
    }
    // `null` is kept: upstream's parser throws reading a property of it.
    t.position = o.get("position").cloned().map(Box::new);
    if let Some(children) = o.get("children").filter(|v| !v.is_null()) {
        t.children = Some(tokens_from_json(children)?);
    }
    if o.contains_key("start") && !o.contains_key("level") {
        let offset = |name: &str| {
            o.get(name)
                .and_then(Value::as_i64)
                .ok_or_else(|| format!("a parseTags object's {name} is an integer"))
        };
        t.pseudo = Some(Box::new(PseudoToken {
            start: offset("start")?,
            end: offset("end")?,
            shape: if o.contains_key("info") || o.contains_key("map") {
                PseudoShape::Tag {
                    nesting: o.contains_key("nesting"),
                }
            } else {
                PseudoShape::Text
            },
        }));
    }
    Ok(t)
}

fn flag(o: &Map<String, Value>, name: &str) -> Result<bool, String> {
    match o.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(other) => Err(format!("a token's {name} is a boolean, not {other}")),
    }
}

/// `Ast.fromJSON(JSON.stringify(tokens))`'s throw, if it throws: the reviver's `new klass()`
/// for a `$$mdtype` that names a method every object inherits (see
/// [`crate::next::parser::revive`]).
pub fn revive_check(tokens: &Value) -> Result<(), String> {
    crate::next::parser::revive(tokens).map(|_| ())
}

/// [`tokens_json`] of tokens a request gave, each `meta` as `Ast.fromJSON` revives it and
/// `JSON.stringify` then writes it (a `$$mdtype` object is its class's instance: a
/// `Variable` gains `path`, a `Node` its fields; a `__proto__` key that set an instance's
/// prototype is gone), as the oracle writes them.
pub fn tokens_json_revived(tokens: &[Token]) -> Value {
    let mut json = tokens_json(tokens);
    let mut stack = vec![&mut json];
    while let Some(v) = stack.pop() {
        match v {
            Value::Array(items) => stack.extend(items.iter_mut()),
            Value::Object(token) => {
                if let Some(meta) = token.get_mut("meta")
                    && let Ok(revived) = crate::next::parser::revive(meta)
                {
                    *meta = revived.to_json();
                }
                if let Some(children) = token.get_mut("children") {
                    stack.push(children);
                }
            }
            _ => {}
        }
    }
    json
}

/// The oracle's tokens output for a request's own tokens (a hand-made stream): the request's
/// JSON as `Ast.fromJSON` revives it (every `$$mdtype` object its class's instance: see
/// [`crate::next::parser::revive`]), each token (and each token in a `children` array) given
/// `''` for an absent `content`, `markup`, `info` and `tag`, in that order after its own keys,
/// as `tests/next/oracle.cjs`'s `reviveTokens` does, then `JSON.stringify`'d. The JSON as it
/// is where reviving throws ([`revive_check`] says so first).
pub fn request_tokens_json_revived(tokens: &Value) -> Value {
    let mut json = match crate::next::parser::revive(tokens) {
        Ok(revived) => revived.to_json(),
        Err(_) => return tokens.clone(),
    };
    let mut stack: Vec<&mut Value> = match &mut json {
        Value::Array(items) => items.iter_mut().collect(),
        _ => Vec::new(),
    };
    while let Some(token) = stack.pop() {
        if let Value::Object(fields) = token {
            for f in ["content", "markup", "info", "tag"] {
                if !fields.contains_key(f) {
                    fields.insert(f.into(), Value::String(String::new()));
                }
            }
            if let Some(Value::Array(children)) = fields.get_mut("children") {
                stack.extend(children.iter_mut());
            }
        }
    }
    json
}

/// Tokens as `JSON.stringify` writes upstream's (see [`token_json`]).
pub fn tokens_json(tokens: &[Token]) -> Value {
    Value::Array(tokens.iter().map(token_json).collect())
}

/// One token as `JSON.stringify` writes upstream's: a markdown-it token with every field of
/// [`TOKEN_FIELDS`] in that order (`null` for `None`), then `errors` when a plugin set them
/// (and `position`, which only a hand-made token has); an object of `parseTags` with its own
/// fields alone ([`PseudoToken`]): `{type, start, end, content}` for a text, `{map,
/// position, start, end, info, type, nesting?, meta}` for a tag.
pub fn token_json(t: &Token) -> Value {
    let mut o = Map::new();
    let map = || match t.map {
        Some([a, b]) => Value::Array(vec![a.into(), b.into()]),
        None => Value::Null,
    };
    let meta = || t.meta.clone().unwrap_or(Value::Null);
    match t.pseudo.as_deref() {
        Some(PseudoToken {
            start,
            end,
            shape: PseudoShape::Text,
        }) => {
            o.insert("type".into(), t.kind.clone().into());
            o.insert("start".into(), (*start).into());
            o.insert("end".into(), (*end).into());
            o.insert("content".into(), t.content.clone().into());
        }
        Some(PseudoToken {
            start,
            end,
            shape: PseudoShape::Tag { nesting },
        }) => {
            o.insert("map".into(), map());
            o.insert(
                "position".into(),
                t.position.as_deref().cloned().unwrap_or(Value::Null),
            );
            o.insert("start".into(), (*start).into());
            o.insert("end".into(), (*end).into());
            o.insert("info".into(), t.info.clone().into());
            o.insert("type".into(), t.kind.clone().into());
            if *nesting {
                o.insert("nesting".into(), t.nesting.into());
            }
            o.insert("meta".into(), meta());
        }
        None => {
            o.insert("map".into(), map());
            o.insert("level".into(), t.level.into());
            o.insert(
                "children".into(),
                t.children.as_deref().map_or(Value::Null, tokens_json),
            );
            o.insert("content".into(), t.content.clone().into());
            o.insert("markup".into(), t.markup.clone().into());
            o.insert("info".into(), t.info.clone().into());
            o.insert("block".into(), t.block.into());
            o.insert("hidden".into(), t.hidden.into());
            o.insert("type".into(), t.kind.clone().into());
            o.insert("tag".into(), t.tag.clone().into());
            o.insert(
                "attrs".into(),
                match &t.attrs {
                    Some(attrs) => Value::Array(
                        attrs
                            .iter()
                            .map(|(name, value)| {
                                let value = match value {
                                    AttrValue::Str(s) => Value::String(s.clone()),
                                    AttrValue::Num(n) => json_number(*n),
                                };
                                Value::Array(vec![Value::String(name.clone()), value])
                            })
                            .collect(),
                    ),
                    None => Value::Null,
                },
            );
            o.insert("nesting".into(), t.nesting.into());
            o.insert("meta".into(), meta());
            if let Some(errors) = &t.errors {
                o.insert("errors".into(), Value::Array(errors.clone()));
            }
            if let Some(position) = &t.position {
                o.insert("position".into(), (**position).clone());
            }
        }
    }
    Value::Object(o)
}

// ---------------------------------------------------------------------------
// Comparing and printing
// ---------------------------------------------------------------------------

/// The first place `a` and `b` differ, as (path, a's value, b's value):
/// arrays item by item, objects key by key **in order** (a key order that
/// differs is a difference), numbers as JS numbers (`1` is `1.0`; `true` is
/// not `1`).
pub fn first_difference(a: &Value, b: &Value) -> Option<(String, String, String)> {
    diff_at(a, b, &mut String::new())
}

fn diff_at(a: &Value, b: &Value, path: &mut String) -> Option<(String, String, String)> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let (kx, ky): (Vec<&String>, Vec<&String>) = (x.keys().collect(), y.keys().collect());
            if kx != ky {
                let i = kx.iter().zip(&ky).take_while(|(p, q)| p == q).count();
                return Some((
                    format!("{path}{{keys from #{i}}}"),
                    format!("{:?}", &kx[i.min(kx.len())..]),
                    format!("{:?}", &ky[i.min(ky.len())..]),
                ));
            }
            for (k, v) in x {
                let len = path.len();
                path.push('.');
                path.push_str(k);
                let d = diff_at(v, &y[k], path);
                path.truncate(len);
                if d.is_some() {
                    return d;
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                let d = diff_at(p, q, path);
                path.truncate(len);
                if d.is_some() {
                    return d;
                }
            }
            (x.len() != y.len()).then(|| {
                (
                    format!("{path}.length"),
                    x.len().to_string(),
                    y.len().to_string(),
                )
            })
        }
        (Value::Number(x), Value::Number(y)) => {
            (x.as_f64() != y.as_f64()).then(|| (path.clone(), to_json_string(a), to_json_string(b)))
        }
        _ => (a != b).then(|| (path.clone(), short(a), short(b))),
    }
}

fn short(v: &Value) -> String {
    let s = to_json_string(v);
    if s.chars().count() > 200 {
        format!("{}...", s.chars().take(200).collect::<String>())
    } else {
        s
    }
}

/// `value` as `JSON.stringify` writes it: compact, keys in the value's order,
/// numbers as JS prints them (`1`, `1e+21`, `1e-7`), only `"`, `\` and
/// control characters escaped.
pub fn to_json_string(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value);
    out
}

/// [`to_json_string`], appending to `out`.
pub fn write_json(out: &mut String, value: &Value) {
    write_json_with(out, value, &[])
}

/// [`write_json`], each [`placeholder`] number written as the text it stands for.
fn write_json_with(out: &mut String, value: &Value, texts: &[String]) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if placeholder_index(n, texts.len()).is_some() => {
            out.push_str(&texts[placeholder_index(n, texts.len()).expect("checked")]);
        }
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => out.push_str(&i.to_string()),
            (_, Some(u), _) => out.push_str(&u.to_string()),
            (_, _, Some(f)) if f.is_finite() => out.push_str(&js::number_to_string(f)),
            _ => out.push_str("null"),
        },
        Value::String(s) => write_string(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_with(out, x, texts);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, k);
                out.push(':');
                write_json_with(out, x, texts);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn object_field<'a>(
    o: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Map<String, Value>, String> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match o.get(name) {
        None | Some(Value::Null) => Ok(EMPTY.get_or_init(Map::new)),
        Some(Value::Object(m)) => Ok(m),
        Some(other) => Err(format!(
            "a Node's {name} is an object, not {}",
            short(other)
        )),
    }
}

fn array_field<'a>(o: &'a Map<String, Value>, name: &str) -> Result<&'a [Value], String> {
    match o.get(name) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(other) => Err(format!("a Node's {name} is an array, not {}", short(other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What the oracle prints for `{% foo a=1 b=$v.x[0]["k"] f=eq(1, x=$y)
    /// o={b: 1, 0: 2} %}` (trimmed to one node).
    fn upstream_tag_node() -> Value {
        json!({"$$mdtype":"Node","errors":[{"id":"x","level":"warning","message":"m",
            "location":{"start":{"offset":3},"end":{"offset":9}}}],"lines":[2,3,4,5],"inline":false,
            "attributes":{"a":1,"b":{"$$mdtype":"Variable","path":["v","x",0,"k"]},
              "f":{"$$mdtype":"Function","name":"eq","parameters":{"0":1,"x":{"$$mdtype":"Variable","path":["y"]}}},
              "o":{"0":2,"b":1}},
            "children":[],"type":"tag","tag":"foo",
            "annotations":[{"type":"attribute","name":"a","value":1}],
            "slots":{},"location":{"start":{"line":2},"end":{"line":3}}})
    }

    /// Takes a JSON value apart one array or object at a time (serde_json's drop recurses).
    fn drop_json(value: Value) {
        let mut stack = vec![value];
        while let Some(value) = stack.pop() {
            match value {
                Value::Array(items) => stack.extend(items),
                Value::Object(fields) => stack.extend(fields.into_iter().map(|(_, v)| v)),
                _ => {}
            }
        }
    }

    #[test]
    fn node_json_of_a_tree_of_any_depth() {
        // A chain 50,000 nodes deep, each with two slots and a second child, on a 2 MiB stack:
        // its JSON is made without recursion (the recursive one aborted at 17,002 levels).
        const DEPTH: usize = 50_000;
        let (levels, first) = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let leaf = |name: &str| {
                    Node::new(
                        NodeType::Text,
                        IndexMap::new(),
                        Vec::new(),
                        Some(name.into()),
                    )
                };
                let mut node = leaf("bottom");
                for _ in 0..DEPTH {
                    let mut parent =
                        Node::new(NodeType::Tag, IndexMap::new(), vec![node, leaf("c")], None);
                    parent.slots.insert("b".into(), leaf("s1"));
                    parent.slots.insert("a".into(), leaf("s2"));
                    node = parent;
                }
                let json = node_json(&node);
                let first = to_json_string(&json["slots"]) + &to_json_string(&json["children"][1]);
                let mut levels = 0;
                let mut at = &json;
                while let Some(child) = at["children"].get(0) {
                    at = child;
                    levels += 1;
                }
                assert_eq!(at["tag"], "bottom");
                drop_json(json);
                (levels, first)
            })
            .expect("a thread")
            .join()
            .expect("no overflow");
        assert_eq!(levels, DEPTH);
        assert_eq!(
            first,
            concat!(
                r#"{"b":{"$$mdtype":"Node","errors":[],"lines":[],"inline":false,"attributes":{},"children":[],"type":"text","tag":"s1","annotations":[],"slots":{}},"#,
                r#""a":{"$$mdtype":"Node","errors":[],"lines":[],"inline":false,"attributes":{},"children":[],"type":"text","tag":"s2","annotations":[],"slots":{}}}"#,
                r#"{"$$mdtype":"Node","errors":[],"lines":[],"inline":false,"attributes":{},"children":[],"type":"text","tag":"c","annotations":[],"slots":{}}"#
            )
        );
    }

    #[test]
    fn a_node_round_trips_through_upstream_json() {
        let upstream = upstream_tag_node();
        let node = node_from_json(&upstream).unwrap();
        assert_eq!(node.lines, [2, 3, 4, 5]);
        assert!(matches!(node.attributes["b"], Scalar::Variable(_)));
        assert!(matches!(node.attributes["f"], Scalar::Function(_)));
        // A grammar error's location (offsets only) is kept as it is.
        assert_eq!(
            node.errors[0].location,
            Some(json!({"start":{"offset":3},"end":{"offset":9}}))
        );
        let back = node_json(&node);
        assert_eq!(first_difference(&upstream, &back), None);
        assert_eq!(to_json_string(&upstream), to_json_string(&back));
        assert_eq!(
            to_json_string(&back["attributes"]),
            r#"{"a":1,"b":{"$$mdtype":"Variable","path":["v","x",0,"k"]},"f":{"$$mdtype":"Function","name":"eq","parameters":{"0":1,"x":{"$$mdtype":"Variable","path":["y"]}}},"o":{"0":2,"b":1}}"#
        );
    }

    #[test]
    fn attribute_order_is_a_difference() {
        let a = json!({"attributes": {"x": 1, "y": 2}});
        let b = json!({"attributes": {"y": 2, "x": 1}});
        let (path, _, _) = first_difference(&a, &b).unwrap();
        assert_eq!(path, ".attributes{keys from #0}");
        assert_eq!(first_difference(&json!([1, 2.5]), &json!([1.0, 2.5])), None);
        assert!(first_difference(&json!(true), &json!(1)).is_some());
    }

    #[test]
    fn tokens_round_trip_through_upstreams_json() {
        // What upstream writes for a fence with tags (parseTags' objects in `children`, a
        // variable's without `nesting`) and for an ordered list's opening.
        let upstream = json!([
            {"map":[0,3],"level":0,"children":[
                {"type":"text","start":0,"end":1,"content":"x "},
                {"map":[1,2],"position":{"start":3,"end":10},"start":2,"end":8,"info":"{% a %}","type":"tag_open","nesting":1,"meta":{"tag":"a","attributes":null}},
                {"map":[1,2],"position":{"start":10,"end":18},"start":9,"end":16,"info":"{% $v %}","type":"variable","meta":{"variable":{"$$mdtype":"Variable","path":["v"]}}},
                {"type":"text","start":17,"end":16,"content":""}],
             "content":"x {% a %}{% $v %}","markup":"```","info":"js {% process=false %}","block":true,"hidden":false,
             "type":"fence","tag":"code","attrs":null,"nesting":0,
             "meta":{"attributes":[{"type":"attribute","name":"process","value":false}]},"errors":[]},
            {"map":[3,4],"level":0,"children":null,"content":"","markup":".","info":"","block":true,"hidden":true,
             "type":"ordered_list_open","tag":"ol","attrs":[["start",2]],"nesting":1,"meta":null}
        ]);
        let tokens = tokens_from_json(&upstream).unwrap();
        assert_eq!(
            tokens[1].attrs,
            Some(vec![("start".into(), AttrValue::Num(2.0))])
        );
        let children = tokens[0].children.as_ref().unwrap();
        assert_eq!(children.len(), 4);
        assert_eq!(
            children[2].pseudo.as_deref(),
            Some(&PseudoToken {
                start: 9,
                end: 16,
                shape: PseudoShape::Tag { nesting: false }
            })
        );
        let back = tokens_json(&tokens);
        assert_eq!(first_difference(&upstream, &back), None);
        assert_eq!(to_json_string(&upstream), to_json_string(&back));
    }

    #[test]
    fn a_render_tree_round_trips_with_tags_in_attributes() {
        let upstream = json!({"$$mdtype":"Tag","name":"foo","attributes":{"qux":"test",
            "bar":[{"$$mdtype":"Tag","name":"p","attributes":{},"children":["This is a test"]}]},
            "children":[{"$$mdtype":"Tag","name":"p","attributes":{},"children":["x",1.5,null,[1,[2]]]}]});
        let tree = render_from_json(&upstream).unwrap();
        let RenderableTreeNode::Tag(tag) = &tree else {
            panic!("a tag")
        };
        let Scalar::Array(bar) = &tag.attributes["bar"] else {
            panic!("an array")
        };
        assert!(matches!(bar[0], Scalar::Tag(_)));
        assert_eq!(first_difference(&upstream, &render_json(&tree)), None);
    }

    #[test]
    fn numbers_print_as_js_prints_them() {
        for (n, s) in [
            (1.0, "1"),
            (-0.0, "0"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (0.1, "0.1"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(to_json_string(&Scalar::Number(n).to_json()), s);
        }
        assert_eq!(to_json_string(&Scalar::Number(f64::NAN).to_json()), "null");
    }
}
