//! Port of upstream Markdoc v0.5.11's `src/transformer.ts`.
//!
//! Owner: the semantics agent.
//!
//! `findSchema`, `attributes`, `children`, `node`, and `globalAttributes` (in
//! [`crate::next::schema::global_attributes`]). Also the resolution the AST classes do before
//! a transform, which upstream keeps beside them: `Node.resolve` (`ast/node.ts`), `resolve`
//! (`ast/base.ts`), `Variable.resolve` (`ast/variable.ts`) and `Function.resolve`
//! (`ast/function.ts`).
//!
//! A transform runs on a resolved document: upstream's `resolve` makes a copy of the AST in
//! which every attribute is a value instead of a `Variable` or `Function`. Here that copy is a
//! [`ResolvedNode`]: the original [`Node`] for everything resolving leaves alone (type, tag,
//! inline, lines, annotations, ...) and the resolved attributes, children and slots, whose
//! values ([`Value`]) can be `undefined`.
//!
//! Upstream's promise handling (`isPromise`, `Promise.all`) has no counterpart: nothing here is
//! asynchronous, and no value a config written as data can produce is a thenable.
//!
//! Where upstream assigns a computed key (`output[name] = value` in `attributes`,
//! `output[k] = ...` in `resolve`), `__proto__` is `Object.prototype`'s setter, not a key; a
//! spread or `Object.fromEntries` (`Node.resolve`'s slots, `mergeConfig`, the partial tag's
//! variables) makes it a key like any other. Each site here does what its upstream line does.
//!
//! Values are JavaScript objects with prototypes ([`Value`]'s object model): `resolve` copies
//! an object whose prototype is `Object.prototype` (`output[k] = ...`, where an own
//! `__proto__` key calls the setter, so the copy gets that prototype) and returns any other
//! object as it is, unresolved (a node's attributes whose prototype a `__proto__` attribute
//! set, a hash literal with a `__proto__` after its first key, a call whose parameters got
//! one): what reads it afterwards reads through the prototype (`String()`, `Tag.isTag`, a
//! variable's path, `node.attributes[key]`). A value is a `Variable` or a call when its
//! chain's `resolve` is upstream's (`Ast.isAst` and `value.resolve instanceof Function`),
//! which then runs with the value as `this` (`this.path.reduce(...)`, `config.functions[
//! this.name]`); an array is resolved by its `map` (an own `map` that is not a function, or
//! an own `constructor` that is not a constructor, throws as V8 does) and keeps its holes.
//!
//! # Divergences from upstream
//!
//! Depth: upstream recurses until V8's stack overflows and throws `RangeError: Maximum call
//! stack size exceeded`. Here each level of `resolve` and `transform` charges what its frames
//! cost node's stack on [`crate::depth`]'s budget ([`cost`]), so that each chain stops where a
//! fresh node 22 process throws (called from the top level of a script) and mixed ones (a
//! deep value in a partial deep in a document) about where node does; the recursions run on
//! a stack that grows as they need it. A warmed-up node goes deeper (the numbers on both
//! sides are on [`cost`]'s items):
//!
//! - `transform`: 1,457 tags (node 1,397 fresh, 1,457 after a deep value was resolved; 2,295
//!   to 2,457 warmed up), 1,064 headings, 1,046 fences, 1,114 `if` tags, 1,230 lists and
//!   items in turn, 1,768 tags in slots, 904 nested partials.
//! - `resolve`: 2,111 nodes on a path through children (node 2,039 for a chain, more than
//!   8,212 warmed up), 1,818 through slots; it walks without recursing.
//! - values: 1,718 nested arrays, 3,483 objects, 1,382 calls (node 1,716, 3,481, 1,381 fresh;
//!   more than 6,928, 3,496, 1,548 warmed up).
//!
//! Where the stack cannot grow ([`crate::depth::can_grow`]: the `stack-growth` feature off,
//! or wasm), the recursions keep fixed limits that fit a 2 MiB stack in a debug build with a
//! quarter spare, the host's limit there: [`MAX_TRANSFORM_DEPTH`] (1,100 levels; a partial
//! counts two) and [`MAX_VALUE_DEPTH`] (1,400 nested arrays or objects, 700 calls), and a
//! transform or resolution stops once it has used three quarters of a 2 MiB stack from its
//! outermost level (a debug build's heading, fence and slot levels take more stack than the
//! counts allow for: 663 headings, 642 fences, 438 slots there).

use crate::ast::{Function, Node, PathSegment, Variable};
use crate::next::realm::Builtin;
use crate::next::schema::value::{
    self, Found as ValueFound, Intrinsic, Method, Object, PlainObject, Proto, TagValue, Value,
};
use crate::next::schema::{self, Config, Found, Schema, TypeSpec, Variables, lookup};
use crate::types::{JsObject, MarkdocError, Result, Scalar};
use indexmap::IndexMap;
use std::cell::Cell;
use std::sync::Arc;

/// A node of a resolved document (upstream's `node.resolve(config)`).
#[derive(Debug, Clone)]
pub struct ResolvedNode<'a> {
    /// The node as parsed: its type, tag, inline flag, annotations, lines, location, errors.
    pub node: &'a Node,
    /// `resolve(node.attributes, config)`: the attributes resolved into a new plain object (a
    /// `__proto__` among them sets its prototype, as `output[k] = v` does) when their
    /// prototype is `Object.prototype`; else what `resolve` makes of the attributes object
    /// itself: the object as it is (its prototype, and unresolved values: `Variable` and
    /// `Function` objects), or, when its prototype makes it a variable or a call (a
    /// `__proto__` attribute holding one), what that resolves to, which can be any value.
    /// Read them as upstream does with [`ResolvedNode::attribute`] (through the prototype).
    pub attributes: Value,
    pub children: Vec<ResolvedNode<'a>>,
    pub slots: IndexMap<String, ResolvedNode<'a>>,
}

impl ResolvedNode<'_> {
    /// `node.attributes[key]`: an own attribute, else the attributes' prototype chain's.
    /// `Err` where a getter on the chain throws.
    pub fn attribute(&self, key: &str) -> Result<Value> {
        match &self.attributes {
            Value::Object(o) => value::object_get(o, key).map_err(throw),
            other => value::get(other, key).map_err(throw),
        }
    }

    /// `node.findSchema(config)`.
    pub fn find_schema<'c>(&self, config: &'c Config) -> SchemaRef<'c> {
        find_schema(self.node, config)
    }

    /// `node.transformAttributes(config)`.
    pub fn transform_attributes(&self, config: &Config) -> Result<Object> {
        attributes(self, config)
    }

    /// `node.transformChildren(config)`.
    pub fn transform_children(&self, config: &Config) -> Result<Vec<Value>> {
        children(self, config)
    }

    /// `node.transform(config)`.
    pub fn transform(&self, config: &Config) -> Result<Value> {
        node(self, config)
    }
}

/// What `findSchema` returns.
#[derive(Debug, Clone, Copy)]
pub enum SchemaRef<'c> {
    /// The config's schema.
    Own(&'c Schema),
    /// A name every JS object inherits (`toString`, `constructor`, ...): a function or a
    /// prototype object, truthy, with none of a schema's properties: an empty schema.
    Inherited,
    /// `undefined` (or a falsy entry).
    None,
}

static EMPTY_SCHEMA: Schema = Schema {
    render: None,
    children: None,
    attributes: None,
    slots: None,
    self_closing: false,
    inline: None,
    transform: None,
    validate: None,
    description: None,
};

impl<'c> SchemaRef<'c> {
    /// The schema, if there is one (an inherited name reads as an empty one).
    pub fn get(self) -> Option<&'c Schema> {
        match self {
            SchemaRef::Own(schema) => Some(schema),
            SchemaRef::Inherited => Some(&EMPTY_SCHEMA),
            SchemaRef::None => None,
        }
    }
}

/// `findSchema(node, {nodes = {}, tags = {}})`: `node.tag ? tags[node.tag] : nodes[node.type]`.
pub fn find_schema<'c>(node: &Node, config: &'c Config) -> SchemaRef<'c> {
    let found = match node.tag.as_deref() {
        Some(tag) if !tag.is_empty() => lookup(&config.tags, tag),
        _ => lookup(&config.nodes, node.node_type.as_str()),
    };
    match found {
        Found::Own(Some(schema)) => SchemaRef::Own(schema),
        Found::Own(None) | Found::Missing => SchemaRef::None,
        Found::Inherited => SchemaRef::Inherited,
    }
}

/// An error where upstream throws.
fn throw(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

/// What each level of `resolve` and `transform` costs on the thread's depth budget
/// ([`crate::depth`]): a share of node's stack, so that each shape on its own goes as deep as in
/// a fresh node 22 process called from the top of a script, and mixed ones (a value resolved
/// deep in a document, lists inside partials) about as deep as node goes. The numbers are the
/// deepest a fresh process takes (and a warmed-up one), measured with
/// `findings/conformance/work/depth/node_shapes.cjs`; a chain of n nodes is n + 2 levels deep
/// counting the document and the text at the bottom.
pub mod cost {
    use crate::depth::{extra, per_level};

    /// A level of a chain of tags without a schema `transform` (with or without `render`),
    /// blockquotes, paragraphs, inlines, strongs or links: `transformer.node`, `children`, its
    /// `flatMap` and the callback. 1,397 nodes in a chain (warm 2,295 to 2,457); 1,457 (warm
    /// 2,507) when a value 500 arrays deep was resolved first, which is this cost's: V8 has
    /// compiled more of what transforming calls by then.
    const PLAIN: u64 = per_level(1_459);
    /// `transformer.node` (a share of `PLAIN`).
    pub const NODE: u64 = PLAIN / 3;
    /// `transformer.children`: the rest of `PLAIN`.
    pub const CHILDREN: u64 = PLAIN - NODE;
    /// A schema's `transform` (the call, `transformChildren` and what a heading's adds): a
    /// chain of headings, 1,063 (warm 1,958); lists and items in turn, 1,208 (warm 2,103).
    pub const TRANSFORM: u64 = extra(1_459, 1_066);
    /// What a fence's `transform` adds to a heading's: 1,045 (warm 1,989).
    pub const FENCE: u64 = extra(1_066, 1_048);
    /// The `if` tag's `transform` over its children (`flatMap` and `child.transform`, after
    /// [`NODE`] and [`TRANSFORM`]): 1,113 (warm 1,898).
    pub const IF: u64 = per_level(1_116) - NODE - TRANSFORM;
    /// What the partial tag's `transform` adds to a schema transform's: partials each
    /// including the next, 903 (warm 1,840).
    pub const PARTIAL: u64 = extra(1_066, 906);
    /// A slot transformed by `transformer.attributes` (after [`NODE`]): tags each in the last
    /// one's slot, 1,767 (warm 2,673).
    pub const SLOT: u64 = per_level(1_771) - NODE;

    /// `Node.resolve` of a child (with `map` and its callback): a chain of 2,037 tags, 2,039
    /// nodes on the path (warm more than 8,212); 40 nested tags around a `debug()` take what
    /// 79 levels of `JSON.stringify` take (`debug()` 4,163 deep at the top, 4,084 in them,
    /// warm 4,147), as if 2,111 nodes fit, which is this cost's.
    pub const RESOLVE_CHILD: u64 = per_level(super::MAX_RESOLVE_DEPTH as u64);
    /// `Node.resolve` of a slot (`Object.entries(...).map`): 1,816 tags each in the last one's
    /// slot (warm 6,424 to 7,159).
    pub const RESOLVE_SLOT: u64 = per_level(1_819);
    /// `Node.resolve`'s own frame (a share of a level), while the node's attributes are
    /// resolved after its children: the `map` of the children has returned.
    pub const RESOLVE_SELF: u64 = RESOLVE_CHILD / 3;
    /// `resolve` of an object (the node's attributes, an object literal): 3,481 nested objects
    /// in a tag's attribute (warm 3,496).
    pub const VALUE_OBJECT: u64 = per_level(3_487);
    /// `resolve` of an array (`map` and its callback): 1,716 nested arrays (warm more than
    /// 6,928).
    pub const VALUE_ARRAY: u64 = per_level(1_720);
    /// A level of nested calls (`not(not(...))`: 1,381, warm 1,548): `resolve` of the call and
    /// `Function.resolve` ([`CALL`], charged by `resolve`), and `resolve` of its parameters
    /// ([`PARAMETERS`]).
    const NESTED_CALL: u64 = per_level(1_384);
    /// `resolve` of a call and `Function.resolve`, which the function's `transform` runs on.
    pub const CALL: u64 = NESTED_CALL / 3 * 2;
    /// `resolve` of a call's parameters (returned before the function's `transform` runs).
    pub const PARAMETERS: u64 = NESTED_CALL - CALL;
}

/// How deep `transform` may go where the stack cannot grow ([`crate::depth::can_grow`]: the
/// `stack-growth` feature off, or wasm): each `transformer.node` call running on the thread is
/// a level, and so is each partial being transformed; one level more throws upstream's
/// `RangeError: Maximum call stack size exceeded`. Where the stack grows, the levels'
/// [`cost`]s bound the depth instead, at node's.
///
/// Upstream recurses through `node`, `children`, the schemas' `transform`s and partials until
/// V8's stack overflows, at a depth that depends on the nodes (what their transforms add to
/// each level) and on how warm the process is. Node 22 (counting the document, the tags and
/// the leaf on the deepest path, as here) transforms at most:
///
/// | chain of | fresh process | warmed up |
/// |---|---|---|
/// | tags without a schema `transform`, blockquotes, paragraphs | 1,399 | 2,297 to 2,459 |
/// | slots | 1,769 | 2,675 |
/// | lists and items | 1,210 | 2,105 |
/// | `if` tags | 1,115 | 1,900 |
/// | headings, fences | 1,065, 1,047 | 1,960, 1,991 |
/// | partials, each including the next | 903 partials | 1,840 partials |
///
/// Without stack growth this port stops at this depth, which keeps a quarter of a 2 MiB stack
/// spare in a debug build for every chain above (measured, the costliest a chain of lists,
/// 1.3 KiB a level, and of partials, 2.7 KiB a partial, which is why a partial counts twice),
/// so past 1,100 levels (550 nested partials) it throws where node does not: that band is the
/// host's limit there.
pub const MAX_TRANSFORM_DEPTH: usize = 1100;

/// The deepest node `resolve` (`Node.resolve`) reaches through children, from the top of a
/// thread: a node with more ancestors than `MAX_RESOLVE_DEPTH - 1` throws upstream's
/// `RangeError`. Node 22 resolves a tag chain whose deepest node is the 2,039th on its path
/// from the root in a fresh process (more than 8,212 warmed up, where V8 has optimized the
/// recursion), and 40 nested tags cost what 79 levels of `JSON.stringify` do, as if 2,111
/// fitted, which is what this port takes ([`cost::RESOLVE_CHILD`]); through slots, 1,818
/// (6,424 to 7,159 warmed up). `resolve` walks without recursing and charges its path on the
/// depth budget ([`cost::RESOLVE_CHILD`], [`cost::RESOLVE_SLOT`]), so called deeper (a
/// partial's content, resolved where the partial is transformed) it reaches less deep, as
/// upstream's does.
pub const MAX_RESOLVE_DEPTH: usize = 2111;

/// How deep `resolve` goes into a value (an attribute's arrays and objects, a function call's
/// parameters) where the stack cannot grow ([`crate::depth::can_grow`]): each array or object
/// a level, each function call two; one level more throws upstream's `RangeError`. Where the
/// stack grows, the values' [`cost`]s bound the depth instead, at node's.
///
/// Node 22 transforms a tag whose attribute holds 1,716 nested arrays (more than 6,928 warmed
/// up), 3,481 nested objects (3,496) or 1,381 nested calls (`not(not(...))`, 1,548) and throws
/// past them. Without stack growth this port takes 1,399 nested arrays or objects and 699
/// nested calls (the innermost value is a level too) and throws from 1,400 and 700, which keeps
/// a quarter of a 2 MiB stack spare in a debug build (an object costs it 1.05 KiB a level, a
/// call 1.6 KiB): from there up to node's numbers it throws where node does not.
pub const MAX_VALUE_DEPTH: usize = 1400;

/// How much of the thread's stack `transform` and `resolve` may use where the stack cannot
/// grow, from the outermost level on: three quarters of a 2 MiB stack. A level that would
/// start past it throws `RangeError`, whatever its counts: the counts fit a 2 MiB stack in a
/// debug build for the chains [`MAX_TRANSFORM_DEPTH`] lists, and this bounds the rest (a
/// heading's transform, a slot's, a schema `transform` written in Rust, a deep value resolved
/// deep in a transform), measured, not estimated.
const STACK_BUDGET_BYTES: usize = 1536 * 1024;

/// The stack (in a debug build) a level of nested calls of built-in functions costs at most
/// (`realm::tests::the_stack_a_call_costs` measures 5.4 KiB).
const CALL_LEVEL_BYTES: usize = 6 * 1024;

/// Does one more call of a built-in fit the stack the transform shares with it (a value
/// converted deep in a transform, `{% debug(...) %}` in a deep partial)? Where the stack grows
/// ([`crate::depth::can_grow`]) every call runs on a stack with room for it (`realm::call`
/// under [`crate::depth::grow!`]), and [`crate::next::realm::MAX_CALL_DEPTH`] alone bounds
/// them; where it cannot, the call must start a level within [`STACK_BUDGET_BYTES`] of where
/// the outermost `transform` or `resolve` level started (measured, as [`Nested`] measures its
/// levels). Past it the call throws `RangeError`, as upstream's does where V8's stack runs
/// out.
pub(crate) fn call_fits_stack() -> bool {
    if crate::depth::can_grow() || TRANSFORM_DEPTH.get() + VALUE_DEPTH.get() == 0 {
        return true;
    }
    STACK_BASE.get().abs_diff(crate::depth::stack_address()) + CALL_LEVEL_BYTES
        <= STACK_BUDGET_BYTES
}

/// The error upstream throws past a recursion limit.
const STACK_OVERFLOW: &str = "RangeError: Maximum call stack size exceeded";

/// [`STACK_OVERFLOW`] as the error (apart, so that the frames of the recursion stay small).
#[cold]
#[inline(never)]
fn overflow() -> MarkdocError {
    throw(STACK_OVERFLOW)
}

thread_local! {
    /// How many `node` calls are running on this thread (counted where the stack cannot grow).
    static TRANSFORM_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// How many `resolve` calls (of values) are running on this thread (likewise).
    static VALUE_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Where the stack was at the outermost counted level (likewise).
    static STACK_BASE: Cell<usize> = const { Cell::new(0) };
}

/// One more level of a recursion: its cost on the thread's depth budget, and where the stack
/// cannot grow a count in a thread-local against a fixed limit. Both are given back when it is
/// dropped (also when an error or a panic unwinds through it).
pub(crate) struct Nested {
    _level: crate::depth::Level,
    counter: Option<&'static std::thread::LocalKey<Cell<usize>>>,
}

/// One more level of the transform's depth costing `cost`, for what recurses into `transform`
/// besides [`node`] (a partial: [`cost::PARTIAL`]).
pub(crate) fn enter_transform(cost: u64) -> Result<Nested> {
    Nested::enter(cost, &TRANSFORM_DEPTH, MAX_TRANSFORM_DEPTH).ok_or_else(overflow)
}

/// A level costing `cost` on the depth budget alone (what adds to a level that [`node`] or
/// [`resolve`] counts already).
pub(crate) fn charge(cost: u64) -> Result<crate::depth::Level> {
    crate::depth::enter(cost).ok_or_else(overflow)
}

impl Nested {
    /// The level, or `None` where node's stack would run out (or, where the stack cannot grow,
    /// past the fixed limit or [`STACK_BUDGET_BYTES`]).
    fn enter(
        cost: u64,
        counter: &'static std::thread::LocalKey<Cell<usize>>,
        limit: usize,
    ) -> Option<Nested> {
        let counter = if crate::depth::can_grow() {
            None
        } else {
            let depth = counter.get();
            if depth >= limit {
                return None;
            }
            let here = crate::depth::stack_address();
            if TRANSFORM_DEPTH.get() + VALUE_DEPTH.get() == 0 {
                STACK_BASE.set(here);
            } else if STACK_BASE.get().abs_diff(here) > STACK_BUDGET_BYTES {
                return None;
            }
            Some(counter)
        };
        let level = crate::depth::enter(cost)?;
        if let Some(counter) = counter {
            counter.set(counter.get() + 1);
        }
        Some(Nested {
            _level: level,
            counter,
        })
    }
}

impl Drop for Nested {
    fn drop(&mut self) {
        if let Some(counter) = self.counter {
            counter.set(counter.get() - 1);
        }
    }
}

/// `transformer.attributes(node, config)`: the attributes the schema renders, under their
/// render names, after the type's `transform` and the default; then the slots the schema
/// renders, each transformed. Each goes in by assignment (`output[name] = value`), so a render
/// name `__proto__` sets the output's prototype instead of making a key (see
/// [`PlainObject`]).
pub fn attributes(node: &ResolvedNode<'_>, config: &Config) -> Result<Object> {
    let schema = find_schema(node.node, config).get();
    let mut output = PlainObject::new();

    let attrs = schema::merged_attributes(schema);
    for (key, attr) in &attrs {
        let Some(name) = schema::renders(attr, key).map_err(throw)? else {
            continue;
        };

        let mut value = node.attribute(key)?;
        if let Some(t) = &attr.attr_type
            && t.is_function()
        {
            // `const instance = new attr.type(); if (instance.transform) ...`
            match t {
                TypeSpec::Markdoc(m) => {
                    if let Some(v) = m.transform(value.clone()) {
                        value = v;
                    }
                }
                TypeSpec::Custom(custom) => {
                    if let Some(v) = custom.transform(value.clone(), config) {
                        value = v?;
                    }
                }
                // A built-in function that is no constructor: `new` throws, naming the callee
                // by its source text.
                TypeSpec::Realm(b) if !b.is_constructor() => {
                    return Err(throw("TypeError: attr.type is not a constructor"));
                }
                _ => {}
            }
        }
        if value.is_undefined() {
            value = attr.default.clone().unwrap_or(Value::Undefined);
        }

        if value.is_undefined() {
            continue;
        }
        output.assign(name, value);
    }

    if let Some(slots) = schema.and_then(|s| s.slots.as_ref()) {
        for (key, slot) in slots {
            let Some(name) = slot.renders(key).map_err(throw)? else {
                continue;
            };
            match lookup(&node.slots, key) {
                Found::Own(slot_node) => {
                    let value = {
                        let Some(_slot) = crate::depth::enter(cost::SLOT) else {
                            return Err(overflow());
                        };
                        self::node(slot_node, config)?
                    };
                    output.assign(name, value);
                }
                // `node.slots[key]` is a built-in function: transforming it reads its
                // `children` (undefined) and throws.
                Found::Inherited => {
                    return Err(throw(
                        "TypeError: Cannot read properties of undefined (reading 'flatMap')",
                    ));
                }
                Found::Missing => {}
            }
        }
    }

    Ok(output.into_object())
}

/// `transformer.children(node, config)`: each child transformed, an array a child returns
/// spliced in (`flatMap`).
pub fn children(node: &ResolvedNode<'_>, config: &Config) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    if node.children.is_empty() {
        return Ok(out);
    }
    let Some(_level) = crate::depth::enter(cost::CHILDREN) else {
        return Err(overflow());
    };
    for child in &node.children {
        flat_push(&mut out, self::node(child, config)?);
    }
    Ok(out)
}

/// `flatMap`'s step: an array's elements (a hole is no element), or the value.
/// `Array.prototype` is an array too (`Array.isArray`), of no elements.
pub fn flat_push(out: &mut Vec<Value>, value: Value) {
    match value {
        Value::Array(items) => out.extend(items.present().map(|(_, v)| v.clone())),
        other if value::is_array(&other) => {}
        other => out.push(other),
    }
}

/// `transformer.node(node, config)`: the schema's `transform` if it has one; else the children,
/// in a tag named by the schema's `render` if it has one. Where node's stack would run out
/// (the levels' [`cost`]s on the depth budget, through children, slots, schema transforms and
/// partials alike; past [`MAX_TRANSFORM_DEPTH`] calls where the stack cannot grow), it throws
/// upstream's `RangeError` instead. Each level runs where the stack has room for it
/// ([`crate::depth`]), so a schema `transform` written in Rust may recurse through
/// [`ResolvedNode::transform_children`] or [`node`] as deep as upstream's would.
pub fn node(node: &ResolvedNode<'_>, config: &Config) -> Result<Value> {
    let Some(_depth) = Nested::enter(cost::NODE, &TRANSFORM_DEPTH, MAX_TRANSFORM_DEPTH) else {
        return Err(overflow());
    };
    crate::depth::grow!(node_here(node, config))
}

/// The body of [`node`], on a stack with room for it.
#[inline(always)]
fn node_here(node: &ResolvedNode<'_>, config: &Config) -> Result<Value> {
    let schema = find_schema(node.node, config).get();
    if let Some(transform) = schema.and_then(|s| s.transform.as_ref()) {
        let Some(_transform) = crate::depth::enter(cost::TRANSFORM) else {
            return Err(overflow());
        };
        return transform(node, config);
    }

    let children = children(node, config)?;
    rendered(node, config, schema, children)
}

/// The rest of [`node`] without a schema `transform`: the children, in a tag named by the
/// schema's `render` if it has one. (A function of its own so that the frame `node` keeps on
/// the stack for every level of a deep tree stays small.)
#[inline(never)]
fn rendered(
    node: &ResolvedNode<'_>,
    config: &Config,
    schema: Option<&Schema>,
    children: Vec<Value>,
) -> Result<Value> {
    let Some(render) = schema.and_then(|s| s.render.as_ref()) else {
        return Ok(Value::array(children));
    };

    let attributes = attributes(node, config)?;
    Ok(TagValue::new(render.clone(), attributes, children).into())
}

// ---------------------------------------------------------------------------------------
// Resolution: ast/node.ts, ast/base.ts, ast/variable.ts, ast/function.ts
// ---------------------------------------------------------------------------------------

/// `node.resolve(config)`: a copy whose children, attributes and slots are resolved, in
/// upstream's order: the children first (each resolved whole), then the attributes, then the
/// slots. Upstream recurses; this keeps a stack of its own and charges the path it walks on the
/// depth budget as upstream's frames would take node's stack ([`cost::RESOLVE_CHILD`],
/// [`cost::RESOLVE_SLOT`]; [`MAX_RESOLVE_DEPTH`] nodes through children from the top of a
/// thread): a node deeper than what is left throws upstream's `RangeError` when the walk comes
/// to it (after what upstream resolves before it, as upstream's stack overflows there), and the
/// attributes of a node are resolved with its path charged.
pub fn resolve_node<'a>(node: &'a Node, config: &Config) -> Result<ResolvedNode<'a>> {
    /// A node being resolved: what of it is done so far, and the path's cost down to it, its
    /// own level included.
    struct Frame<'a> {
        node: &'a Node,
        children: Vec<ResolvedNode<'a>>,
        attributes: Option<Value>,
        slots: IndexMap<String, ResolvedNode<'a>>,
        path: u64,
        own: u64,
    }
    let frame = |node: &'a Node, path: u64, own: u64| Frame {
        node,
        children: Vec::with_capacity(node.children.len()),
        attributes: None,
        slots: IndexMap::new(),
        path,
        own,
    };
    let left = crate::depth::left();
    if cost::RESOLVE_CHILD > left {
        return Err(throw(STACK_OVERFLOW));
    }
    let mut stack = vec![frame(node, cost::RESOLVE_CHILD, cost::RESOLVE_CHILD)];
    loop {
        let top = stack
            .last_mut()
            .expect("the root is on the stack until it is done");
        let next = if let Some(child) = top.node.children.get(top.children.len()) {
            Some((child, cost::RESOLVE_CHILD))
        } else {
            if top.attributes.is_none() {
                let path = top.path - top.own + cost::RESOLVE_SELF;
                top.attributes = Some(resolve_attributes_at(top.node, path, config)?);
            }
            top.node
                .slots
                .get_index(top.slots.len())
                .map(|(_, slot)| (slot, cost::RESOLVE_SLOT))
        };
        if let Some((next, cost)) = next {
            let path = top.path + cost;
            if path > left {
                return Err(throw(STACK_OVERFLOW));
            }
            stack.push(frame(next, path, cost));
            continue;
        }
        let done = stack.pop().expect("the frame just looked at");
        let resolved = ResolvedNode {
            node: done.node,
            attributes: done.attributes.unwrap_or(Value::Undefined),
            children: done.children,
            slots: done.slots,
        };
        let Some(parent) = stack.last_mut() else {
            return Ok(resolved);
        };
        if parent.children.len() < parent.node.children.len() {
            parent.children.push(resolved);
        } else {
            let (name, _) = parent
                .node
                .slots
                .get_index(parent.slots.len())
                .expect("a slot was being resolved");
            parent.slots.insert(name.clone(), resolved);
        }
    }
}

/// `resolve(this.attributes, config)` for a node whose path down from where `resolve_node`
/// started costs `path` (the node's own `Node.resolve` frame included): the attributes
/// resolved ([`resolve_attributes`]) with the path held on the depth budget, and the object's
/// own level ([`cost::VALUE_OBJECT`]).
#[inline(never)]
fn resolve_attributes_at(node: &Node, path: u64, config: &Config) -> Result<Value> {
    let _path = charge(path)?;
    // (Not a level of the fixed count where the stack cannot grow, as it never was.)
    let _object = charge(cost::VALUE_OBJECT)?;
    resolve_attributes(node, config)
}

/// `resolve(node.attributes, config)` (`Node.resolve`): a new plain object of the resolved
/// values (`output[k] = resolve(v)`) when the attributes' prototype is `Object.prototype`,
/// else what upstream's `resolve` makes of the attributes object (see
/// [`ResolvedNode::attributes`]).
pub fn resolve_attributes(node: &Node, config: &Config) -> Result<Value> {
    match &node.attributes_prototype {
        None => Ok(Value::Object(Arc::new(resolve_object(
            &node.attributes,
            config,
        )?))),
        Some(proto) => {
            let props: JsObject<Value> = node
                .attributes
                .iter()
                .map(|(k, v)| (k.clone(), Value::from_scalar(v)))
                .collect();
            let proto = match proto.as_ref() {
                Scalar::Null => Proto::Null,
                other => Proto::from_value(&Value::from_scalar(other)).unwrap_or(Proto::OBJECT),
            };
            resolve_js(
                &Value::Object(Arc::new(Object::with_proto(props, proto))),
                config,
            )
        }
    }
}

/// `resolve(value, config)` (ast/base.ts) of an AST value: a variable or call resolved, an
/// array or a plain object copied with its items resolved, anything else as it is (see the
/// module's documentation, and [`resolve_value`] for the values the plain [`Scalar`]
/// variants do not hold). A value nested deeper than node's stack takes (the levels' [`cost`]s
/// on the depth budget; [`MAX_VALUE_DEPTH`] where the stack cannot grow) throws upstream's
/// `RangeError`.
pub fn resolve(value: &Scalar, config: &Config) -> Result<Value> {
    // One guard for every kind, so that a debug build's frame, which each level of a deep
    // value keeps on the stack, stays as small as it was (where the stack cannot grow, every
    // value is a level of the fixed count, the innermost too, and a call one more).
    let Some(_depth) = Nested::enter(value_cost(value), &VALUE_DEPTH, MAX_VALUE_DEPTH) else {
        return Err(overflow());
    };
    // What recurses, apart from the rest, so that the frame each level of a deep value keeps
    // on the stack stays small.
    match value {
        Scalar::Array(items) => crate::depth::grow!(resolve_array(items, config)),
        Scalar::Object(o) => {
            crate::depth::grow!(resolve_object(o, config).map(|o| Value::Object(Arc::new(o))))
        }
        Scalar::Function(function) => resolve_function(function, config),
        Scalar::Js(js) => resolve_js(js.value(), config),
        _ => resolve_scalar(value, config),
    }
}

/// What resolving `value` costs on the depth budget, besides what it holds.
fn value_cost(value: &Scalar) -> u64 {
    match value {
        Scalar::Array(_) => cost::VALUE_ARRAY,
        Scalar::Object(_) => cost::VALUE_OBJECT,
        Scalar::Function(_) => cost::CALL,
        _ => 0,
    }
}

/// [`resolve`] of what does not hold values.
#[inline(never)]
fn resolve_scalar(value: &Scalar, config: &Config) -> Result<Value> {
    Ok(match value {
        Scalar::Undefined => Value::Undefined,
        Scalar::Null => Value::Null,
        Scalar::Boolean(b) => Value::Boolean(*b),
        Scalar::Number(n) => Value::Number(*n),
        Scalar::String(s) => Value::string(s.as_str()),
        Scalar::Array(items) => resolve_array(items, config)?,
        Scalar::Object(o) => Value::Object(Arc::new(resolve_object(o, config)?)),
        Scalar::Variable(variable) => resolve_variable(variable, config)?,
        Scalar::Function(function) => resolve_function(function, config)?,
        Scalar::Js(js) => resolve_js(js.value(), config)?,
        // Not a plain object, and without a `resolve` method: returned as it is.
        Scalar::Tag(tag) => Value::Tag(Arc::new(TagValue::from_tag(tag))),
    })
}

/// An array's items resolved (`value.map((item) => resolve(item, config))`). (A loop of its
/// own rather than an iterator chain, whose frames a deep value would stack once per level.)
fn resolve_array(items: &[Scalar], config: &Config) -> Result<Value> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(resolve(item, config)?);
    }
    Ok(Value::array(out))
}

/// A plain object's entries resolved into a new one (`output[k] = resolve(v, config)`), in a
/// JS object's order: built as a [`JsObject`], so n index keys (an object literal's, a call's
/// arguments) take O(n log n) in any order. `output['__proto__'] = v` is the prototype
/// setter, not a property (an object literal's first key can be an own `__proto__`): an
/// object or `null` becomes the copy's prototype, a primitive is ignored.
pub fn resolve_object(object: &IndexMap<String, Scalar>, config: &Config) -> Result<Object> {
    let mut out = PlainObject::new();
    for (k, v) in object {
        let value = resolve(v, config)?;
        out.assign(k.clone(), value);
    }
    Ok(out.into_object())
}

/// [`resolve`] of a JavaScript value (what an AST holds as [`Scalar::Js`], and what such a
/// value holds), as upstream's `resolve` takes it: a primitive as it is; an array by its
/// `map` (V8's `TypeError` when its `map` is not `Array.prototype.map` or its `constructor`
/// is not a constructor; holes stay holes, other properties are not copied); a value whose
/// `$$mdtype` is truthy and whose `resolve` is upstream's (a `Variable`, a call, a `Node`,
/// or an object inheriting from one) by that method, with the value as `this`; an object whose
/// prototype is not `Object.prototype` as it is; a plain object copied with `output[k] =
/// resolve(v)`.
pub fn resolve_value(value: &Value, config: &Config) -> Result<Value> {
    // A level of the value's depth, charged as [`resolve`] charges an AST value's.
    let cost = match value {
        Value::Array(_) => cost::VALUE_ARRAY,
        v if v.is_object() => cost::VALUE_OBJECT,
        _ => 0,
    };
    let _depth = Nested::enter(cost, &VALUE_DEPTH, MAX_VALUE_DEPTH).ok_or_else(overflow)?;
    crate::depth::grow!(resolve_js(value, config))
}

/// [`resolve_value`] within a level already counted.
fn resolve_js(value: &Value, config: &Config) -> Result<Value> {
    if !value.is_object() {
        return Ok(value.clone());
    }
    if let Value::Array(items) = value {
        // `value.map((item) => resolve(item, config))`.
        value::array_method(value, "map", Method::ArrayMap, "value").map_err(throw)?;
        value::species_check(value).map_err(throw)?;
        let mut present = Vec::with_capacity(items.present_len());
        for (i, item) in items.present() {
            present.push((i, resolve_value(item, config)?));
        }
        return Ok(Value::sparse_array(
            items.len() as u32,
            present,
            IndexMap::new(),
        ));
    }
    // `isAst(value) && value?.resolve instanceof Function`.
    if value::is_ast(value) {
        match resolve_method(value)? {
            Some(Ok(method)) => return call_resolve(method, value, config),
            // Another function of the realm, called with the value as `this` and the config
            // (a plain object to every function a value can hold here).
            Some(Err(f)) => {
                return match crate::next::realm::call(f, value, &[Value::empty_object()])
                    .map_err(throw)?
                {
                    crate::next::realm::Called::Value(v)
                    | crate::next::realm::Called::Wrapper(v) => Ok(v),
                };
            }
            None => {}
        }
    }
    if !value::has_object_prototype(value) {
        return Ok(value.clone());
    }
    let mut out = PlainObject::new();
    for (k, v) in value::own_entries(value) {
        let v = resolve_value(&v, config)?;
        out.assign(k, v);
    }
    Ok(Value::Object(Arc::new(out.into_object())))
}

/// The `resolve` a value's chain finds, when it is a function: one of upstream's
/// (`Variable`'s, `Function`'s, `Node`'s; `Ok`), or another function of the realm (`Err`);
/// `None` for anything else (an own `resolve` that is no function).
fn resolve_method(value: &Value) -> Result<Option<std::result::Result<Method, Builtin>>> {
    let f = match value::lookup(value, "resolve").map_err(throw)? {
        ValueFound::Builtin(_, Intrinsic::Method(m)) => m.builtin(),
        ValueFound::Builtin(_, Intrinsic::Ctor(c)) => c.builtin(),
        ValueFound::Value(Value::Builtin(f)) if f.is_callable() => f,
        _ => return Ok(None),
    };
    Ok(Some(match Method::of(f) {
        Some(m @ (Method::VariableResolve | Method::AstFunctionResolve | Method::NodeResolve)) => {
            Ok(m)
        }
        _ => Err(f),
    }))
}

/// One of upstream's `resolve` methods called with `this` (any value whose chain has it).
fn call_resolve(method: Method, this: &Value, config: &Config) -> Result<Value> {
    match method {
        Method::VariableResolve => variable_resolve(this, config),
        Method::AstFunctionResolve => function_resolve(this, config),
        Method::NodeResolve => node_resolve(this, config),
        _ => unreachable!("a resolve method"),
    }
}

/// `Variable.resolve({variables})`: `variables(path)` when the variables are a function, else
/// `path.reduce((obj = {}, key) => obj[key], variables)`.
pub fn resolve_variable(variable: &Variable, config: &Config) -> Result<Value> {
    let mut obj = match &config.variables {
        Some(Variables::Fn(f)) => return Ok(f(&variable.path)),
        Some(Variables::Value(v)) => v.clone(),
        None => Value::Undefined,
    };
    for step in &variable.path {
        if obj.is_undefined() {
            obj = Value::empty_object();
        }
        obj = value::get_property(&obj, &value::property_key(step)).map_err(throw)?;
    }
    Ok(obj)
}

/// `Variable.prototype.resolve` with `this` any value: `variables instanceof Function ?
/// variables(this.path) : this.path.reduce((obj = {}, key) => obj[key], variables)`, `path`
/// read through `this`'s chain and reduced as `Array.prototype.reduce` does (its elements
/// present, in order; V8's `TypeError: this.path.reduce is not a function` when its `reduce`
/// is not that one).
fn variable_resolve(this: &Value, config: &Config) -> Result<Value> {
    let path = value::get(this, "path").map_err(throw)?;
    let mut obj = match &config.variables {
        Some(Variables::Fn(f)) => return Ok(f(&path_segments(&path))),
        Some(Variables::Value(v)) => v.clone(),
        None => Value::Undefined,
    };
    value::array_method(&path, "reduce", Method::ArrayReduce, "this.path").map_err(throw)?;
    let (_, steps) = value::array_like_elements(&path).map_err(throw)?;
    for (_, step) in steps {
        if obj.is_undefined() {
            obj = Value::empty_object();
        }
        let key = value::to_property_key(&step).map_err(throw)?;
        obj = value::get(&obj, &key).map_err(throw)?;
    }
    Ok(obj)
}

/// A path value as a variables function takes it (a Rust caller's `Variables::Fn`): each
/// element present a step, a number as a number, anything else as `String()` makes it.
fn path_segments(path: &Value) -> Vec<PathSegment> {
    value::array_like_elements(path)
        .map(|(_, steps)| {
            steps
                .into_iter()
                .map(|(_, step)| match step {
                    Value::Number(n) => PathSegment::Number(n),
                    other => {
                        PathSegment::String(value::to_property_key(&other).unwrap_or_default())
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `Function.resolve(config)`: `null` for a function the config does not have; else its
/// `transform` of the resolved parameters (`undefined` without one).
pub fn resolve_function(function: &Function, config: &Config) -> Result<Value> {
    let found = lookup(&config.functions, &function.name);
    if let Found::Missing | Found::Own(None) = found {
        return Ok(Value::Null);
    }
    // `resolve(this.parameters)`: its frames on the depth budget, and where the stack cannot
    // grow the call's second level of the fixed count (see [`MAX_VALUE_DEPTH`]; [`resolve`]
    // counted the first and charged `Function.resolve`, [`cost::CALL`], which the function's
    // `transform` runs on).
    let parameters = {
        let Some(_parameters) = Nested::enter(cost::PARAMETERS, &VALUE_DEPTH, MAX_VALUE_DEPTH)
        else {
            return Err(overflow());
        };
        crate::depth::grow!(resolve_object(&function.parameters, config))?
    };
    call_function(found, Value::Object(Arc::new(parameters)), config)
}

/// `Function.prototype.resolve` (upstream's AST class) with `this` any value:
/// `config.functions[this.name]` (`null` when falsy), then its `transform?.(resolve(this.
/// parameters, config), config)`.
fn function_resolve(this: &Value, config: &Config) -> Result<Value> {
    let name = value::get(this, "name").map_err(throw)?;
    let name = value::to_property_key(&name).map_err(throw)?;
    let found = lookup(&config.functions, &name);
    if let Found::Missing | Found::Own(None) = found {
        return Ok(Value::Null);
    }
    let parameters = value::get(this, "parameters").map_err(throw)?;
    // `resolve(this.parameters)`, as in [`resolve_function`].
    let parameters = {
        let Some(_parameters) = Nested::enter(cost::PARAMETERS, &VALUE_DEPTH, MAX_VALUE_DEPTH)
        else {
            return Err(overflow());
        };
        crate::depth::grow!(resolve_value(&parameters, config))?
    };
    call_function(found, parameters, config)
}

/// The rest of [`resolve_function`]: the function's `transform` of the resolved parameters,
/// `undefined` without one (a built-in function of every object has none). (Apart, so that
/// the frame each level of nested calls keeps on the stack stays small.)
#[inline(never)]
fn call_function(
    found: Found<'_, Option<schema::ConfigFunction>>,
    parameters: Value,
    config: &Config,
) -> Result<Value> {
    match found {
        Found::Own(Some(f)) => match &f.transform {
            Some(transform) => transform(&parameters, config),
            None => Ok(Value::Undefined),
        },
        _ => Ok(Value::Undefined),
    }
}

/// `Node.prototype.resolve` with `this` any value (a hand-made token's `Node` object):
/// `Object.assign(new Node(), this, {children: this.children.map((child) =>
/// child.resolve(config)), attributes: resolve(this.attributes, config), slots:
/// Object.fromEntries(Object.entries(this.slots).map(([name, slot2]) => [name,
/// slot2.resolve(config)]))})`.
fn node_resolve(this: &Value, config: &Config) -> Result<Value> {
    let call = |item: &Value, expression: &str| -> Result<Value> {
        match resolve_method(item)? {
            Some(Ok(method)) => call_resolve(method, item, config),
            Some(Err(f)) => {
                match crate::next::realm::call(f, item, &[Value::empty_object()]).map_err(throw)? {
                    crate::next::realm::Called::Value(v)
                    | crate::next::realm::Called::Wrapper(v) => Ok(v),
                }
            }
            None => Err(throw(format!(
                "TypeError: {expression}.resolve is not a function"
            ))),
        }
    };
    let children = value::get(this, "children").map_err(throw)?;
    value::array_method(&children, "map", Method::ArrayMap, "this.children").map_err(throw)?;
    value::species_check(&children).map_err(throw)?;
    let (length, elements) = value::array_like_elements(&children).map_err(throw)?;
    let mut resolved = Vec::with_capacity(elements.len());
    for (i, child) in elements {
        resolved.push((i as u32, call(&child, "child")?));
    }
    let children = Value::sparse_array(length as u32, resolved, IndexMap::new());
    let attributes = value::get(this, "attributes").map_err(throw)?;
    let attributes = resolve_value(&attributes, config)?;
    let slots = value::get(this, "slots").map_err(throw)?;
    let mut slots_out = JsObject::new();
    for (name, slot) in value::own_entries_of(&slots).map_err(throw)? {
        slots_out.insert(name, call(&slot, "slot2")?);
    }
    // `Object.assign(new Node(), this, {children, attributes, slots})`.
    let mut node = PlainObject::with_proto(Proto::Builtin(value::BuiltinProto::NODE_PROTOTYPE));
    for (k, v) in new_node_properties() {
        node.assign(k, v);
    }
    for (k, v) in value::own_entries(this) {
        node.assign(k, v);
    }
    node.assign("children", children);
    node.assign("attributes", attributes);
    node.assign("slots", Value::object(slots_out));
    Ok(Value::Object(Arc::new(node.into_object())))
}

/// `new Node()`'s own properties, in their order.
fn new_node_properties() -> Vec<(&'static str, Value)> {
    vec![
        ("$$mdtype", Value::string("Node")),
        ("errors", Value::array(Vec::new())),
        ("lines", Value::array(Vec::new())),
        ("inline", Value::Boolean(false)),
        ("attributes", Value::empty_object()),
        ("children", Value::array(Vec::new())),
        ("type", Value::string("node")),
        ("tag", Value::Undefined),
        ("annotations", Value::array(Vec::new())),
        ("slots", Value::empty_object()),
    ]
}

#[cfg(test)]
mod tests;
