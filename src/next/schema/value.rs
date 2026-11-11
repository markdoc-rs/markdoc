//! The JavaScript values upstream's semantics handle once a document is resolved.
//!
//! Owner: the semantics agent; the object model (prototypes, sparse arrays, extra
//! properties, the operations on them): the model agent (A2).
//!
//! [`Scalar`] is the AST's value (what the parser writes and JSON can hold). Resolving a
//! document turns each attribute into a [`Value`]: what upstream's `resolve` returns, which
//! JSON cannot always hold:
//!
//! - `undefined` ([`Value::Undefined`]): a missing variable, a configured function without a
//!   `transform`, `default()`'s fallback. Upstream relies on it: `value === undefined ?
//!   attr.default : value`, `'primary' in attributes`, and `JSON.stringify` leaving out an
//!   object member that is `undefined` (an array item becomes `null`).
//! - identity: `===` compares objects and arrays by reference. A value read from
//!   `config.variables` is the same object each time it is read; an array or object literal
//!   in the document is a new one each time it is resolved. Arrays and objects are held in
//!   an [`Arc`], and `===` is [`Arc::ptr_eq`].
//! - a rendered tag ([`Value::Tag`], upstream's `Tag` instance), which slots and transforms
//!   put into values.
//! - strings as JavaScript holds them: UTF-16 code units, which can be a lone surrogate
//!   ([`JsString`]; see "Strings" below).
//!
//! # Objects
//!
//! A value that is an object has what a JavaScript object has:
//!
//! - **A prototype** ([`Proto`]): a plain object's is `Object.prototype`; an object literal or
//!   a function's parameters can have another one (a `__proto__` key after the first, a named
//!   `__proto__` parameter, `output[k] = ...` in `resolve`), an object, an array, a tag or
//!   `null`. Every read goes through it ([`get`], [`has_property`], `ToPrimitive`, `Tag.isTag`,
//!   `value.constructor`); what only enumerates own properties (`Object.entries`, a spread,
//!   `JSON.stringify`) does not. A chain ends at a built-in object of the realm
//!   ([`crate::next::realm`], reached through [`realm`]), which answers for it: a built-in is
//!   itself a value ([`Value::Builtin`]: `$s.trim`, `$x.constructor`, `$x.__proto__`), and
//!   what upstream calls (a `toString`, a `toJSON`, a `resolve`) is called as V8 runs it.
//! - **Instances of upstream's AST classes**: a `Variable` is an object with the own properties
//!   `$$mdtype` and `path` and the prototype `Variable.prototype` (whose `resolve` resolves it),
//!   a `Function` one with `$$mdtype`, `name` and `parameters` (`Function.prototype`), a `Node`
//!   one with a node's fields (`Node.prototype`). A document can give them more properties or
//!   replace theirs (`{% x class=$v .path /%}` makes `path` true), and a value whose prototype
//!   is one is read through it (`{x: 1, __proto__: $v}` resolves as the variable).
//! - **Arrays** ([`Array`]): elements up to a length of 2^32 - 1, with holes (sparse, as V8
//!   holds them: no memory for a hole), and any other own property (`.b` on an array `class`
//!   is `b: true`, `.constructor` makes the species constructor `true`). A hole is absent in
//!   every operation as in JS: `join` writes nothing for it, `JSON.stringify` `null`,
//!   `Object.entries`, `map` and `flatMap` skip it.
//!
//! An object's own properties are a [`JsObject`]: array-index keys ascending, then the other
//! keys in insertion order, held in two maps so that no insertion shifts the others. Keys are
//! Rust strings: every key upstream can make from a document or a config read from JSON is well
//! formed (the index keys a spread makes are digits).
//!
//! The JS operations upstream's code applies to values are here, each with JS's semantics:
//! `typeof` ([`type_of`]), `ToBoolean` ([`Value::to_boolean`]), `ToPrimitive` and `ToString`
//! ([`to_primitive`], [`Value::to_js_string`]), `ToNumber` ([`to_number`]), `ToPropertyKey`
//! ([`to_property_key`]), `===` and SameValueZero ([`strict_equals`], [`same_value_zero`]),
//! `JSON.stringify` ([`json_stringify`], `toJSON` looked up through the chain),
//! `Array.prototype.join` ([`array_join`]), property reads ([`get`], [`has_property`],
//! [`has_own_property`]), `Object.entries` / spread ([`own_entries`]), `Array.isArray`
//! ([`is_array`]), `ArraySpeciesCreate` ([`species_check`]) and filling a new object by
//! assignment ([`PlainObject`]).
//!
//! A value keeps what JSON leaves out, because upstream's code reads it: an object member that
//! is `undefined` stays a member (`String({toString: undefined})` throws, as the HTML renderer
//! finds when it prints an attribute). It is dropped only on the way to JSON
//! ([`Value::to_scalar`], [`Value::to_render`], [`json_stringify`]).
//!
//! # Strings
//!
//! A string value is a [`JsString`]: text while it is well formed, its UTF-16 code units when
//! it holds a lone surrogate, which JS string operations on valid text make: a variable path
//! into a string (`$s[0]` of "😀" is `"\ud83d"`) and a partial's `variables` given a string
//! (spread one code unit per key). Equality ([`strict_equals`], `equals()`), `length`,
//! indexing, `ToString`, `JSON.stringify` (`debug($s[0])` writes the six characters
//! `"\ud83d"`), truthiness, type checks, RegExp input and the HTML renderer work on code
//! units, as in JS: two different halves are different strings, a half is not U+FFFD, and two
//! halves that meet in the output (adjacent text) are the character they make. A lone
//! surrogate becomes U+FFFD only where a value leaves as a Rust `String`: the HTML output,
//! error messages, the JSON render tree ([`Value::to_render`]) and [`JsString`]'s
//! `to_string_lossy`. Node writes the same bytes for one when it encodes UTF-8, so that
//! boundary is the one place the port's text differs from node's strings.
//!
//! # Depth and size
//!
//! A value can nest any depth (a config variable built in Rust or read by
//! [`Value::from_json`]). Building, dropping and converting one never recurses on Rust's
//! stack. Where upstream recurses ([`Value::to_js_string`]'s `Array.prototype.join`,
//! [`json_stringify`], the HTML renderer), V8's stack bounds the depth and node throws
//! `RangeError: Maximum call stack size exceeded`; the port's traversals are loops that charge
//! their path on the depth budget ([`crate::depth`]) as node's frames take its stack, so that
//! from the top of a thread they stop at node 22's limits ([`MAX_JOIN_DEPTH`],
//! [`MAX_JSON_DEPTH`], [`crate::next::renderers::html::MAX_RENDER_DEPTH`]) and deeper in a
//! document (a `debug()` in nested tags, an attribute rendered deep in a tree) earlier, as
//! node's do, and return that error past it. A string
//! longer than V8's limit ([`MAX_STRING_LENGTH`] code units) is V8's `RangeError: Invalid
//! string length` where `join` or `JSON.stringify` would make one (a sparse array 2^32 - 1
//! long is joined or written in the time its present elements take, as its holes are counted,
//! not visited).
//!
//! # Divergences from upstream
//!
//! - Values are immutable: a built-in a document's object holds under `toString`, `valueOf` or
//!   `toJSON` that sets properties of `this` (`Array.prototype.push`, ...) returns what V8's
//!   does, but a later read of the same object does not see what it set (see the
//!   divergences of the realm's calls, `next/realm/call.rs`, with its other ones: a
//!   `Function` body, locale data). [`Value::to_scalar`] and [`Value::to_render`] write a built-in as
//!   `JSON.stringify` does but call no `toJSON` ([`json_stringify`] does).
//! - UTF-16 at the Rust boundary: a lone surrogate leaving as Rust text is U+FFFD (see
//!   "Strings"); node's string holds the code unit. A key with a lone surrogate (only a value
//!   built in Rust can make one) is not modeled: keys are Rust strings.
//! - Call stack depth: node's limits depend on how much stack its caller already uses (a frame
//!   more or less moves them by one) and, for code V8 optimizes, on whether it has. The
//!   limits here are node 22's at the top level of a script, where `String()` converts 3,302
//!   nested arrays and `JSON.stringify` writes 4,170 nested arrays or objects, fresh or warmed
//!   up (both are builtins), less what the calls below charge the depth budget (see
//!   "Depth"): an attribute's `String()` in the HTML renderer converts 3,301 nested arrays
//!   (node 3,301) and `{% debug($v) %}` 4,164 (node 4,163, 4,164 warmed up), 4,085 inside 40
//!   tags (node 4,084, 4,147 warmed up). A caller's own frames (a test harness's, a Rust
//!   program's) are not node's and do not count.
//! - [`Value::to_render`] converts as deep as `JSON.stringify` writes ([`MAX_JSON_DEPTH`])
//!   where the stack grows ([`crate::depth::can_grow`]); where it cannot, it stops at
//!   [`MAX_RENDER_TREE_DEPTH`] (735) nested arrays and objects: the render tree it returns is
//!   recursive data (`Scalar`, `Tag`, and its `serde_json::Value` JSON), whose `Clone` and
//!   `PartialEq` recurse, and must fit a 2 MiB stack in a debug build there.
//!   [`json_stringify`] of the same tree is upstream's `JSON.stringify(tree)`, with node's
//!   limit.
//! - Rust-only: a [`TagValue`]'s `attributes` and `children` are held in it, not as values of
//!   their own, so reading them from a tag (`$t.attributes`, only a variable a Rust caller
//!   built can hold a tag) gives a new object each read where JavaScript gives the same one
//!   (`===` between two reads is false here). A [`crate::next::schema::Variables::Fn`] (a Rust
//!   closure) gets a path as [`crate::ast::PathSegment`]s: one a document replaced (`.path`)
//!   or a hand-made token gave is converted element by element.
//! - The JSON converters ([`Value::to_scalar`], [`Value::to_render`], [`to_json_value`] and
//!   `Scalar::to_json`) hold one JSON value per hole of a sparse array (32 bytes) where node's
//!   `JSON.stringify` writes five characters: they take V8's string limit as node does
//!   (`RangeError: Invalid string length` past 2^29 - 24 code units, so at most about 107
//!   million holes), with that constant more memory. [`json_stringify`] writes the text, as
//!   node does.

mod array;
mod json_text;
mod ops;
pub mod realm;
mod string;

pub use array::{Array, MAX_ARRAY_LENGTH, Present};
pub use ops::*;
pub use realm::{BuiltinProto, Ctor, Intrinsic, Method};

pub use crate::next::realm::Builtin;
pub use string::{CodeUnits, JsString, JsStringBuilder};

use crate::markdown_it::js;
use crate::types::{JsObject, RenderableTreeNode, Scalar, Tag};
use indexmap::IndexMap;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// `RangeError: Maximum call stack size exceeded`: what node throws where its recursion runs
/// out of stack, and the port past its fixed limit.
pub const RANGE_ERROR: &str = "RangeError: Maximum call stack size exceeded";

/// `RangeError: Invalid string length`: what V8 throws making a string longer than
/// [`MAX_STRING_LENGTH`].
pub const INVALID_STRING_LENGTH: &str = "RangeError: Invalid string length";

/// V8's longest string (`String::kMaxLength` on 64-bit, node 22): 2^29 - 24 code units.
pub const MAX_STRING_LENGTH: usize = (1 << 29) - 24;

/// The nesting of arrays `String()` (`Array.prototype.join`) converts from the top of a thread
/// (an empty one at the bottom counts): node 22 converts 3,302 nested arrays called from the
/// top level of a script and throws a `RangeError` for 3,303, fresh or warmed up (`join` is a
/// builtin); one frame deeper, 3,301. Each array costs the depth budget `JOIN_COST`.
pub const MAX_JOIN_DEPTH: usize = 3302;

/// What an array being joined costs on the depth budget ([`crate::depth`]).
pub(crate) const JOIN_COST: u64 = crate::depth::per_level(MAX_JOIN_DEPTH as u64);

/// The nesting of arrays and objects `JSON.stringify` writes from the top of a thread (an
/// empty one at the bottom counts; a tag is an object around its `children` array, two
/// levels): node 22 writes 4,170 nested arrays or objects around a string called from the top
/// level of a script and throws a `RangeError` for 4,171 (fresh or warmed up: the serializer is
/// native code); one frame deeper, 4,169. Each costs the depth budget `JSON_COST`.
pub const MAX_JSON_DEPTH: usize = 4170;

/// What an array or object being written costs on the depth budget ([`crate::depth`]).
pub(crate) const JSON_COST: u64 = crate::depth::per_level(MAX_JSON_DEPTH as u64);

/// The nesting of arrays and objects [`Value::to_render`] converts (counted as
/// [`MAX_JSON_DEPTH`] counts them) where the stack cannot grow ([`crate::depth::can_grow`]).
/// The render tree it returns is recursive data: `Scalar`s and `Tag`s, whose `Clone` and
/// `PartialEq` recurse, and their JSON (`RenderableTreeNode::to_json`, a `serde_json::Value`,
/// whose `Clone` recurses). On a 2 MiB thread a debug build clones 981 nested objects as
/// `Scalar`s (1,611 arrays, 1,359 tags) and 1,028 as a `serde_json::Value`; this keeps a
/// quarter of that stack spare. Where the stack grows, `Scalar` and `Tag` recurse on a stack
/// with room for each level, and the limit is [`MAX_JSON_DEPTH`].
pub const MAX_RENDER_TREE_DEPTH: usize = 735;

/// [`MAX_RENDER_TREE_DEPTH`] or [`MAX_JSON_DEPTH`], as the stack can grow on this thread.
fn render_tree_limit() -> usize {
    if crate::depth::can_grow() {
        MAX_JSON_DEPTH
    } else {
        MAX_RENDER_TREE_DEPTH
    }
}

/// A JavaScript value, as upstream's resolved attributes and render values hold it.
#[derive(Clone)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    /// A string, as JS holds it (see the module's "Strings").
    String(JsString),
    /// An array (see [`Array`]); its identity is the `Arc`'s.
    Array(Arc<Array>),
    /// An ordinary object: a plain object, an object with another prototype, or an instance of
    /// upstream's AST classes (see the module's "Objects"); its identity is the `Arc`'s.
    Object(Arc<Object>),
    /// A render-tree tag (upstream's `Tag` instance).
    Tag(Arc<TagValue>),
    /// A built-in object of the realm (a function, a prototype, a constructor, Markdoc's
    /// classes): its identity is the [`Builtin`] (see [`crate::next::realm`]).
    Builtin(Builtin),
}

/// An object's prototype (`[[Prototype]]`).
#[derive(Clone)]
pub enum Proto {
    /// A built-in object of the realm: `Object.prototype` for a plain object,
    /// `Variable.prototype` for a variable, any other a document set (`$x.__proto__`).
    Builtin(Builtin),
    /// `null`.
    Null,
    /// A document's object (an object, an array or a tag).
    Value(Value),
}

impl Proto {
    /// `Object.prototype`.
    pub const OBJECT: Proto = Proto::Builtin(Builtin::OBJECT_PROTOTYPE);

    /// Is it `Object.prototype` (`Object.getPrototypeOf(value) === Object.prototype`)?
    pub fn is_object_prototype(&self) -> bool {
        matches!(self, Proto::Builtin(Builtin::OBJECT_PROTOTYPE))
    }

    /// `proto` as a prototype: an object (an array, an object or a tag) or `null`; anything
    /// else is no prototype (`Object.prototype.__proto__`'s setter ignores it).
    pub fn from_value(proto: &Value) -> Option<Proto> {
        match proto {
            Value::Null => Some(Proto::Null),
            Value::Array(_) | Value::Object(_) | Value::Tag(_) => Some(Proto::Value(proto.clone())),
            Value::Builtin(b) => Some(Proto::Builtin(*b)),
            _ => None,
        }
    }
}

/// An ordinary object: its own properties (a [`JsObject`], which it dereferences to) and its
/// prototype. Dropping one never recurses, however deep the values nest.
#[derive(Clone)]
pub struct Object {
    props: JsObject<Value>,
    proto: Proto,
}

impl Default for Object {
    fn default() -> Self {
        Object::new(JsObject::new())
    }
}

/// Upstream's `Tag` (`src/tag.ts`) as a value: `name`, `attributes`, `children`, the own
/// properties `new Tag(name, attributes, children)` gives it (with `$$mdtype` first), and
/// `Tag.prototype`. The name is any value (a schema's `render`, which is normally a string);
/// the attributes are the object `transformer.attributes` made (whose prototype a
/// `__proto__` render name sets). Dropping one never recurses, however deep the values nest.
#[derive(Clone)]
pub struct TagValue {
    pub name: Value,
    /// In a JS object's order.
    pub attributes: Object,
    pub children: Vec<Value>,
}

impl Object {
    /// A plain object (`Object.prototype`) with these own properties.
    pub fn new(properties: JsObject<Value>) -> Self {
        Object {
            props: properties,
            proto: Proto::OBJECT,
        }
    }

    /// An object with these own properties and this prototype.
    pub fn with_proto(properties: JsObject<Value>, proto: Proto) -> Self {
        Object {
            props: properties,
            proto,
        }
    }

    /// The prototype.
    pub fn proto(&self) -> &Proto {
        &self.proto
    }

    /// Replaces the prototype.
    pub fn set_proto(&mut self, proto: Proto) {
        self.proto = proto;
    }

    /// The own properties.
    pub fn into_inner(mut self) -> JsObject<Value> {
        std::mem::take(&mut self.props)
    }

    /// The own properties and the prototype.
    pub fn into_parts(mut self) -> (JsObject<Value>, Proto) {
        let proto = std::mem::replace(&mut self.proto, Proto::Null);
        (std::mem::take(&mut self.props), proto)
    }
}

impl Deref for Object {
    type Target = JsObject<Value>;
    fn deref(&self) -> &JsObject<Value> {
        &self.props
    }
}

impl DerefMut for Object {
    fn deref_mut(&mut self) -> &mut JsObject<Value> {
        &mut self.props
    }
}

impl From<JsObject<Value>> for Object {
    fn from(properties: JsObject<Value>) -> Self {
        Object::new(properties)
    }
}

impl From<IndexMap<String, Value>> for Object {
    fn from(properties: IndexMap<String, Value>) -> Self {
        Object::new(properties.into())
    }
}

impl<K: Into<String>, const N: usize> From<[(K, Value); N]> for Object {
    fn from(entries: [(K, Value); N]) -> Self {
        Object::new(entries.into())
    }
}

impl From<Object> for JsObject<Value> {
    fn from(object: Object) -> Self {
        object.into_inner()
    }
}

/// Drops values without recursing: the contents of an array, object or tag this was the last
/// reference to go onto the list instead of being dropped inside it.
fn drop_values(mut pending: Vec<Value>) {
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => {
                if let Some(mut items) = Arc::into_inner(items) {
                    pending.append(&mut items.take_values());
                }
            }
            Value::Object(object) => {
                if let Some(mut object) = Arc::into_inner(object) {
                    pending.extend(std::mem::take(&mut object.props).into_values());
                    if let Proto::Value(p) = std::mem::replace(&mut object.proto, Proto::Null) {
                        pending.push(p);
                    }
                }
            }
            Value::Tag(tag) => {
                if let Some(mut tag) = Arc::into_inner(tag) {
                    pending.append(&mut tag.children);
                    pending.extend(std::mem::take(&mut tag.attributes.props).into_values());
                    if let Proto::Value(p) =
                        std::mem::replace(&mut tag.attributes.proto, Proto::Null)
                    {
                        pending.push(p);
                    }
                    pending.push(std::mem::replace(&mut tag.name, Value::Undefined));
                }
            }
            _ => {}
        }
    }
}

impl Drop for Array {
    fn drop(&mut self) {
        let values = self.take_values();
        if !values.is_empty() {
            drop_values(values);
        }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        let mut values: Vec<Value> = Vec::new();
        if !self.props.is_empty() {
            values.extend(std::mem::take(&mut self.props).into_values());
        }
        if let Proto::Value(p) = std::mem::replace(&mut self.proto, Proto::Null) {
            values.push(p);
        }
        if !values.is_empty() {
            drop_values(values);
        }
    }
}

impl Drop for TagValue {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.children);
        pending.push(std::mem::replace(&mut self.name, Value::Undefined));
        pending.extend(std::mem::take(&mut self.attributes.props).into_values());
        if let Proto::Value(p) = std::mem::replace(&mut self.attributes.proto, Proto::Null) {
            pending.push(p);
        }
        drop_values(pending);
    }
}

impl TagValue {
    /// `Tag.isTag(value)` (`src/tag.ts`): `value?.$$mdtype === 'Tag'`, read through the value's
    /// prototype chain (a tag, an object that says it is one, or one that inherits that).
    pub fn is_tag(value: &Value) -> bool {
        match value {
            Value::Tag(_) => true,
            Value::Undefined | Value::Null => false,
            _ => matches!(get_value(value, "$$mdtype"), Ok(Value::String(t)) if t == "Tag"),
        }
    }

    /// `new Tag(name, attributes, children)`.
    pub fn new(
        name: impl Into<Value>,
        attributes: impl Into<Object>,
        children: Vec<Value>,
    ) -> Self {
        Self {
            name: name.into(),
            attributes: attributes.into(),
            children,
        }
    }

    /// The name, when it is a string (as every tag the default schemas make has).
    pub fn name_str(&self) -> Option<&JsString> {
        match &self.name {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

/// How deep [`fmt::Debug`] prints a value before it writes `…` (a value can nest deeper than
/// a recursive formatter's stack allows).
const DEBUG_DEPTH: usize = 64;

/// A value with the depth it is printed at.
struct Nested<'a>(&'a Value, usize);

impl fmt::Debug for Nested<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Nested(value, depth) = *self;
        if depth >= DEBUG_DEPTH
            && matches!(value, Value::Array(_) | Value::Object(_) | Value::Tag(_))
        {
            return f.write_str("…");
        }
        match value {
            Value::Undefined => f.write_str("Undefined"),
            Value::Null => f.write_str("Null"),
            Value::Boolean(b) => f.debug_tuple("Boolean").field(b).finish(),
            Value::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Value::String(s) => f.debug_tuple("String").field(s).finish(),
            Value::Array(items) => f
                .debug_tuple("Array")
                .field(&NestedArray(items, depth + 1))
                .finish(),
            Value::Object(o) => {
                let mut t = f.debug_tuple("Object");
                t.field(&NestedMap(o, depth + 1));
                if !o.proto.is_object_prototype() {
                    t.field(&NestedProto(&o.proto, depth + 1));
                }
                t.finish()
            }
            Value::Tag(tag) => f
                .debug_struct("Tag")
                .field("name", &Nested(&tag.name, depth + 1))
                .field("attributes", &NestedMap(&tag.attributes, depth + 1))
                .field("children", &NestedList(&tag.children, depth + 1))
                .finish(),
            Value::Builtin(b) => f.debug_tuple("Builtin").field(b).finish(),
        }
    }
}

struct NestedProto<'a>(&'a Proto, usize);

impl fmt::Debug for NestedProto<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Proto::Builtin(p) => write!(f, "__proto__: {p:?}"),
            Proto::Null => f.write_str("__proto__: null"),
            Proto::Value(v) => write!(f, "__proto__: {:?}", Nested(v, self.1)),
        }
    }
}

struct NestedList<'a>(&'a [Value], usize);

impl fmt::Debug for NestedList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|v| Nested(v, self.1)))
            .finish()
    }
}

struct NestedArray<'a>(&'a Array, usize);

impl fmt::Debug for NestedArray<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let array = self.0;
        if let (Some(items), false) = (array.as_dense(), array.has_named()) {
            return NestedList(items, self.1).fmt(f);
        }
        let mut m = f.debug_map();
        m.entry(&"length", &array.len());
        for (i, v) in array.present() {
            m.entry(&i, &Nested(v, self.1));
        }
        for (k, v) in array.named() {
            m.entry(k, &Nested(v, self.1));
        }
        m.finish()
    }
}

struct NestedMap<'a>(&'a JsObject<Value>, usize);

impl fmt::Debug for NestedMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(k, v)| (k, Nested(v, self.1))))
            .finish()
    }
}

/// As a derived `Debug` would print it, down to a depth of 64 (`…` below).
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Nested(self, 0).fmt(f)
    }
}

impl fmt::Debug for Array {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        NestedArray(self, 1).fmt(f)
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        NestedMap(&self.props, 1).fmt(f)
    }
}

impl fmt::Debug for TagValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TagValue")
            .field("name", &Nested(&self.name, 1))
            .field("attributes", &NestedMap(&self.attributes, 1))
            .field("children", &NestedList(&self.children, 1))
            .finish()
    }
}

impl fmt::Debug for Proto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        NestedProto(self, 1).fmt(f)
    }
}

/// The names `Object.prototype` holds (node 22): what every plain object (and every map
/// upstream keeps in one: `config.tags`, `config.functions`, a schema's `attributes`, ...)
/// inherits. `tags['toString']` is a function, so a tag named `toString` has a schema.
pub const OBJECT_PROTOTYPE: [&str; 12] = realm::OBJECT_PROTOTYPE;

/// Does a plain object inherit `key` from `Object.prototype`?
pub fn is_object_prototype_name(key: &str) -> bool {
    Builtin::OBJECT_PROTOTYPE.own(key).is_some()
}

impl Value {
    /// A new array (a new identity).
    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Arc::new(Array::new(items)))
    }

    /// A new array of `length` with the elements `present` (holes between them) and the other
    /// own properties `named`.
    pub fn sparse_array(
        length: u32,
        present: impl IntoIterator<Item = (u32, Value)>,
        named: IndexMap<String, Value>,
    ) -> Value {
        Value::Array(Arc::new(Array::from_parts(length, present, named)))
    }

    /// A new plain object (a new identity), from its properties (a [`JsObject`], or an
    /// `IndexMap`, whose entries are set in its order).
    pub fn object(properties: impl Into<JsObject<Value>>) -> Value {
        Value::Object(Arc::new(Object::new(properties.into())))
    }

    /// A new object with this prototype.
    pub fn object_with_proto(properties: impl Into<JsObject<Value>>, proto: Proto) -> Value {
        Value::Object(Arc::new(Object::with_proto(properties.into(), proto)))
    }

    /// A new, empty plain object.
    pub fn empty_object() -> Value {
        Value::object(JsObject::new())
    }

    /// A string value.
    pub fn string(s: impl Into<JsString>) -> Value {
        Value::String(s.into())
    }

    /// `new Variable(path)`: `{$$mdtype: 'Variable', path}` with `Variable.prototype`.
    pub fn variable(path: Value) -> Value {
        let props: JsObject<Value> =
            [("$$mdtype", Value::string("Variable")), ("path", path)].into();
        Value::object_with_proto(props, Proto::Builtin(Builtin::VARIABLE_PROTOTYPE))
    }

    /// `new Function(name, parameters)` (upstream's AST class): `{$$mdtype: 'Function', name,
    /// parameters}` with `Function.prototype`.
    pub fn ast_function(name: Value, parameters: Value) -> Value {
        let props: JsObject<Value> = [
            ("$$mdtype", Value::string("Function")),
            ("name", name),
            ("parameters", parameters),
        ]
        .into();
        Value::object_with_proto(props, Proto::Builtin(Builtin::AST_FUNCTION_PROTOTYPE))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    /// `value == null`: `null` or `undefined`.
    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }

    /// Is it an object (`typeof value === 'object' && value !== null`, or a function)?
    pub fn is_object(&self) -> bool {
        matches!(
            self,
            Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_)
        )
    }

    /// JS `ToBoolean` (`!!value`): `undefined`, `null`, `false`, `0`, `-0`, `NaN` and `''`
    /// are false; every object is true.
    pub fn to_boolean(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Boolean(b) => *b,
            Value::Number(n) => !(*n == 0.0 || n.is_nan()),
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Object(_) | Value::Tag(_) | Value::Builtin(_) => true,
        }
    }

    /// `typeof value`: `'function'` for a built-in function, `'object'` for `null` and every
    /// other object.
    pub fn type_of(&self) -> &'static str {
        type_of(self)
    }

    /// `String(value)` (ECMAScript `ToString`): numbers as JS prints them, an object by
    /// `ToPrimitive` with the hint `string` (its `toString`, else its `valueOf`, found through
    /// its prototype chain: an array's is `join`, a plain object's `[object Object]`). `Err`
    /// where JS throws: no callable `toString` or `valueOf` gives a primitive (`TypeError:
    /// Cannot convert object to primitive value`), arrays nested deeper than
    /// [`MAX_JOIN_DEPTH`] (a `RangeError`), a string past [`MAX_STRING_LENGTH`].
    pub fn to_js_string(&self) -> Result<JsString, String> {
        to_js_string(self)
    }

    /// The string, when the value is a string that is well formed (see
    /// [`Value::as_js_str`] for any string: `typeof value === 'string'`).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => s.as_str(),
            _ => None,
        }
    }

    /// `typeof value === 'string'`: the string.
    pub fn as_js_str(&self) -> Option<&JsString> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// A value from JSON, as `JSON.parse` makes it: objects keep their keys in a JS object's
    /// order, and a `__proto__` key is an own property like any other. Objects with a
    /// `$$mdtype` stay plain objects, as `JSON.parse` leaves them. Any depth (a loop).
    pub fn from_json(json: &serde_json::Value) -> Value {
        Value::from_json_with(json, &|_| None)
    }

    /// [`Value::from_json`], where `revive` may stand a value of its own for an object (as a
    /// `JSON.parse` reviver does; the harness's `{"$type": name}`), which is then not read
    /// further. Any depth (a loop).
    pub fn from_json_with(
        json: &serde_json::Value,
        revive: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Option<Value>,
    ) -> Value {
        enum Frame<'a> {
            Array(std::slice::Iter<'a, serde_json::Value>, Vec<Value>),
            Object(
                serde_json::map::Iter<'a>,
                JsObject<Value>,
                Option<&'a String>,
            ),
        }
        let leaf = |json: &serde_json::Value| -> Option<Value> {
            Some(match json {
                serde_json::Value::Null => Value::Null,
                serde_json::Value::Bool(b) => Value::Boolean(*b),
                serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
                serde_json::Value::String(s) => Value::string(s.as_str()),
                serde_json::Value::Object(o) => return revive(o),
                serde_json::Value::Array(_) => return None,
            })
        };
        fn open(json: &serde_json::Value) -> Frame<'_> {
            match json {
                serde_json::Value::Array(items) => {
                    Frame::Array(items.iter(), Vec::with_capacity(items.len()))
                }
                serde_json::Value::Object(o) => Frame::Object(o.iter(), JsObject::new(), None),
                _ => unreachable!("a leaf"),
            }
        }
        if let Some(value) = leaf(json) {
            return value;
        }
        let mut stack = vec![open(json)];
        loop {
            let top = stack.last_mut().expect("a frame");
            let next = match top {
                Frame::Array(items, _) => items.next(),
                Frame::Object(entries, _, key) => entries.next().map(|(k, v)| {
                    *key = Some(k);
                    v
                }),
            };
            let done = match next {
                Some(json) => match leaf(json) {
                    Some(value) => value,
                    None => {
                        stack.push(open(json));
                        continue;
                    }
                },
                None => match stack.pop().expect("a frame") {
                    Frame::Array(_, items) => Value::array(items),
                    Frame::Object(_, object, _) => Value::object(object),
                },
            };
            // `done` is the next item of the frame on top (or, with none left, the result).
            match stack.last_mut() {
                None => return done,
                Some(Frame::Array(_, items)) => items.push(done),
                Some(Frame::Object(_, object, key)) => {
                    object.insert(key.take().expect("the key").clone(), done);
                }
            }
        }
    }

    /// An AST value as the JS value it is, without resolving it: a `Variable` or `Function` is
    /// an instance of upstream's class (its own properties, its class's prototype), an object
    /// a plain object, a value the AST holds as [`Scalar::Js`] that value. Any depth (a loop).
    pub fn from_scalar(scalar: &Scalar) -> Value {
        from_tree(Tree::Scalar(scalar))
    }

    /// The value as JSON keeps it (what `JSON.stringify` writes, as a [`Scalar`]): an object
    /// member that is `undefined` is left out, an `undefined` elsewhere (a hole too) is `null`,
    /// a string's lone surrogate is U+FFFD, an object's own enumerable properties alone are
    /// kept (no prototype, no extra property of an array). `Err` where `JSON.stringify` throws
    /// (see [`json_stringify`]) and for arrays and objects nested deeper than
    /// [`MAX_RENDER_TREE_DEPTH`] (a `RangeError`). A tag whose name is not a string is the
    /// object its JSON is (`{"$$mdtype": "Tag", "name": ..., ...}`: the render tree's tags have
    /// string names).
    pub fn to_scalar(&self) -> Result<Scalar, String> {
        to_scalar(self)
    }

    /// The value as a render-tree node: a tag is a `RenderableTreeNode::Tag`, any other value
    /// a `RenderableTreeNode::Scalar` (see [`Value::to_scalar`]). This is the tree's JSON:
    /// what `JSON.stringify` leaves out (an object member that is `undefined`) is gone, so
    /// render the value itself, not this, where upstream renders the tree (a `String()` of an
    /// object sees its members; see [`crate::next::renderers::html::render`]). `Err` (a
    /// `RangeError`) past [`MAX_RENDER_TREE_DEPTH`]; [`json_stringify`] writes the same JSON
    /// as text, with `JSON.stringify`'s own limit.
    pub fn to_render(&self) -> Result<RenderableTreeNode, String> {
        self.to_scalar().map(RenderableTreeNode::from)
    }

    /// A render-tree node (a tree read from JSON) as a value: a tag is a tag, any other node
    /// its value (see [`Value::from_scalar`]). Any depth (a loop).
    pub fn from_render(node: &RenderableTreeNode) -> Value {
        from_tree(Tree::Node(node))
    }
}

/// An AST value, a render-tree node or a tag, to convert into a [`Value`].
#[derive(Clone, Copy)]
enum Tree<'a> {
    Scalar(&'a Scalar),
    Node(&'a RenderableTreeNode),
    Tag(&'a Tag),
}

/// [`Value::from_scalar`] and [`Value::from_render`]: a loop over a stack of half-built
/// arrays, objects and tags (a tag's attributes, then its children).
fn from_tree(root: Tree<'_>) -> Value {
    /// What a finished frame becomes.
    enum Kind {
        Array,
        Object,
        /// A function: its parameters object becomes `parameters`, with this name.
        Function(String),
    }
    enum Frame<'a> {
        Array(std::slice::Iter<'a, Scalar>, Vec<Value>, Kind),
        Object(
            indexmap::map::Iter<'a, String, Scalar>,
            JsObject<Value>,
            Option<&'a String>,
            Kind,
        ),
        Tag {
            attributes: indexmap::map::Iter<'a, String, Scalar>,
            key: Option<&'a String>,
            children: std::slice::Iter<'a, RenderableTreeNode>,
            built: TagValue,
        },
    }
    /// A value that holds no other, or the frame of an array, object or tag to fill.
    enum Step<'a> {
        Leaf(Value),
        Open(Frame<'a>),
    }
    fn path_scalar(step: &crate::ast::PathSegment) -> Value {
        match step {
            crate::ast::PathSegment::String(s) => Value::string(s.as_str()),
            crate::ast::PathSegment::Number(n) => Value::Number(*n),
        }
    }
    fn leaf(tree: Tree<'_>) -> Step<'_> {
        let scalar = match tree {
            Tree::Scalar(scalar) => scalar,
            Tree::Node(RenderableTreeNode::Scalar(scalar)) => scalar,
            Tree::Node(RenderableTreeNode::Tag(tag)) => return Step::Open(open_tag(tag)),
            Tree::Tag(tag) => return Step::Open(open_tag(tag)),
        };
        Step::Leaf(match scalar {
            Scalar::Undefined => Value::Undefined,
            Scalar::Null => Value::Null,
            Scalar::Boolean(b) => Value::Boolean(*b),
            Scalar::Number(n) => Value::Number(*n),
            Scalar::String(s) => Value::string(s.as_str()),
            Scalar::Variable(v) => {
                Value::variable(Value::array(v.path.iter().map(path_scalar).collect()))
            }
            Scalar::Js(js) => js.value().clone(),
            Scalar::Function(f) => {
                return Step::Open(Frame::Object(
                    f.parameters.iter(),
                    JsObject::new(),
                    None,
                    Kind::Function(f.name.clone()),
                ));
            }
            Scalar::Tag(tag) => return Step::Open(open_tag(tag)),
            Scalar::Array(items) => {
                return Step::Open(Frame::Array(
                    items.iter(),
                    Vec::with_capacity(items.len()),
                    Kind::Array,
                ));
            }
            Scalar::Object(o) => {
                return Step::Open(Frame::Object(o.iter(), JsObject::new(), None, Kind::Object));
            }
        })
    }
    fn open_tag(tag: &Tag) -> Frame<'_> {
        Frame::Tag {
            attributes: tag.attributes.iter(),
            key: None,
            children: tag.children.iter(),
            built: TagValue {
                name: Value::string(tag.name.as_str()),
                attributes: Object::default(),
                children: Vec::with_capacity(tag.children.len()),
            },
        }
    }
    fn finish(kind: Kind, value: Value) -> Value {
        match kind {
            Kind::Array | Kind::Object => value,
            Kind::Function(name) => Value::ast_function(Value::string(name), value),
        }
    }
    let mut stack = match leaf(root) {
        Step::Leaf(value) => return value,
        Step::Open(frame) => vec![frame],
    };
    loop {
        let top = stack.last_mut().expect("a frame");
        let next: Option<Tree<'_>> = match top {
            Frame::Array(items, _, _) => items.next().map(Tree::Scalar),
            Frame::Object(entries, _, key, _) => entries.next().map(|(k, v)| {
                *key = Some(k);
                Tree::Scalar(v)
            }),
            Frame::Tag {
                attributes,
                key,
                children,
                ..
            } => match attributes.next() {
                Some((k, v)) => {
                    *key = Some(k);
                    Some(Tree::Scalar(v))
                }
                None => {
                    *key = None;
                    children.next().map(Tree::Node)
                }
            },
        };
        let done = match next {
            Some(tree) => match leaf(tree) {
                Step::Leaf(value) => value,
                Step::Open(frame) => {
                    stack.push(frame);
                    continue;
                }
            },
            None => match stack.pop().expect("a frame") {
                Frame::Array(_, items, kind) => finish(kind, Value::array(items)),
                Frame::Object(_, object, _, kind) => finish(kind, Value::object(object)),
                Frame::Tag { built, .. } => Value::Tag(Arc::new(built)),
            },
        };
        match stack.last_mut() {
            None => return done,
            Some(Frame::Array(_, items, _)) => items.push(done),
            Some(Frame::Object(_, object, key, _)) => {
                object.insert(key.take().expect("the key").clone(), done);
            }
            Some(Frame::Tag { key, built, .. }) => match key.take() {
                Some(k) => {
                    built.attributes.insert(k.clone(), done);
                }
                None => built.children.push(done),
            },
        }
    }
}

/// A new object (`{}`) filled by assignments, `object[key] = value`, as upstream fills the
/// objects it builds that way (`transformer.attributes`' output, `resolve`'s copy of an
/// object): JavaScript's `[[Set]]` on an object nothing else holds yet (see [`set`]).
///
/// An assignment to `__proto__` is not a key while the object inherits `Object.prototype`'s
/// accessor: its setter makes an object or `null` the object's prototype and ignores anything
/// else. Once the chain has no accessor (a `null` prototype, or an own `__proto__` property of
/// an object on it), `__proto__` is assigned as a key like any other. (A spread, a computed
/// key in an object literal, `Object.fromEntries` and `Object.defineProperty` make a key
/// `__proto__` like any other; those sites use [`JsObject::insert`].)
#[derive(Debug, Default)]
pub struct PlainObject {
    object: Object,
}

impl PlainObject {
    /// `{}`.
    pub fn new() -> Self {
        Self::default()
    }

    /// An object nothing holds yet with this prototype (`new Node()`'s, before `Object.assign`
    /// fills it).
    pub fn with_proto(proto: Proto) -> Self {
        PlainObject {
            object: Object::with_proto(JsObject::new(), proto),
        }
    }

    /// `object[key] = value`.
    pub fn assign(&mut self, key: impl Into<String>, value: Value) {
        set(&mut self.object, key.into(), value);
    }

    /// The object's own entries, in a JS object's order (its prototype left behind).
    pub fn into_entries(self) -> IndexMap<String, Value> {
        self.object.into_inner().into()
    }

    /// The object's own properties (its prototype left behind).
    pub fn into_properties(self) -> JsObject<Value> {
        self.object.into_inner()
    }

    /// The object, with its prototype.
    pub fn into_object(self) -> Object {
        self.object
    }
}

impl TagValue {
    /// The render tree's [`Tag`]: attributes that are `undefined` left out, as
    /// `JSON.stringify` leaves them out (see [`Value::to_scalar`]).
    pub fn to_tag(&self) -> Result<Tag, String> {
        match &mut Value::Tag(Arc::new(self.clone())).to_scalar()? {
            Scalar::Tag(tag) => Ok(std::mem::take(tag.as_mut())),
            _ => unreachable!("a tag converts to a tag"),
        }
    }

    /// A render tree's [`Tag`] as a value. Any depth (a loop).
    pub fn from_tag(tag: &Tag) -> TagValue {
        match from_tree(Tree::Tag(tag)) {
            Value::Tag(tag) => Arc::into_inner(tag).expect("a tag nothing else holds yet"),
            _ => unreachable!("a tag converts to a tag"),
        }
    }
}

impl From<Scalar> for Value {
    fn from(scalar: Scalar) -> Self {
        Value::from_scalar(&scalar)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.into())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s.into())
    }
}

impl From<JsString> for Value {
    fn from(s: JsString) -> Self {
        Value::String(s)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Boolean(b)
    }
}

impl From<TagValue> for Value {
    fn from(tag: TagValue) -> Self {
        Value::Tag(Arc::new(tag))
    }
}

/// `Value::to_scalar`, a loop: arrays, objects and tags are converted bottom-up from a stack
/// of half-built containers, counted as `JSON.stringify` nests them. A hole is `null`; an
/// array longer than [`SPARSE_CHECK`] (its text at least that long) is first written by
/// [`json_stringify`], which throws V8's `RangeError: Invalid string length` exactly where
/// node's would.
fn to_scalar(value: &Value) -> Result<Scalar, String> {
    enum Frame<'a> {
        Array {
            items: &'a Array,
            next: usize,
            out: Vec<Scalar>,
        },
        Object {
            entries: crate::types::JsObjectIter<'a, Value>,
            key: Option<&'a String>,
            out: IndexMap<String, Scalar>,
        },
        /// A tag: its attributes (as an object frame would), then its children; its name
        /// already converted when it is not a string.
        Tag {
            tag: &'a TagValue,
            name: Option<Scalar>,
            attributes: crate::types::JsObjectIter<'a, Value>,
            key: Option<&'a String>,
            out_attributes: IndexMap<String, Scalar>,
            next_child: usize,
            out_children: Vec<RenderableTreeNode>,
        },
    }
    fn leaf(value: &Value) -> Option<Scalar> {
        Some(match value {
            Value::Undefined | Value::Null => Scalar::Null,
            Value::Boolean(b) => Scalar::Boolean(*b),
            Value::Number(n) => Scalar::Number(*n),
            Value::String(s) => Scalar::String(s.to_string_lossy().into_owned()),
            Value::Array(_) | Value::Object(_) | Value::Tag(_) => return None,
            // As `JSON.stringify` writes a built-in: a wrapper prototype as its primitive, a
            // function as nothing (`null` in an array), `Array.prototype` as `[]`, any other
            // as `{}` (no prototype of the table has an enumerable property).
            Value::Builtin(b) => match b.data().wrapped {
                Some(crate::next::realm::Wrapped::String(s)) => Scalar::String(s.to_string()),
                Some(crate::next::realm::Wrapped::Number(n)) => Scalar::Number(n),
                Some(crate::next::realm::Wrapped::Boolean(v)) => Scalar::Boolean(v),
                None if b.is_callable() => Scalar::Null,
                None if b.is_array() => Scalar::Array(Vec::new()),
                None => Scalar::Object(IndexMap::new()),
            },
        })
    }
    fn open(value: &Value) -> Result<Frame<'_>, String> {
        Ok(match value {
            Value::Array(items) => Frame::Array {
                items,
                next: 0,
                out: Vec::with_capacity(items.len().min(SPARSE_CHECK)),
            },
            Value::Object(o) => Frame::Object {
                entries: o.iter(),
                key: None,
                out: IndexMap::with_capacity(o.len()),
            },
            Value::Tag(tag) => Frame::Tag {
                tag,
                // A name that is not a string (a schema's `render` given as another value):
                // converted on its own (it is the tag's first member).
                name: match &tag.name {
                    Value::String(_) => None,
                    other => Some(to_scalar(other)?),
                },
                attributes: tag.attributes.iter(),
                key: None,
                out_attributes: IndexMap::with_capacity(tag.attributes.len()),
                next_child: 0,
                out_children: Vec::with_capacity(tag.children.len()),
            },
            _ => unreachable!("a leaf"),
        })
    }
    // The JSON nesting a frame adds: one for an array or object; a tag is an object whose
    // attributes and children are one level further down.
    fn levels(frame: &Frame<'_>) -> usize {
        if matches!(frame, Frame::Tag { .. }) {
            2
        } else {
            1
        }
    }
    if let Some(scalar) = leaf(value) {
        return Ok(scalar);
    }
    let mut checked = false;
    let mut check = |items: &Array| -> Result<(), String> {
        if !checked && items.len() > SPARSE_CHECK {
            checked = true;
            json_stringify(value, "")?;
        }
        Ok(())
    };
    let first = open(value)?;
    if let Frame::Array { items, .. } = &first {
        check(items)?;
    }
    let mut nesting = levels(&first);
    if nesting > render_tree_limit() {
        return Err(RANGE_ERROR.into());
    }
    let mut stack = vec![first];
    loop {
        let top = stack.last_mut().expect("a frame");
        // The next child to convert (leaving the key it goes under in the frame), or `None`
        // when the frame is complete.
        let next: Option<&Value> = match top {
            Frame::Array { items, next, .. } => {
                if *next < items.len() {
                    let item = items.get(*next).unwrap_or(&Value::Undefined);
                    *next += 1;
                    Some(item)
                } else {
                    None
                }
            }
            Frame::Object { entries, key, .. } => loop {
                match entries.next() {
                    // A member that is `undefined` is left out.
                    Some((_, v)) if v.is_undefined() || is_callable(v) => continue,
                    Some((k, v)) => {
                        *key = Some(k);
                        break Some(v);
                    }
                    None => break None,
                }
            },
            Frame::Tag {
                tag,
                attributes,
                key,
                next_child,
                ..
            } => loop {
                match attributes.next() {
                    Some((_, v)) if v.is_undefined() || is_callable(v) => continue,
                    Some((k, v)) => {
                        *key = Some(k);
                        break Some(v);
                    }
                    None => {
                        *key = None;
                        let child = tag.children.get(*next_child);
                        *next_child += 1;
                        break child;
                    }
                }
            },
        };
        let done = match next {
            Some(child) => match leaf(child) {
                Some(scalar) => scalar,
                None => {
                    let frame = open(child)?;
                    if let Frame::Array { items, .. } = &frame {
                        check(items)?;
                    }
                    nesting += levels(&frame);
                    if nesting > render_tree_limit() {
                        return Err(RANGE_ERROR.into());
                    }
                    stack.push(frame);
                    continue;
                }
            },
            None => {
                let frame = stack.pop().expect("a frame");
                nesting -= levels(&frame);
                match frame {
                    Frame::Array { out, .. } => Scalar::Array(out),
                    Frame::Object { out, .. } => Scalar::Object(out),
                    Frame::Tag {
                        tag,
                        name,
                        out_attributes,
                        out_children,
                        ..
                    } => match (name, &tag.name) {
                        (None, Value::String(n)) => Scalar::Tag(Box::new(Tag {
                            name: n.to_string_lossy().into_owned(),
                            attributes: out_attributes,
                            children: out_children,
                        })),
                        // `{$$mdtype, name, attributes, children}`, a plain object's JSON.
                        (name, _) => {
                            let mut object = IndexMap::with_capacity(4);
                            object.insert("$$mdtype".to_string(), Scalar::String("Tag".into()));
                            if let Some(name) = name.filter(|_| !tag.name.is_undefined()) {
                                object.insert("name".to_string(), name);
                            }
                            object.insert("attributes".to_string(), Scalar::Object(out_attributes));
                            object.insert(
                                "children".to_string(),
                                Scalar::Array(
                                    out_children
                                        .into_iter()
                                        .map(|child| match child {
                                            RenderableTreeNode::Tag(tag) => Scalar::Tag(tag),
                                            RenderableTreeNode::Scalar(value) => value,
                                        })
                                        .collect(),
                                ),
                            );
                            Scalar::Object(object)
                        }
                    },
                }
            }
        };
        match stack.last_mut() {
            None => return Ok(done),
            Some(Frame::Array { out, .. }) => out.push(done),
            Some(Frame::Object { out, key, .. }) => {
                out.insert(key.take().expect("the key").clone(), done);
            }
            Some(Frame::Tag {
                key,
                out_attributes,
                out_children,
                ..
            }) => match key.take() {
                Some(k) => {
                    out_attributes.insert(k.clone(), done);
                }
                None => out_children.push(RenderableTreeNode::from(done)),
            },
        }
    }
}

/// `JSON.parse(JSON.stringify(value))` as a `serde_json::Value`, made without a [`Scalar`] in
/// between (a sparse array costs one JSON `null` per hole, not a `Scalar` and a JSON value):
/// what [`Value::to_scalar`] keeps, as JSON. `Err` as for [`Value::to_scalar`]; `Ok(None)` for
/// `undefined`. A loop, at any depth.
pub fn to_json_value(value: &Value) -> Result<Option<serde_json::Value>, String> {
    use serde_json::Value as J;
    enum Frame {
        Array {
            items: Arc<Array>,
            next: usize,
            out: Vec<J>,
        },
        /// An object's (or a tag's) own enumerable properties, as `JSON.stringify` writes
        /// them.
        Object {
            members: std::vec::IntoIter<(String, Value)>,
            key: Option<String>,
            out: serde_json::Map<String, J>,
            /// A tag's `attributes` and `children` sit one level further down.
            levels: usize,
        },
    }
    fn leaf(value: &Value) -> Option<Option<J>> {
        Some(Some(match value {
            Value::Undefined => return Some(None),
            Value::Null => J::Null,
            Value::Boolean(b) => J::Bool(*b),
            Value::Number(n) => crate::types::json_number(*n),
            Value::String(s) => J::String(s.to_string_lossy().into_owned()),
            Value::Array(_) | Value::Object(_) | Value::Tag(_) => return None,
            // As `JSON.stringify` writes a built-in (see `to_scalar`).
            Value::Builtin(b) => match b.data().wrapped {
                Some(crate::next::realm::Wrapped::String(s)) => J::String(s.to_string()),
                Some(crate::next::realm::Wrapped::Number(n)) => crate::types::json_number(n),
                Some(crate::next::realm::Wrapped::Boolean(v)) => J::Bool(v),
                None if b.is_callable() => return Some(None),
                None if b.is_array() => J::Array(Vec::new()),
                None => J::Object(serde_json::Map::new()),
            },
        }))
    }
    fn open(value: &Value) -> Frame {
        match value {
            Value::Array(items) => Frame::Array {
                items: Arc::clone(items),
                next: 0,
                out: Vec::with_capacity(items.len().min(SPARSE_CHECK)),
            },
            other => Frame::Object {
                members: own_entries(other).into_iter(),
                key: None,
                out: serde_json::Map::new(),
                levels: if matches!(other, Value::Tag(_)) { 2 } else { 1 },
            },
        }
    }
    fn levels(frame: &Frame) -> usize {
        match frame {
            Frame::Array { .. } => 1,
            Frame::Object { levels, .. } => *levels,
        }
    }
    if let Some(json) = leaf(value) {
        return Ok(json);
    }
    let mut checked = false;
    let mut check = |frame: &Frame| -> Result<(), String> {
        if let Frame::Array { items, .. } = frame
            && !checked
            && items.len() > SPARSE_CHECK
        {
            checked = true;
            json_stringify(value, "")?;
        }
        Ok(())
    };
    let first = open(value);
    check(&first)?;
    let mut nesting = levels(&first);
    if nesting > render_tree_limit() {
        return Err(RANGE_ERROR.into());
    }
    let mut stack = vec![first];
    loop {
        let top = stack.last_mut().expect("a frame");
        let next: Option<Value> = match top {
            Frame::Array { items, next, .. } => {
                if *next < items.len() {
                    let item = items.get(*next).cloned().unwrap_or(Value::Undefined);
                    *next += 1;
                    Some(item)
                } else {
                    None
                }
            }
            Frame::Object { members, key, .. } => loop {
                match members.next() {
                    Some((_, v)) if v.is_undefined() || is_callable(&v) => continue,
                    Some((k, v)) => {
                        *key = Some(k);
                        break Some(v);
                    }
                    None => break None,
                }
            },
        };
        let done = match next {
            Some(child) => match leaf(&child) {
                // An array item that is `undefined` (or a hole) is `null`.
                Some(json) => json.unwrap_or(J::Null),
                None => {
                    let frame = open(&child);
                    check(&frame)?;
                    nesting += levels(&frame);
                    if nesting > render_tree_limit() {
                        return Err(RANGE_ERROR.into());
                    }
                    stack.push(frame);
                    continue;
                }
            },
            None => {
                let frame = stack.pop().expect("a frame");
                nesting -= levels(&frame);
                match frame {
                    Frame::Array { out, .. } => J::Array(out),
                    Frame::Object { out, .. } => J::Object(out),
                }
            }
        };
        match stack.last_mut() {
            None => return Ok(Some(done)),
            Some(Frame::Array { out, .. }) => out.push(done),
            Some(Frame::Object { out, key, .. }) => {
                out.insert(key.take().expect("the key"), done);
            }
        }
    }
}

/// The length past which [`Value::to_scalar`] first checks an array's text against V8's
/// string limit (a hole is five characters of it: `null,`).
const SPARSE_CHECK: usize = 1 << 20;

/// The string a property key is: a number path step as JS prints it (`ToPropertyKey`).
pub fn property_key(step: &crate::ast::PathSegment) -> String {
    match step {
        crate::ast::PathSegment::String(s) => s.clone(),
        crate::ast::PathSegment::Number(n) => js::number_to_string(*n),
    }
}

/// `String(value)` of an AST or render-tree value: what a template literal prints (the value
/// as JS holds it: see [`Value::from_scalar`]). `Err` as for [`Value::to_js_string`].
pub fn scalar_to_js_string(value: &Scalar) -> Result<String, String> {
    Ok(Value::from_scalar(value)
        .to_js_string()?
        .into_string_lossy())
}

/// A structural comparison of two values (own properties, prototypes, elements and holes,
/// recursively; strings by code unit; numbers by `==` but `NaN` equal to itself): what
/// [`Scalar::Js`]'s `PartialEq` uses. Not JS's `===` (see [`strict_equals`]). A loop.
pub fn same_shape(a: &Value, b: &Value) -> bool {
    let mut pending: Vec<(Value, Value)> = vec![(a.clone(), b.clone())];
    while let Some((a, b)) = pending.pop() {
        match (&a, &b) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => {}
            (Value::Boolean(x), Value::Boolean(y)) if x == y => {}
            (Value::Number(x), Value::Number(y)) if x == y || (x.is_nan() && y.is_nan()) => {}
            (Value::String(x), Value::String(y)) if x == y => {}
            (Value::Array(x), Value::Array(y)) => {
                if Arc::ptr_eq(x, y) {
                    continue;
                }
                if x.len() != y.len()
                    || x.present_len() != y.present_len()
                    || x.named().count() != y.named().count()
                {
                    return false;
                }
                for ((i, p), (j, q)) in x.present().zip(y.present()) {
                    if i != j {
                        return false;
                    }
                    pending.push((p.clone(), q.clone()));
                }
                for ((k, p), (l, q)) in x.named().zip(y.named()) {
                    if k != l {
                        return false;
                    }
                    pending.push((p.clone(), q.clone()));
                }
            }
            (Value::Object(x), Value::Object(y)) => {
                if Arc::ptr_eq(x, y) {
                    continue;
                }
                if x.len() != y.len() {
                    return false;
                }
                for ((k, p), (l, q)) in x.iter().zip(y.iter()) {
                    if k != l {
                        return false;
                    }
                    pending.push((p.clone(), q.clone()));
                }
                match (&x.proto, &y.proto) {
                    (Proto::Builtin(p), Proto::Builtin(q)) if p == q => {}
                    (Proto::Null, Proto::Null) => {}
                    (Proto::Value(p), Proto::Value(q)) => pending.push((p.clone(), q.clone())),
                    _ => return false,
                }
            }
            (Value::Tag(x), Value::Tag(y)) => {
                if Arc::ptr_eq(x, y) {
                    continue;
                }
                if x.children.len() != y.children.len() || x.attributes.len() != y.attributes.len()
                {
                    return false;
                }
                pending.push((x.name.clone(), y.name.clone()));
                for ((k, p), (l, q)) in x.attributes.iter().zip(y.attributes.iter()) {
                    if k != l {
                        return false;
                    }
                    pending.push((p.clone(), q.clone()));
                }
                for (p, q) in x.children.iter().zip(&y.children) {
                    pending.push((p.clone(), q.clone()));
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests;
