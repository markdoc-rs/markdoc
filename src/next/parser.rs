//! Port of upstream Markdoc v0.5.11's `src/parser.ts`.
//!
//! Owner: the parser agent.
//!
//! Tokens to [`Node`]: [`parse`] is upstream's `parser(tokens, args)`, line for line
//! (`mappings`, `annotate`, `handleAttrs`, `handleToken`, then the transforms in
//! [`super::transforms`]), quirks included:
//!
//! - a closing token pops the node on top of the stack only when that node has the token's
//!   type and tag; otherwise it becomes a node of its own with a `missing-opening` error;
//! - after a token's children the stack loses its top, whatever that is, so an inline tag
//!   left open (`a {% foo %} b`) takes the `inline` node's place and leaves it open: the
//!   paragraph's close then misses its opening and both stay open to the end;
//! - nodes still open at the end get `missing-closing`;
//! - a node is attached to its parent when it is made, before its children;
//! - attributes, the objects attribute values are and slots are JavaScript objects, and
//!   `annotate` reads and writes them as JavaScript does (see `heap`, the parse's objects):
//!   `node.attributes[name] !== undefined` reads through the attributes' prototype (an
//!   inherited name counts as set), `node.attributes[name] = value` is `[[Set]]` (an
//!   attribute named `__proto__` given an object or `null` makes it the attributes'
//!   prototype, which later reads go through), `node.attributes.class` reads through the
//!   prototype too, and `.name` (`class[name] = value`) sets a property of whatever the class
//!   is: an object's (in a JS object's key order; an own `__proto__` from an object literal is
//!   overwritten, else `.__proto__` does nothing), an array's element or `length` (holes as JS
//!   has them: the array is sparse, up to 2^32 - 1 long, whatever its length costs nothing), an
//!   array's other own property (`.b`), a `Variable`'s or `Function`'s own property (`.b`, or
//!   `.path` replaced), or V8's `TypeError` for a string, number or boolean (a string's
//!   `length` and indices are read-only, any other name cannot be created), except
//!   `.__proto__`, which does nothing;
//! - an annotation is the attribute object upstream pushes, so the one that set `class` to an
//!   object or an array shows what later `.name` annotations, inline ones included, did to it,
//!   and a `__proto__` annotation shows what `.name` did to the object it made the prototype;
//! - the tag grammar's objects keep the prototype a later `__proto__` key gave them
//!   (`Object.assign`) and a call's parameters the one a named `__proto__` parameter gave
//!   them, so the AST holds them ([`Scalar::Js`]);
//! - a slot named by a `primary` the attributes inherit is filed under it, and a slot named
//!   `__proto__` (`parent.slots.__proto__ = node`) is the slots' prototype
//!   ([`Node::slots_prototype`]), in no list of slots or children;
//! - a hand-made token's `meta` is read as `Ast.fromJSON` revives it (the harness's oracle
//!   revives hand-made tokens so): an object whose `$$mdtype` names `Variable`, `Function` or
//!   `Node` is that class's instance (`Object.assign(new klass(), value)`), and an annotation
//!   that is not an attribute object is in `annotations` as it is
//!   ([`AttributeValue::raw`]).
//!
//! Once the tokens are parsed nothing changes these objects any more, so the AST holds them as
//! values made then: the plain [`Scalar`] variants for plain data, [`Scalar::Js`] for an
//! object that has a prototype, holes or extra properties (an object two references share is
//! one value), and [`Node::attributes_prototype`] for the attributes' prototype.
//!
//! Upstream's positions, exactly ([`ParserArgs::location`], on by default as upstream's):
//!
//! - **`lines`** ([`Node::lines`]) is `token.map || parent.lines || []`: the node's own copy
//!   of its token's 0-based line map (`[first, last + 1]`), or, for a token without a map
//!   (an inline token's children), the very array its parent holds. A closing token whose
//!   node it closes appends its own map to that node's array (`parent.lines.push(...map)`):
//!   `{% callout %}` on lines 2-3 closed on lines 4-5 has `lines` `[2, 3, 4, 5]`, and so has
//!   every node that shares the array, made before the closing or after it (a token both
//!   open and with `children` stays on the stack, so a closing among its children grows the
//!   lines its later children share). Here the node whose token had the map owns the array
//!   and a closing grows it in place; a node that shares it gets its final content at the
//!   end. The document's is `[]` (and grows only under a hand-made `document_close`).
//! - **`location`** ([`Node::location`]) is `{file, start: {line: lines[0], character:
//!   position.start}, end: {line: lines[1], character: position.end}}` as the node's lines
//!   are when it is made (a closing appended later does not move it): `file` is
//!   [`ParserArgs::file`], `character` the token's `position` (only the tags Markdoc finds
//!   in a fence have one: UTF-16 units from the newline before the tag's line), copied as it
//!   is (a hand-made token's may be any value; `null` throws upstream's `TypeError`); an
//!   undefined member is left out of JSON. The document, and the nodes the table transform
//!   makes, have none.
//! - With `location: false` neither is set: every node keeps its own empty `lines`, which a
//!   closing still appends to, as upstream's do.
//! - **Errors** are upstream's objects: a token's own `errors` entries as they are (a hand-made
//!   token may give any value: [`ValidationError::raw`]), `parse-error` with the tokenizer's
//!   `location` (`null` for a tag inline, `{start: {offset}, end: {offset}}` in the source for
//!   a tag on its own lines, `{start: {line, character}, end: {line, character}}` for one in
//!   a fence), and `missing-opening`, `missing-closing`, `duplicate-attribute` and
//!   `no-inline-annotations` with none.
//!
//! This crate's own positions are [`Node::span`] and [`ValidationError::span`] ([`Span`]: a
//! 1-based line, a UTF-16 column and a byte offset into `source`, the text the tokens were
//! made from, at each end; also what the old `crate::parser` gave). They are opt-in: set only
//! with [`ParserArgs::spans`] on (and `location` not `false`); off, the default, the parser
//! does none of their work and every node and error has `span: None`. Finding them costs
//! about half of a parse, and upstream has no such thing: its own positions, `lines` and
//! `location` above, are the same either way. A tag node spans exactly its `{% ... %}`,
//! inline tags included; a text node starts at its first content byte; a block starts at
//! its first marker or character; the document spans the whole source; the nodes the table
//! transform makes span their rows (see [`super::transforms::table`]). The errors the parser
//! makes have the span of what they are
//! about: `missing-closing` the opener's, `missing-opening` the closing token's, `parse-error`
//! the tag's, `duplicate-attribute` and `no-inline-annotations` the tag or annotation that
//! brought the attribute. A node made of a block's closing token (one that misses its
//! opening) sits, empty, where that block ends. `parser/positions.rs` says how the spans are
//! found.
//!
//! Where this crate differs from upstream, on purpose:
//!
//! - **Frontmatter**, with [`ParserArgs::parse_frontmatter`] (off by default: the attribute is
//!   then upstream's raw text): the document's `frontmatter` attribute is the YAML parsed as
//!   the old parser parses it (a mapping, as [`Scalar::Object`], keys in YAML order as a JS
//!   object orders them). YAML that does not parse to a mapping keeps upstream's raw string
//!   and adds a `frontmatter-invalid` error to the document; parsing never fails over it.
//!
//! # Divergences from upstream
//!
//! What upstream does only on V8's stack, or with values a Rust `String` cannot hold. The port's
//! behaviour on each is deterministic and pinned by a test in `parser/tests.rs`.
//!
//! - **Depth: where V8's stack runs out** (node 22.22.1, default stack; each limit's own
//!   docs give the measurements). The port's walks keep their stacks on the heap and charge
//!   their path on the depth budget ([`crate::depth`]) as upstream's frames take node's
//!   stack, so that called from the top of a thread they stop where node's fresh process
//!   throws `RangeError: Maximum call stack size exceeded` at the top level of a script, and
//!   called deeper (from a schema's transform in Rust) earlier; a warmed-up node, whose
//!   compiled frames are smaller, goes a little further.
//!   - [`MAX_AST_DEPTH`] (4,643): a tree whose deepest node is 4,644 levels down throws in
//!     a fresh process; the warmed-up node parses 4,644 (4,645 throws), and called from
//!     deeper in its stack node throws earlier (past 4,562 in the harness's oracle). The
//!     shapes that reach it are inline tags left open (`p {% x %} q` paragraphs, two levels
//!     each) and block tags with `maxNesting` raised; a chain of block slots goes to 4,821 in
//!     node (not modeled: V8 tiers the walk differently there, see the constant).
//!     Pinned by `the_tree_parses_to_its_depth_limit`.
//!   - [`MAX_TOKEN_NESTING`] (2,914): a hand-made token 2,915 levels down in `children`
//!     throws in both (unless it is hidden or an empty text, which node's fresh process
//!     still parses); a warmed-up node parses tokens 2,929 to 3,624 levels down. Pinned by
//!     `nested_tokens_parse_to_the_nesting_limit`.
//!   - [`MAX_VALUE_DEPTH`] (300): a hand-made token's `meta` value nested deeper where the
//!     stack cannot grow ([`crate::depth::can_grow`]), and its `position` member or `errors`
//!     entry nested deeper anywhere; upstream's parser keeps any (it never looks inside), the
//!     port returns the error. Pinned by `deep_meta_values_parse_to_the_value_limit`.
//! - **What only a hand-made token holds** (Markdoc's tokenizer never makes it): a `tag` that
//!   is not a string is kept as `String()` makes it ([`Node::tag`] is a string); an annotation
//!   that is not an attribute object is in `annotations` as it is; an entry of its `errors`
//!   is kept whole ([`ValidationError::raw`]).
//!
//! `parse` returns `Err` exactly where upstream's parser throws (a `TypeError` from JS
//! semantics, or V8's `RangeError` at the depths above), and past [`MAX_VALUE_DEPTH`]. The
//! error is [`MarkdocError::ParseError`] holding JS's `String(error)` of what upstream throws,
//! its name and message (`TypeError: Cannot create property 'b' on string 'a'` for `{% x
//! class="a" .b /%}`, `RangeError: Maximum call stack size exceeded`, `RangeError: Invalid
//! array length`, `TypeError: klass is not a constructor` for a hand-made `$$mdtype` that
//! `Ast.fromJSON` cannot construct).

mod heap;
pub(crate) mod positions;

use self::heap::{HFound, HValue, Heap, Memo};
use self::positions::{Extent, Positions};
use crate::ast::Node;
use crate::grammar;
use crate::markdown_it::{AttrValue, Token, js};
use crate::sourcemap::LineIndex;
use crate::types::{
    AttributeValue, Location, LocationEdge, MarkdocError, NodeType, Position, Result, Scalar, Span,
    ValidationError, ValidationLevel, array_index, js_insert,
};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::sync::Arc;

/// Upstream's `ParserArgs` (`types.ts`): `{file?, slots?, location?, conditionalTags?}`, and
/// three switches of this crate's, off by default.
#[derive(Debug, Clone, PartialEq)]
pub struct ParserArgs {
    /// `location.file` of every node.
    pub file: Option<String>,
    /// Put `{% slot "name" %}` tags in their parent's `slots` instead of its children.
    pub slots: bool,
    /// Give nodes `lines` and `location` (upstream: unless `location: false`).
    pub location: bool,
    /// The tags the table transform keeps as row wrappers; `None` is upstream's default,
    /// `['if']`.
    pub conditional_tags: Option<Vec<String>>,
    /// This crate's: the document's `frontmatter` attribute is the YAML parsed into an object
    /// (see the module's documentation). Off: upstream's raw text.
    pub parse_frontmatter: bool,
    /// This crate's: `crate::next::index::parse` does not tokenize a byte-order mark that
    /// starts the text (upstream does, so a heading on line 1 is then a paragraph). The
    /// parser takes tokens as they are and ignores it.
    pub strip_bom: bool,
    /// This crate's: give nodes and the parser's errors this crate's own positions,
    /// [`Node::span`] and [`ValidationError::span`] (1-based line, UTF-16 column and byte
    /// offset at each end; see the module's documentation), which `validate()` passes on as
    /// [`crate::validator::ValidateError::span`]. Off by default, as upstream has none: the
    /// parser then does none of their work (about half of a parse) and every `span` is
    /// `None`. A tool that reports positions (a linter, a language server) turns it on.
    /// Needs `location`: with `location: false` nothing has a span.
    pub spans: bool,
}

impl Default for ParserArgs {
    /// `parser(tokens)`: no file, no slots, locations on, `conditionalTags` defaulted; this
    /// crate's switches off (no spans).
    fn default() -> Self {
        ParserArgs {
            file: None,
            slots: false,
            location: true,
            conditional_tags: None,
            parse_frontmatter: false,
            strip_bom: false,
            spans: false,
        }
    }
}

impl ParserArgs {
    /// `parser(tokens, 'file.md')`: a file name alone.
    pub fn file(file: impl Into<String>) -> Self {
        ParserArgs {
            file: Some(file.into()),
            ..ParserArgs::default()
        }
    }
}

/// `mappings`: token types whose nodes are named otherwise.
fn mapping(type_name: &str) -> Option<&'static str> {
    match type_name {
        "ordered_list" => Some("list"),
        "bullet_list" => Some("list"),
        "code_inline" => Some("code"),
        "list_item" => Some("item"),
        "variable" => Some("text"),
        _ => None,
    }
}

/// `CreateDataProperty(object, name, value)`: in its place when the object has it, else
/// appended, so an array-index key may sit out of a JS object's order until [`js_order`]
/// puts the object right, once (n keys in any order cost O(n log n)).
fn js_append<V>(object: &mut IndexMap<String, V>, name: &str, value: V) {
    match object.get_mut(name) {
        Some(slot) => *slot = value,
        None => {
            object.insert(name.to_owned(), value);
        }
    }
}

/// An object [`js_append`] built, in a JS object's key order: the array-index keys ascending,
/// then the others in the order they were first set (a stable sort, when it is needed).
fn js_order<V>(object: &mut IndexMap<String, V>) {
    let rank = |key: &str| array_index(key).map_or((1, 0), |n| (0, n));
    if !object.keys().is_sorted_by_key(|k| rank(k)) {
        object.sort_by(|a, _, b, _| rank(a).cmp(&rank(b)));
    }
}

/// JS truthiness of a JSON value.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `String(value)` for a JSON value (what a template literal or a property key makes of it).
/// An array is `array.join(",")`, null items empty and nested arrays joined in their place,
/// from a stack on the heap (a hand-made token's array may nest to any depth).
fn json_string(value: &Value) -> String {
    let items = match value {
        Value::Null => return "null".into(),
        Value::Bool(b) => return b.to_string(),
        Value::Number(n) => return js::number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => return s.clone(),
        Value::Object(_) => return "[object Object]".into(),
        Value::Array(items) => items,
    };
    let mut out = String::new();
    // Per open array: what is left of it, and whether an item of it has been written.
    let mut stack = vec![(items.iter(), false)];
    while let Some((rest, written)) = stack.last_mut() {
        let Some(item) = rest.next() else {
            stack.pop();
            continue;
        };
        if *written {
            out.push(',');
        }
        *written = true;
        match item {
            Value::Null => {}
            Value::Array(inner) => stack.push((inner.iter(), false)),
            item => out.push_str(&json_string(item)),
        }
    }
    out
}

/// How deep a token may sit in `children` for [`parse`] (a top-level token is at level 0
/// and its children at level 1); one deeper is `RangeError: Maximum call stack size
/// exceeded`, before it is handled.
///
/// Upstream's `handleToken` recurses into `children`, and V8 (node 22.22.1, its default
/// stack) runs out of stack there. In a fresh process a token at level 2,915 throws (a hidden
/// or empty text token there returns at once and does not; at 2,916 every token throws);
/// once V8 has compiled `handleToken` the recursion goes deeper, measured at 2,967 to 3,624
/// levels depending on what was compiled. Here the walk is iterative and stops where the
/// fresh process does, from the top of a thread: each level costs `TOKEN_COST` of what is
/// left of the depth budget ([`crate::depth`]). Markdoc's tokenizer nests tokens two deep at
/// most, so only hand-made tokens reach it; those between this level and V8's parse upstream
/// (not modeled).
pub const MAX_TOKEN_NESTING: usize = 2_914;

/// What a level of `handleToken`'s recursion into `children` costs on the depth budget.
pub(crate) const TOKEN_COST: u64 = crate::depth::per_level(MAX_TOKEN_NESTING as u64);

/// How deep the tree [`parse`] returns may be: its deepest node at most this many levels
/// below the document (whose children are 1 down). Deeper is `RangeError: Maximum call
/// stack size exceeded`.
///
/// Upstream's `parse` ends with the table transform, which walks the whole tree with
/// `walk()`, a recursive generator (slots and children alike), and V8 (node 22.22.1, its
/// default stack) runs out of stack in it at a node 4,644 levels down in a fresh process,
/// `parse` called from the top level of a script; once warmed up, 4,644 still parses and
/// 4,645 throws. The depth is the same for every
/// shape that nests: unclosed inline tags, which leave their inline node and its block
/// open (two levels per paragraph, heading, tight list item or slot-holding paragraph;
/// three per loose list item, blockquote, table row or list-table row; annotations change
/// nothing), and block tags nested in children and slots in turn, past markdown-it's
/// `maxNesting` (100 by default; block structures stop there). One shape goes further:
/// block slots each in the last one's slot (`maxNesting` raised, slots on), 4,821 in a
/// fresh process and 4,823 warmed up: V8 compiles that walk differently (`walk()` alone takes
/// 4,645 nodes through children, 4,824 through slots only, 4,646 through both in turn, and
/// 3,662 through slots above 100 children), which this port does not model. The port's walk
/// keeps its stack on the heap ([`super::transforms::table`]), charges each level
/// `AST_COST` of what is left of the depth budget ([`crate::depth`]) and returns the error
/// where the fresh process throws on every other shape; deeper trees that a warmed-up node,
/// or a chain of slots, still parses are not modeled. Node's limit also moves with the stack
/// below the call:
/// `Markdoc.parse` called from the harness's oracle (`tests/next/oracle.cjs`, inside a
/// readline callback) throws for 2,281 paragraphs of `{% a %} x` (a tree 4,564 deep) and
/// parses 2,280 (4,562 deep), so there node throws for trees 4,563 to 4,642 deep that the
/// port returns.
/// (Measurements: `findings/conformance/work/depth/node-limits-*.jsonl`,
/// `findings/conformance/work/fix-parser/node-depth.txt`; the oracle's,
/// `findings/conformance/work/join/`.)
pub const MAX_AST_DEPTH: usize = 4_643;

/// What a level of the table transform's `walk()` costs on the depth budget.
pub(crate) const AST_COST: u64 = crate::depth::per_level(MAX_AST_DEPTH as u64);

/// How deep the values of a token's `meta` may nest (arrays, objects, functions and
/// variables in one another) where the stack cannot grow ([`crate::depth::can_grow`]: the
/// `stack-growth` feature off, or wasm) before [`parse`] returns `RangeError: Maximum call
/// stack size exceeded`. Where it grows, any depth: upstream's parser keeps such a value as it
/// is (it never looks inside), and this crate's [`Scalar`] is safe to drop, clone, compare and
/// (de)serialize at any depth there. The members of a token's `position` and the entries of
/// its `errors`, which upstream copies into the AST as they are, are held as JSON
/// (`serde_json`'s values, whose own drop and clone recurse): this limit holds for them
/// wherever the stack is.
///
/// Where the stack cannot grow, the tag grammar never makes a value nested deeper than this
/// (its own fixed limit, [`crate::grammar::MAX_NESTING`]: 299 arrays, objects or calls in one
/// another, and a variable or a value in the last), and a [`Scalar`]'s `Clone`, `PartialEq`
/// and serde recurse on the thread's own stack, so a hand-made token's deeper value is the
/// error (the host's limit there).
pub const MAX_VALUE_DEPTH: usize = 300;

/// `Err(RangeError)` unless `value` nests at most [`MAX_VALUE_DEPTH`] deep: a scalar is 0
/// deep, `[1]` and `{"a": 1}` 1, and an AST value's JSON as deep as the grammar nests it (a
/// `{"$$mdtype": "Function", ..., "parameters": {...}}` or `{"$$mdtype": "Variable", "path":
/// [...]}` is one level, its parameters' values and path's steps one deeper), so a token the
/// grammar made parses whether its values come from its JSON or from
/// [`Token::grammar_meta`] (see `grammar_scalar`).
fn check_value_depth(value: &Value) -> Result<()> {
    let container = |v: &&Value| matches!(v, Value::Array(_) | Value::Object(_));
    if !container(&value) {
        return Ok(());
    }
    // The arrays and objects still to look into, with how deep each is.
    let mut stack = vec![(value, 1)];
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_VALUE_DEPTH {
            return Err(stack_overflow());
        }
        match value {
            Value::Array(items) => {
                stack.extend(items.iter().filter(container).map(|v| (v, depth + 1)));
            }
            Value::Object(fields) => {
                // An AST value's own member, whose items sit one level down.
                let own = match fields.get("$$mdtype").and_then(Value::as_str) {
                    Some("Function") => fields.get("parameters"),
                    Some("Variable") => fields.get("path"),
                    _ => None,
                };
                for field in fields.values().filter(container) {
                    match (own, field) {
                        (Some(own), Value::Object(members)) if std::ptr::eq(own, field) => {
                            stack
                                .extend(members.values().filter(container).map(|v| (v, depth + 1)));
                        }
                        (Some(own), Value::Array(items)) if std::ptr::eq(own, field) => {
                            stack.extend(items.iter().filter(container).map(|v| (v, depth + 1)));
                        }
                        _ => stack.push((field, depth + 1)),
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// What V8 throws when its stack runs out, and what [`parse`] returns past its limits
/// ([`MAX_TOKEN_NESTING`], [`MAX_AST_DEPTH`], [`MAX_VALUE_DEPTH`]): `RangeError: Maximum
/// call stack size exceeded`, as [`MarkdocError::ParseError`] holds it (JS's
/// `String(error)`, as for the parser's `TypeError`s).
pub(crate) fn stack_overflow() -> MarkdocError {
    MarkdocError::ParseError("RangeError: Maximum call stack size exceeded".into())
}

/// A hand-made token's JSON `meta` value as an object of the parse (see
/// [`Heap::alloc_json`]: `Ast.fromJSON`'s reviver), after the depth check where the stack
/// cannot grow ([`check_value_depth`]).
fn heap_from_json(heap: &mut Heap, value: &Value) -> Result<HValue> {
    if !crate::depth::can_grow() {
        check_value_depth(value)?;
    }
    heap.alloc_json(value).map_err(MarkdocError::ParseError)
}

/// A value of the tag grammar as an object of the parse (see [`Heap::alloc_grammar`]).
fn heap_from_grammar(heap: &mut Heap, value: &grammar::Value) -> Result<HValue> {
    heap.alloc_grammar(value).map_err(MarkdocError::ParseError)
}

/// An annotation, as `annotate` pushes it onto `node.annotations`: an attribute object the
/// tag grammar made (`{type, name, value}`), or whatever a hand-made token's `attributes`
/// list holds.
enum Annotation {
    Grammar {
        kind: grammar::AttributeType,
        name: String,
        value: HValue,
    },
    Raw(HValue),
}

/// What [`annotate`] works on: the parse's heap and the node being annotated.
struct Annotated<'p> {
    heap: &'p mut Heap,
    built: &'p mut Built,
}

impl Annotated<'_> {
    /// `node.attributes` as an object of the parse's heap, moving the node there the first
    /// time (see [`annotate`]): its attributes and the annotations [`annotate_plain`] pushed
    /// so far, the `class` an annotation shares one object with it.
    fn attributes(&mut self) -> HValue {
        if let Some(id) = self.built.attributes {
            return HValue::Object(id);
        }
        let alias = self.built.class_alias.take();
        let node = self.built.node_mut();
        let attributes = std::mem::take(&mut node.attributes);
        let annotations = std::mem::take(&mut node.annotations);
        let id = self.heap.alloc(heap::Class::Object);
        let mut class = None;
        for (k, v) in attributes {
            let v = self.heap.alloc_scalar(&v);
            if k == "class" && alias.is_some() {
                class = Some(v.clone());
            }
            self.heap.define(id, k, v);
        }
        for (i, a) in annotations.into_iter().enumerate() {
            let value = match (alias, &class) {
                (Some(alias), Some(class)) if alias == i => class.clone(),
                _ => self.heap.alloc_scalar(&a.value),
            };
            let kind = if a.attr_type == "class" {
                grammar::AttributeType::Class
            } else {
                grammar::AttributeType::Attribute
            };
            self.built.annotations.push(Annotation::Grammar {
                kind,
                name: a.name,
                value,
            });
        }
        self.built.attributes = Some(id);
        HValue::Object(id)
    }
}

/// Does a value of the tag grammar hold an object with a prototype a `__proto__` key gave it
/// (a hash's later `__proto__`, a call's named `__proto__` parameter)?
fn has_prototype(value: &grammar::Value) -> bool {
    if !matches!(
        value,
        grammar::Value::Array(_)
            | grammar::Value::Object(_)
            | grammar::Value::Variable(_)
            | grammar::Value::Function(_)
    ) {
        return false;
    }
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            grammar::Value::Array(items) => stack.extend(items),
            grammar::Value::Object(o) => {
                if !matches!(o.prototype(), grammar::Prototype::ObjectPrototype) {
                    return true;
                }
                stack.extend(o.entries().into_iter().map(|(_, v)| v));
            }
            grammar::Value::Variable(var) => stack.extend(&var.path),
            grammar::Value::Function(f) => {
                if !matches!(
                    f.parameters.prototype(),
                    grammar::Prototype::ObjectPrototype
                ) {
                    return true;
                }
                stack.extend(f.parameters.entries().into_iter().map(|(_, v)| v));
            }
            _ => {}
        }
    }
    false
}

/// A value of the tag grammar that holds no prototype (see [`has_prototype`]) as the AST
/// holds it: what [`Scalar::from_json`] makes of its JSON, except that a number is the
/// grammar's own (an overlong literal is infinite, which JSON writes as `null`; `-0` stays
/// `-0`). Where the stack cannot grow ([`crate::depth::can_grow`]), deeper than
/// [`MAX_VALUE_DEPTH`] (an array, object, function or variable a level, a function's
/// parameters one deeper) is `RangeError`, as [`Heap::alloc_grammar`] counts; where it grows,
/// any depth (the grammar's depth budget bounds what it makes), a level at a time on a stack
/// with room for it.
fn grammar_scalar(value: &grammar::Value) -> Result<Scalar> {
    let limit = if crate::depth::can_grow() {
        usize::MAX
    } else {
        MAX_VALUE_DEPTH
    };
    grammar_scalar_at(value, 0, limit)
}

/// [`grammar_scalar`] of a value `depth` levels down.
fn grammar_scalar_at(value: &grammar::Value, depth: usize, limit: usize) -> Result<Scalar> {
    crate::depth::grow!(grammar_scalar_here(value, depth, limit))
}

#[inline(always)]
fn grammar_scalar_here(value: &grammar::Value, depth: usize, limit: usize) -> Result<Scalar> {
    let depth = match value {
        grammar::Value::Array(_)
        | grammar::Value::Object(_)
        | grammar::Value::Variable(_)
        | grammar::Value::Function(_) => depth + 1,
        _ => depth,
    };
    if depth > limit {
        return Err(stack_overflow());
    }
    let object = |o: &grammar::Object| -> Result<IndexMap<String, Scalar>> {
        let mut out = IndexMap::new();
        for (k, v) in o.entries() {
            js_insert(&mut out, k, grammar_scalar_at(v, depth, limit)?);
        }
        Ok(out)
    };
    Ok(match value {
        grammar::Value::Null => Scalar::Null,
        grammar::Value::Bool(b) => Scalar::Boolean(*b),
        grammar::Value::Number(n) => Scalar::Number(*n),
        grammar::Value::String(s) => Scalar::String(s.clone()),
        grammar::Value::Array(items) => Scalar::Array(
            items
                .iter()
                .map(|v| grammar_scalar_at(v, depth, limit))
                .collect::<Result<_>>()?,
        ),
        grammar::Value::Object(o) => Scalar::Object(object(o)?),
        grammar::Value::Variable(v) => Scalar::Variable(crate::ast::Variable::new(
            v.path
                .iter()
                .map(|step| match step {
                    grammar::Value::Number(n) => crate::ast::PathSegment::Number(*n),
                    grammar::Value::String(s) => crate::ast::PathSegment::String(s.clone()),
                    // The grammar's steps are identifiers, numbers and strings.
                    other => crate::ast::PathSegment::String(other.to_json_string()),
                })
                .collect(),
        )),
        grammar::Value::Function(f) => Scalar::Function(crate::ast::Function::new(
            f.name.clone(),
            object(&f.parameters)?,
        )),
    })
}

/// `object[name] !== undefined` for a plain object (its prototype `Object.prototype`).
fn js_defined(object: &IndexMap<String, Scalar>, name: &str) -> bool {
    match object.get(name) {
        Some(value) => !matches!(value, Scalar::Undefined),
        None => crate::next::schema::value::is_object_prototype_name(name),
    }
}

/// JS truthiness of an AST value.
fn scalar_truthy(value: &Scalar) -> bool {
    match value {
        Scalar::Undefined | Scalar::Null => false,
        Scalar::Boolean(b) => *b,
        Scalar::Number(n) => *n != 0.0 && !n.is_nan(),
        Scalar::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// One attribute the tag grammar made, annotated as [`annotate`] does, while the node's
/// attributes are plain data (no prototype; no object that two references share and a `.name`
/// could change): `Ok(false)`, with nothing done, where that no longer holds (an attribute
/// named `__proto__`, a value with a prototype, a `.name` onto a `class` that is an
/// annotation's object, an array, a variable or a call), and the heap takes over.
fn annotate_plain(
    node: &mut Annotated<'_>,
    attribute: &grammar::Attribute,
    span: Option<&Span>,
) -> Result<bool> {
    if has_prototype(&attribute.value) {
        return Ok(false);
    }
    let name = attribute.name.as_str();
    match attribute.kind {
        grammar::AttributeType::Attribute => {
            if name == "__proto__" {
                return Ok(false);
            }
            let value = grammar_scalar(&attribute.value)?;
            let index = node.built.node().annotations.len();
            let shared = matches!(
                value,
                Scalar::Object(_) | Scalar::Array(_) | Scalar::Variable(_) | Scalar::Function(_)
            );
            let n = node.built.node_mut();
            n.annotations.push(AttributeValue {
                attr_type: "attribute".into(),
                name: name.to_string(),
                value: value.clone(),
                raw: None,
            });
            if js_defined(&n.attributes, name) {
                n.errors.push(ValidationError {
                    id: "duplicate-attribute".into(),
                    level: ValidationLevel::Warning,
                    message: format!("Attribute '{name}' already set"),
                    location: None,
                    raw: None,
                    span: span.cloned(),
                });
            }
            js_append(&mut n.attributes, name, value);
            if name == "class" {
                node.built.class_alias = shared.then_some(index);
            }
        }
        grammar::AttributeType::Class => {
            let alias = node.built.class_alias;
            let n = node.built.node_mut();
            match n.attributes.get_mut("class") {
                Some(class) if scalar_truthy(class) => match class {
                    // A `class` the `.name`s made: no annotation holds it.
                    Scalar::Object(object) if alias.is_none() => {
                        // An own `__proto__` (a computed key's) is set; else the setter
                        // ignores `true`.
                        if name != "__proto__" || object.contains_key(name) {
                            js_append(object, name, Scalar::Boolean(true));
                        }
                    }
                    Scalar::String(_) | Scalar::Number(_) | Scalar::Boolean(_) => {
                        // `Object.prototype.__proto__`'s setter does nothing on a primitive;
                        // any other name throws in strict mode.
                        if name != "__proto__" {
                            let mut heap = Heap::new();
                            let primitive = heap.alloc_scalar(class);
                            heap.set(&primitive, name, HValue::Bool(true))
                                .map_err(MarkdocError::ParseError)?;
                        }
                    }
                    _ => return Ok(false),
                },
                _ => {
                    // `{ [name]: value }`: a new object no annotation holds (a computed key
                    // makes even `__proto__` an own property).
                    let mut class = IndexMap::new();
                    class.insert(name.to_string(), Scalar::Boolean(true));
                    js_insert(&mut n.attributes, "class", Scalar::Object(class));
                    node.built.class_alias = None;
                }
            }
            node.built.node_mut().annotations.push(AttributeValue {
                attr_type: "class".into(),
                name: name.to_string(),
                value: Scalar::Boolean(true),
                raw: None,
            });
        }
    }
    Ok(true)
}

/// `annotate(node, attributes)`. `span` is where the attributes come from, the
/// `duplicate-attribute` errors' [`ValidationError::span`] (this crate's addition).
///
/// Upstream pushes each attribute object itself onto `node.annotations` and assigns its value
/// to `node.attributes`, so the annotation whose `value` became `node.attributes.class` holds
/// the very object that later `.name` annotations change (on this call or on a later one, for
/// an inline parent): `class={a: 1} .b` leaves that annotation's value `{a: 1, b: true}`,
/// `class=[1, 2, 3] .length` leaves it `[1]`, `class=$v .b` gives the variable `b: true`. An
/// attribute named `__proto__` makes its value (an object or `null`) the attributes'
/// prototype, which every later read goes through (`node.attributes[name] !== undefined`,
/// `node.attributes.class`): `.name` can change an object the prototype holds. Every object
/// is in the parse's [`Heap`], and [`Parser::assemble`] makes the AST's values of them once
/// the tokens are parsed.
fn annotate(
    mut node: Annotated<'_>,
    attributes: &Value,
    values: Option<&[grammar::Attribute]>,
    span: Option<&Span>,
) -> Result<()> {
    let throw = MarkdocError::ParseError;
    // The grammar's own attributes when the token carries them (`attributes` is their JSON):
    // plain data stays plain while it can ([`annotate_plain`], which is all a document
    // written by hand needs), and goes to the heap from the first attribute that needs it.
    if let Some(values) = values {
        for a in values {
            if node.built.attributes.is_none() && annotate_plain(&mut node, a, span)? {
                continue;
            }
            node.attributes();
            let value = heap_from_grammar(node.heap, &a.value)?;
            let annotation = Annotation::Grammar {
                kind: a.kind,
                name: a.name.clone(),
                value,
            };
            annotate_heap(&mut node, annotation, span)?;
        }
        return Ok(());
    }
    // `for (const attribute of attributes)`: an array, or a string's characters (by code
    // point); anything else throws (V8 names the variable, whatever its value).
    let list: Vec<Annotation> = match attributes {
        Value::Array(items) => items
            .iter()
            .map(|a| {
                // An attribute object's members are as deep as the grammar nests its values
                // (the object itself is no level of them); any depth where the stack grows.
                match a {
                    Value::Object(members) => {
                        if !crate::depth::can_grow() {
                            members.values().try_for_each(check_value_depth)?;
                        }
                        Ok(Annotation::Raw(
                            node.heap.alloc_json(a).map_err(MarkdocError::ParseError)?,
                        ))
                    }
                    _ => Ok(Annotation::Raw(heap_from_json(node.heap, a)?)),
                }
            })
            .collect::<Result<_>>()?,
        Value::String(s) => s
            .chars()
            .map(|c| Annotation::Raw(HValue::String(c.to_string())))
            .collect(),
        _ => {
            return Err(throw("TypeError: attributes is not iterable".into()));
        }
    };
    node.attributes();
    for annotation in list {
        annotate_heap(&mut node, annotation, span)?;
    }
    Ok(())
}

/// One annotation, on the heap (see [`annotate`]).
fn annotate_heap(
    node: &mut Annotated<'_>,
    annotation: Annotation,
    span: Option<&Span>,
) -> Result<()> {
    let throw = MarkdocError::ParseError;
    {
        // `node.annotations.push(attribute); const { name, value, type } = attribute;`
        let (name, value, kind) = match &annotation {
            Annotation::Grammar { kind, name, value } => (
                HValue::String(name.clone()),
                value.clone(),
                HValue::String(kind.as_str().into()),
            ),
            Annotation::Raw(attribute) => {
                if matches!(attribute, HValue::Null) {
                    node.built.annotations.push(annotation);
                    return Err(throw(
                        "TypeError: Cannot destructure property 'name' of 'attribute' as it is null."
                            .into(),
                    ));
                }
                let field = |key: &str| match node.heap.lookup(attribute, key) {
                    HFound::Value(v) => v,
                    // No built-in prototype has a `name`, `value` or `type`.
                    HFound::Builtin(..) => HValue::Undefined,
                };
                (field("name"), field("value"), field("type"))
            }
        };
        node.built.annotations.push(annotation);
        let kind = match &kind {
            HValue::String(kind) => kind.as_str(),
            _ => "",
        };
        if kind == "attribute" {
            let attributes = node.attributes();
            let key = node.heap.to_property_key(&name).map_err(throw)?;
            if !node.heap.lookup(&attributes, &key).is_undefined() {
                let message = format!(
                    "Attribute '{}' already set",
                    node.heap.display(&name).map_err(throw)?
                );
                node.built.node_mut().errors.push(ValidationError {
                    id: "duplicate-attribute".into(),
                    level: ValidationLevel::Warning,
                    message,
                    location: None,
                    raw: None,
                    span: span.cloned(),
                });
            }
            node.heap.set(&attributes, &key, value).map_err(throw)?;
        } else if kind == "class" {
            let attributes = node.attributes();
            // `if (node.attributes.class) node.attributes.class[name] = value; else
            // node.attributes.class = { [name]: value };`
            match node.heap.lookup(&attributes, "class") {
                HFound::Value(class) if heap::truthy(&class) => {
                    let key = node.heap.to_property_key(&name).map_err(throw)?;
                    node.heap.set(&class, &key, value).map_err(throw)?;
                }
                // Falsy, or not there (no built-in prototype has a `class`).
                _ => {
                    let key = node.heap.to_property_key(&name).map_err(throw)?;
                    let class = node.heap.alloc(heap::Class::Object);
                    node.heap.define(class, key, value);
                    node.heap
                        .set(&attributes, "class", HValue::Object(class))
                        .map_err(throw)?;
                }
            }
        }
    }
    Ok(())
}

/// `Object.fromEntries(token.attrs ?? [])`.
fn attrs_object(token: &Token) -> IndexMap<String, &AttrValue> {
    let mut object = IndexMap::new();
    for (name, value) in token.attrs.iter().flatten() {
        js_insert(&mut object, name.as_str(), value);
    }
    object
}

fn attr_scalar(value: &AttrValue) -> Scalar {
    match value {
        AttrValue::Str(s) => Scalar::String(s.clone()),
        AttrValue::Num(n) => Scalar::Number(*n),
    }
}

fn attr_truthy(value: Option<&&AttrValue>) -> bool {
    match value {
        None => false,
        Some(AttrValue::Str(s)) => !s.is_empty(),
        Some(AttrValue::Num(n)) => *n != 0.0 && !n.is_nan(),
    }
}

/// `handleAttrs(token, type)`.
fn handle_attrs(
    token: &Token,
    type_name: &str,
    heap: &mut Heap,
    memo: &mut Memo,
) -> Result<IndexMap<String, Scalar>> {
    let mut out = IndexMap::new();
    match type_name {
        "heading" => {
            // heading_open tokens always have `tag` set (e.g. "h1"-"h6")
            let level = token.tag.replacen('h', "", 1);
            out.insert(
                "level".into(),
                Scalar::Number(js::string_to_number(&js::to_utf16(&level))),
            );
        }
        "list" => {
            let attrs = token.attrs.as_ref().map(|_| attrs_object(token));
            let ordered = token.kind.starts_with("ordered");
            let start = attrs.as_ref().and_then(|a| a.get("start"));
            out.insert("ordered".into(), Scalar::Boolean(ordered));
            if ordered && attr_truthy(start) {
                out.insert("start".into(), attr_scalar(start.expect("truthy")));
            }
            out.insert("marker".into(), Scalar::String(token.markup.clone()));
        }
        "link" => {
            // `{href: attrs.href}`: `undefined` for a link node made of a closing token,
            // which has no `attrs` (`validate()` sees the key).
            let attrs = attrs_object(token);
            out.insert(
                "href".into(),
                attrs
                    .get("href")
                    .map_or(Scalar::Undefined, |v| attr_scalar(v)),
            );
            if let Some(title) = attrs.get("title").filter(|t| attr_truthy(Some(t))) {
                out.insert("title".into(), attr_scalar(title));
            }
        }
        "image" => {
            let attrs = attrs_object(token);
            out.insert("alt".into(), Scalar::String(token.content.clone()));
            out.insert(
                "src".into(),
                attrs
                    .get("src")
                    .map_or(Scalar::Undefined, |v| attr_scalar(v)),
            );
            if let Some(title) = attrs.get("title").filter(|t| attr_truthy(Some(t))) {
                out.insert("title".into(), attr_scalar(title));
            }
        }
        "em" | "strong" => {
            out.insert("marker".into(), Scalar::String(token.markup.clone()));
        }
        "text" | "code" | "comment" => {
            // `(token.meta || {}).variable || token.content`
            let variable = token
                .meta
                .as_ref()
                .and_then(|m| m.get("variable"))
                .filter(|v| json_truthy(v));
            let content = match (&token.grammar_meta, variable) {
                // The grammar's own value when the token carries it (`meta` is its JSON).
                (Some(meta), _) => match meta.variable.as_ref().filter(|v| v.truthy()) {
                    // Plain data as it is; a value with a prototype through the heap.
                    Some(v) if !has_prototype(v) => grammar_scalar(v)?,
                    Some(v) => {
                        let v = heap_from_grammar(heap, v)?;
                        heap.snapshot(&v, memo)
                    }
                    None => Scalar::String(token.content.clone()),
                },
                (None, Some(v)) => {
                    let v = heap_from_json(heap, v)?;
                    heap.snapshot(&v, memo)
                }
                (None, None) => Scalar::String(token.content.clone()),
            };
            out.insert("content".into(), content);
        }
        "fence" => {
            // fence tokens always have `info` set (empty string if unused)
            let language = token.info.split(' ').next().unwrap_or_default();
            out.insert("content".into(), Scalar::String(token.content.clone()));
            if !(language.is_empty() || language == "{%") {
                out.insert("language".into(), Scalar::String(language.to_string()));
            }
        }
        // `if (token.attrs) {...} return {};`
        "td" | "th" if token.attrs.is_some() => {
            let attrs = attrs_object(token);
            let mut align = None;
            if let Some(style) = attrs.get("style").filter(|s| attr_truthy(Some(s))) {
                let style = style.to_js_string();
                if style.contains("left") {
                    align = Some("left");
                } else if style.contains("center") {
                    align = Some("center");
                } else if style.contains("right") {
                    align = Some("right");
                }
            }
            if let Some(align) = align {
                out.insert("align".into(), Scalar::String(align.into()));
            }
        }
        _ => {}
    }
    Ok(out)
}

/// `token.type.replace(/_(open|close)$/, '')`.
fn strip_nesting_suffix(kind: &str) -> &str {
    kind.strip_suffix("_open")
        .or_else(|| kind.strip_suffix("_close"))
        .unwrap_or(kind)
}

/// What reading `property` of `parent` throws when `parent` is `undefined` (see
/// [`Parser::top`]).
fn no_parent(property: &str) -> MarkdocError {
    MarkdocError::ParseError(format!(
        "TypeError: Cannot read properties of undefined (reading '{property}')"
    ))
}

/// An entry of a token's `errors` as upstream's node holds it: the value itself. Markdoc's
/// own plugins put `{id, level, message}` there, which the [`ValidationError`]'s fields give
/// whole; a hand-made token may put any value, which the error then keeps as its
/// [`ValidationError::raw`] (its fields what `String()` makes of the value's members). Also
/// how [`crate::next::compare::node_from_json`] reads an AST's errors. Nested deeper than
/// [`MAX_VALUE_DEPTH`] it is `RangeError`, as a deep `meta` value is.
pub(crate) fn error_from_json(value: &Value) -> Result<ValidationError> {
    let value = passthrough(value)?;
    let field = |name: &str| value.as_object().and_then(|o| o.get(name));
    let level = field("level")
        .and_then(Value::as_str)
        .and_then(ValidationLevel::from_name);
    let plain = match &value {
        Value::Object(o) => {
            let keys: Vec<&str> = o.keys().map(String::as_str).collect();
            matches!(
                keys.as_slice(),
                ["id", "level", "message"] | ["id", "level", "message", "location"]
            ) && o["id"].is_string()
                && o["message"].is_string()
                && level.is_some()
        }
        _ => false,
    };
    let text = |name: &str| field(name).map(json_string).unwrap_or_default();
    Ok(ValidationError {
        id: text("id"),
        level: level.unwrap_or(ValidationLevel::Error),
        message: text("message"),
        location: field("location").cloned(),
        raw: (!plain).then(|| Box::new(value.clone())),
        span: None,
    })
}

/// A value upstream's parser copies from a token into the AST as it is (a `position`'s
/// `start` or `end`, a syntax error's `location`): the same JSON, its objects' keys in a JS
/// object's order (array-index keys first, ascending). Nested deeper than
/// [`MAX_VALUE_DEPTH`] it is `RangeError`, as a deep `meta` value is (only a hand-made
/// token holds one).
fn passthrough(value: &Value) -> Result<Value> {
    // The arrays and objects still to look into, with how deep each is.
    let mut stack = vec![(value, 0)];
    while let Some((value, depth)) = stack.pop() {
        let items: Box<dyn Iterator<Item = &Value>> = match value {
            Value::Array(items) => Box::new(items.iter()),
            Value::Object(fields) => Box::new(fields.values()),
            _ => continue,
        };
        if depth + 1 > MAX_VALUE_DEPTH {
            return Err(stack_overflow());
        }
        stack.extend(items.map(|v| (v, depth + 1)));
    }
    Ok(js_ordered(value))
}

/// `value` with each object's keys in a JS object's order (see [`passthrough`], which bounds
/// the depth this recurses to).
fn js_ordered(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(js_ordered).collect()),
        Value::Object(fields) => {
            let mut object: IndexMap<String, Value> = fields
                .iter()
                .map(|(k, v)| (k.clone(), js_ordered(v)))
                .collect();
            js_order(&mut object);
            Value::Object(object.into_iter().collect())
        }
        other => other.clone(),
    }
}

/// A node while the tree is being built: upstream attaches a node to its parent when it
/// makes it and keeps adding to it through references; here the nodes live in an arena and
/// refer to each other by index, and the tree is assembled at the end (each node moved
/// once, into its parent).
struct Built {
    /// The node (`None` once [`Parser::assemble`] has moved it into its parent).
    node: Option<Node>,
    children: Vec<usize>,
    slots: IndexMap<String, usize>,
    /// Whose `lines` the node's are: `None` its own ([`Node::lines`], which a closing grows
    /// in place), else the node that owns the array it shares (upstream's `node.lines =
    /// parent.lines`; [`Parser::assemble`] copies the owner's array as it ends up).
    shares: Option<usize>,
    /// The node a slot named `__proto__` made the slots' prototype (`parent.slots[name] =
    /// node` calls `Object.prototype`'s setter): in no list, so nothing walks it.
    slots_proto: Option<usize>,
    /// `node.attributes` once an annotation is on the node: an object of the parse's heap,
    /// which [`Parser::assemble`] makes the node's attributes of.
    attributes: Option<heap::Id>,
    /// The node's annotations once it is on the heap, made the AST's at
    /// [`Parser::assemble`].
    annotations: Vec<Annotation>,
    /// While the node is not on the heap: the annotation whose value is the object, array,
    /// variable or call `node.attributes.class` holds (a `.name` onto it moves the node to
    /// the heap, where the two are one object).
    class_alias: Option<usize>,
}

impl Built {
    fn new(node: Node, shares: Option<usize>) -> Built {
        Built {
            node: Some(node),
            children: Vec::new(),
            slots: IndexMap::new(),
            shares,
            slots_proto: None,
            attributes: None,
            annotations: Vec::new(),
            class_alias: None,
        }
    }

    fn node(&self) -> &Node {
        self.node
            .as_ref()
            .expect("a node is assembled only at the end")
    }

    fn node_mut(&mut self) -> &mut Node {
        self.node
            .as_mut()
            .expect("a node is assembled only at the end")
    }
}

/// A token's children while [`Parser::handle_tree`] handles them.
struct Walk<'t> {
    children: &'t [Token],
    spans: Vec<Extent>,
    /// Where a child without a span of its own sits: where its parent ends.
    end: usize,
    /// The child to handle next.
    next: usize,
    inline_parent: Option<usize>,
}

impl<'t> Walk<'t> {
    /// The next child to handle, and its span.
    fn next(&mut self) -> Option<(&'t Token, Extent)> {
        let child = self.children.get(self.next)?;
        let span = self
            .spans
            .get(self.next)
            .copied()
            .unwrap_or(Extent::at(self.end));
        self.next += 1;
        Some((child, span))
    }
}

struct Parser<'a> {
    arena: Vec<Built>,
    /// Upstream's `nodes` stack.
    nodes: Vec<usize>,
    file: Option<String>,
    /// `file` again, shared by every node's upstream [`Location`] (one string, as JS has).
    shared_file: Option<Arc<str>>,
    handle_slots: bool,
    add_location: bool,
    parse_frontmatter: bool,
    /// This crate's spans: where each token is, and line and column of an offset; `None`
    /// unless [`ParserArgs::spans`] (and `location`) are on, when nothing has a span.
    positions: Option<Positions<'a>>,
    index: Option<LineIndex>,
    source: &'a str,
    /// How deep a token may sit in `children`: [`MAX_TOKEN_NESTING`] from the top of a
    /// thread, what is left of the depth budget at [`parse`].
    max_token_level: usize,
    /// The objects attributes and values are while the tokens are parsed.
    heap: Heap,
    /// The AST's values made of the heap's objects so far.
    memo: Memo,
}

impl<'a> Parser<'a> {
    /// This crate's [`Span`] of a byte extent of the source (`None` unless spans are on).
    fn span_of(&self, extent: Extent) -> Option<Span> {
        let index = self.index.as_ref()?;
        let edge = |offset: usize| {
            let (line, character) = index.line_col(self.source, offset);
            Position {
                line,
                character: Some(character),
                offset,
            }
        };
        Some(Span {
            file: self.file.clone(),
            start: edge(extent.start),
            end: edge(extent.end.max(extent.start)),
        })
    }

    /// The node that owns the `lines` array node `index` holds (see [`Built::shares`]):
    /// upstream's nodes share arrays (`node.lines = token.map || parent.lines || []`), and a
    /// closing token appends its map to the array of the node it closes, so to every node
    /// that shares it.
    fn lines_owner(&self, index: usize) -> usize {
        self.arena[index].shares.unwrap_or(index)
    }

    /// Upstream's `location` of a node whose `lines` are `lines` as they are now: `{file,
    /// start: {line: lines[0], character: position.start}, end: {line: lines[1], character:
    /// position.end}}`, where `position` is the token's (`{}` when it has none; `null`
    /// throws, as reading a property of it does).
    fn location(&self, lines: &[usize], token: &Token) -> Result<Location> {
        let (start, end) = match token.position.as_deref() {
            None => (None, None),
            Some(Value::Null) => {
                return Err(MarkdocError::ParseError(
                    "TypeError: Cannot read properties of null (reading 'start')".into(),
                ));
            }
            Some(position) => (
                position
                    .get("start")
                    .map(passthrough)
                    .transpose()?
                    .map(Box::new),
                position
                    .get("end")
                    .map(passthrough)
                    .transpose()?
                    .map(Box::new),
            ),
        };
        Ok(Location {
            file: self.shared_file.clone(),
            start: LocationEdge {
                line: lines.first().copied(),
                character: start,
            },
            end: LocationEdge {
                line: lines.get(1).copied(),
                character: end,
            },
        })
    }

    /// `nodes[nodes.length - 1]`, which is `undefined` once a hand-made `document_close` has
    /// popped the document: then the first property upstream reads of it throws V8's
    /// `TypeError` naming that property (see [`no_parent`]).
    fn top(&self) -> Option<usize> {
        self.nodes.last().copied()
    }

    /// The document's `frontmatter`: upstream's raw string; with `parse_frontmatter`, the YAML
    /// parsed to an object, or the raw string and an error.
    fn frontmatter(&self, token: &Token, span: Extent) -> (Scalar, Option<ValidationError>) {
        if !self.parse_frontmatter {
            return (Scalar::String(token.content.clone()), None);
        }
        let yaml = positions::frontmatter_text(self.source, token)
            .unwrap_or_else(|| token.content.clone());
        match serde_yaml::from_str::<IndexMap<String, serde_yaml::Value>>(&yaml) {
            Ok(mapping) => {
                let mut object = IndexMap::new();
                for (k, v) in mapping {
                    js_append(&mut object, &k, yaml_to_scalar(v));
                }
                js_order(&mut object);
                (Scalar::Object(object), None)
            }
            Err(e) => (
                Scalar::String(token.content.clone()),
                Some(ValidationError {
                    id: "frontmatter-invalid".into(),
                    level: ValidationLevel::Error,
                    message: format!("Invalid YAML frontmatter: {e}"),
                    location: None,
                    raw: None,
                    span: self.span_of(span),
                }),
            ),
        }
    }

    /// `parent.push(node)` or, for a slot, `parent.slots[name] = node`, at the node's making.
    fn attach(&mut self, parent: usize, built: Built, slot: Option<String>) -> usize {
        let index = self.arena.len();
        self.arena.push(built);
        let parent = &mut self.arena[parent];
        match slot {
            // `parent.slots.__proto__ = node`: `Object.prototype`'s setter (the slots' chain
            // never has an own `__proto__`), so the node becomes the slots' prototype.
            Some(name) if name == "__proto__" => parent.slots_proto = Some(index),
            Some(name) => js_append(&mut parent.slots, &name, index),
            None => parent.children.push(index),
        }
        index
    }

    /// `handleToken(token, nodes, ...)` of a top-level token, and so of every token in its
    /// `children`, in upstream's order. Upstream recurses into `children`; here the tokens
    /// whose children are being walked are a stack on the heap, so a hand-made token nested
    /// to any depth costs no stack. A token more than [`MAX_TOKEN_NESTING`] levels down (from
    /// the top of a thread; see `max_token_level`) is where upstream's recursion runs out of
    /// stack: `RangeError`, before it is handled.
    fn handle_tree(
        &mut self,
        token: &Token,
        span: Extent,
        child_spans: Option<Vec<Extent>>,
    ) -> Result<()> {
        let mut open: Vec<Walk> = Vec::new();
        open.extend(self.handle_token(token, span, child_spans, None)?);
        loop {
            // The level of the next child: 1 for a top-level token's.
            let level = open.len();
            let Some(walk) = open.last_mut() else {
                return Ok(());
            };
            let Some((child, child_span)) = walk.next() else {
                // `nodes.pop()` after the children: whatever is on top.
                open.pop();
                self.nodes.pop();
                continue;
            };
            let inline_parent = walk.inline_parent;
            if level > self.max_token_level {
                return Err(stack_overflow());
            }
            // A child's own children (none that upstream walks) sit where it does (with
            // spans on; off, no extent is read).
            let grandchildren = child
                .children
                .as_ref()
                .filter(|_| self.positions.is_some())
                .map(|c| vec![Extent::at(child_span.start); c.len()]);
            open.extend(self.handle_token(child, child_span, grandchildren, inline_parent)?);
        }
    }

    /// `handleToken(token, nodes, file, handleSlots, addLocation, inlineParent)` up to its
    /// children: when it has any to walk, it has pushed its node (`nodes.push(node)`) and
    /// returns them, for [`Parser::handle_tree`] to handle each and pop the stack after.
    fn handle_token<'t>(
        &mut self,
        token: &'t Token,
        span: Extent,
        child_spans: Option<Vec<Extent>>,
        mut inline_parent: Option<usize>,
    ) -> Result<Option<Walk<'t>>> {
        if token.kind == "frontmatter" {
            let (value, error) = self.frontmatter(token, span);
            let document = *self.nodes.first().ok_or_else(|| {
                MarkdocError::ParseError(
                    "TypeError: Cannot read properties of undefined (reading 'attributes')".into(),
                )
            })?;
            // `nodes[0].attributes.frontmatter = token.content`: through the heap once an
            // annotation made the attributes an object of it (a hand-made inline token at the
            // top level annotates the document).
            let built = &mut self.arena[document];
            if let Some(id) = built.attributes {
                let value = self.heap.alloc_scalar(&value);
                self.heap
                    .set(&HValue::Object(id), "frontmatter", value)
                    .map_err(MarkdocError::ParseError)?;
            } else {
                js_insert(&mut built.node_mut().attributes, "frontmatter", value);
            }
            built.node_mut().errors.extend(error);
            return Ok(None);
        }

        if token.hidden || (token.kind == "text" && token.content.is_empty()) {
            return Ok(None);
        }

        let mut errors: Vec<ValidationError> = token
            .errors
            .iter()
            .flatten()
            .map(error_from_json)
            .collect::<Result<_>>()?;
        let parent = self.top();
        let meta = token.meta.as_ref().and_then(Value::as_object);
        let field = |name: &str| meta.and_then(|m: &Map<String, Value>| m.get(name));
        let (tag, attributes, error) = (field("tag"), field("attributes"), field("error"));
        // This crate's span of the token, for its node and the errors about it.
        let here = self.span_of(span);

        // The grammar's own attribute values, when the token carries them.
        let values = token
            .grammar_meta
            .as_ref()
            .and_then(|m| m.attributes.as_ref())
            .and_then(|a| a.as_deref());

        if token.kind == "annotation" {
            let attributes = attributes.unwrap_or(&Value::Null);
            if let Some(inline_parent) = inline_parent {
                annotate(
                    Annotated {
                        heap: &mut self.heap,
                        built: &mut self.arena[inline_parent],
                    },
                    attributes,
                    values,
                    here.as_ref(),
                )?;
                return Ok(None);
            }
            let parent = self.arena[parent.ok_or_else(|| no_parent("errors"))?].node_mut();
            parent.errors.push(ValidationError {
                id: "no-inline-annotations".into(),
                level: ValidationLevel::Error,
                message: format!(
                    "Can't apply inline annotations to '{}'",
                    parent.node_type.as_str()
                ),
                location: None,
                raw: None,
                span: here,
            });
            return Ok(None);
        }

        let mut type_name = strip_nesting_suffix(&token.kind);
        if let Some(mapped) = mapping(type_name) {
            type_name = mapped;
        }

        if type_name == "error" {
            // `const { message, location } = error`, where the dist's bundler named the
            // variable `error3` (`const { tag, attributes, error: error3 } = token.meta || {}`),
            // which V8's message names. `null` is "as it is null.", as for any destructuring.
            let error = match error {
                Some(Value::Null) => {
                    return Err(MarkdocError::ParseError(
                        "TypeError: Cannot destructure property 'message' of 'error3' as it is \
                         null."
                            .into(),
                    ));
                }
                Some(error) => error,
                None => {
                    return Err(MarkdocError::ParseError(
                        "TypeError: Cannot destructure property 'message' of 'error3' as it is \
                         undefined."
                            .into(),
                    ));
                }
            };
            // `const { message, location } = error`: the tokenizer's `location` (`null`, or
            // UTF-16 offsets into the source, or a line and character in a fence) as it is.
            // `errors.push({ id: 'parse-error', level: 'critical', message, location })`: a
            // `message` that is not a string (only a hand-made `meta.error` has one) makes the
            // error one its fields cannot give whole, so it keeps its value as `raw`.
            let message = error.get("message").map(passthrough).transpose()?;
            let location = error.get("location").map(passthrough).transpose()?;
            let raw = match &message {
                Some(Value::String(_)) => None,
                _ => {
                    let mut raw = Map::new();
                    raw.insert("id".into(), "parse-error".into());
                    raw.insert("level".into(), "critical".into());
                    raw.extend(message.iter().map(|m| ("message".to_string(), m.clone())));
                    raw.extend(location.iter().map(|l| ("location".to_string(), l.clone())));
                    Some(Box::new(Value::Object(raw)))
                }
            };
            errors.push(ValidationError {
                id: "parse-error".into(),
                level: ValidationLevel::Critical,
                message: message.as_ref().map(json_string).unwrap_or_default(),
                location,
                raw,
                span: here.clone(),
            });
        }

        if token.nesting < 0 {
            let parent = parent.ok_or_else(|| no_parent("type"))?;
            let open = self.arena[parent].node();
            let same_tag = match (&open.tag, tag) {
                (None, None) => true,
                (Some(a), Some(Value::String(b))) => a == b,
                _ => false,
            };
            if open.node_type.as_str() == type_name && same_tag {
                let is_tag = open.node_type == NodeType::Tag;
                // `if (parent.lines && token.map) parent.lines.push(...token.map)`, onto the
                // array the node shares (an array is always truthy).
                if let Some(map) = token.map {
                    let owner = self.lines_owner(parent);
                    self.arena[owner].node_mut().lines.extend(map);
                }
                // This crate's: a node other than a tag grows to the end of what closes it
                // (a tag spans its own `{% %}` alone).
                if !is_tag
                    && let (Some(closing), Some(own)) =
                        (here, self.arena[parent].node_mut().span.as_mut())
                    && closing.end.offset > own.end.offset
                {
                    own.end = closing.end;
                }
                self.nodes.pop();
                return Ok(None);
            }

            errors.push(ValidationError {
                id: "missing-opening".into(),
                level: ValidationLevel::Critical,
                message: format!("Node '{type_name}' is missing opening"),
                location: None,
                raw: None,
                span: here.clone(),
            });
        }

        let attrs = handle_attrs(token, type_name, &mut self.heap, &mut self.memo)?;
        // `tag || undefined`
        let node_tag = match tag {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(other) if json_truthy(other) => Some(json_string(other)),
            _ => None,
        };
        // `new Node(type, attrs, undefined, tag)`, the attributes kept as they are made.
        let mut node = Node::new(
            NodeType::from_name(type_name),
            std::iter::empty(),
            Vec::new(),
            node_tag,
        );
        node.attributes = attrs;
        node.errors = errors;
        // `node.lines = token.map || parent.lines || []` and `node.location = {...}`, unless
        // `location: false`, when the node keeps the array it was made with, its own.
        let mut shares = None;
        if self.add_location {
            match token.map {
                Some(map) => {
                    node.lines = map.to_vec();
                    node.location = Some(match token.position {
                        // The common case, without a detour: the token's own map, no position.
                        None => Location {
                            file: self.shared_file.clone(),
                            start: LocationEdge {
                                line: Some(map[0]),
                                character: None,
                            },
                            end: LocationEdge {
                                line: Some(map[1]),
                                character: None,
                            },
                        },
                        Some(_) => self.location(&node.lines, token)?,
                    });
                }
                None => {
                    let owner = self.lines_owner(parent.ok_or_else(|| no_parent("lines"))?);
                    node.location = Some(self.location(&self.arena[owner].node().lines, token)?);
                    shares = Some(owner);
                }
            }
        }

        if inline_parent.is_some() {
            node.inline = true;
        }

        let mut built = Built::new(node, shares);
        if let Some(attributes) = attributes
            && json_truthy(attributes)
            && ["tag", "fence", "image"].contains(&type_name)
        {
            annotate(
                Annotated {
                    heap: &mut self.heap,
                    built: &mut built,
                },
                attributes,
                values,
                here.as_ref(),
            )?;
        }

        // `handleSlots && tag === 'slot' && typeof node.attributes.primary === 'string'`.
        let slot = match tag {
            Some(Value::String(t)) if self.handle_slots && t == "slot" => match built.attributes {
                Some(id) => match self.heap.lookup(&HValue::Object(id), "primary") {
                    HFound::Value(HValue::String(primary)) => Some(primary),
                    _ => None,
                },
                None => match built.node().attributes.get("primary") {
                    Some(Scalar::String(primary)) => Some(primary.clone()),
                    _ => None,
                },
            },
            _ => None,
        };
        // `parent.slots[...] = node` or `parent.push(node)`.
        let parent =
            parent.ok_or_else(|| no_parent(if slot.is_some() { "slots" } else { "push" }))?;
        // This crate's span of the node (moved: nothing reads `here` after this).
        built.node_mut().span = here;
        let index = self.attach(parent, built, slot);

        if token.nesting > 0 {
            self.nodes.push(index);
        }

        let Some(children) = &token.children else {
            return Ok(None);
        };

        if type_name == "inline" {
            inline_parent = Some(parent);
        }

        // `nodes.push(node)`, the children unless the node is a leaf, `nodes.pop()`: an
        // image's push and pop leave the stack as it was.
        let is_leaf_node = type_name == "image";
        if is_leaf_node {
            return Ok(None);
        }
        self.nodes.push(index);
        Ok(Some(Walk {
            children,
            spans: child_spans.unwrap_or_default(),
            end: span.end,
            next: 0,
            inline_parent,
        }))
    }

    /// The tree from the arena: each node with its children and slots, from the document
    /// down. Children are always made after their parent, so going from the last node back
    /// finds every child whole; each node is moved once, into its parent.
    fn assemble(mut self) -> Node {
        let Parser {
            arena, heap, memo, ..
        } = &mut self;
        for index in (0..arena.len()).rev() {
            // A node that shares another's `lines` holds them as every closing left them. The
            // owner was made before the node, so it is still here (a node leaves the arena
            // for its parent, which was made before it, after it).
            if let Some(owner) = arena[index].shares {
                let lines = arena[owner].node().lines.clone();
                arena[index].node_mut().lines = lines;
            }
            let built = &mut arena[index];
            // The tokens are parsed: nothing changes an attribute any more. The AST's values
            // are made of the heap's objects, an object shared between references (an
            // annotation and the attribute it set, a prototype) one value.
            if let Some(node) = built.node.as_mut() {
                if let Some(id) = built.attributes {
                    let (attributes, prototype) = heap.snapshot_attributes(id, memo);
                    node.attributes = attributes;
                    node.attributes_prototype = prototype;
                    node.annotations = std::mem::take(&mut built.annotations)
                        .into_iter()
                        .map(|a| annotation_value(heap, a, memo))
                        .collect();
                } else {
                    // Plain data (see `annotate_plain`): the objects `js_append` built take
                    // a JS object's key order.
                    js_order(&mut node.attributes);
                    if let Some(Scalar::Object(class)) = node.attributes.get_mut("class") {
                        js_order(class);
                    }
                }
            }
            js_order(&mut built.slots);
            let slots_proto = built.slots_proto.take();
            if built.children.is_empty() && built.slots.is_empty() && slots_proto.is_none() {
                continue;
            }
            let (children, slots) = (
                std::mem::take(&mut built.children),
                std::mem::take(&mut built.slots),
            );
            // Room for every child up front: collecting the `filter_map` would start at 4 and
            // double, moving every node at each step and leaving up to half the capacity unused.
            let children: Vec<Node> = {
                let mut nodes = Vec::with_capacity(children.len());
                nodes.extend(children.into_iter().filter_map(|c| arena[c].node.take()));
                nodes
            };
            let slots: IndexMap<String, Node> = slots
                .into_iter()
                .filter_map(|(name, s)| arena[s].node.take().map(|n| (name, n)))
                .collect();
            let slots_prototype = slots_proto.and_then(|p| arena[p].node.take());
            if let Some(node) = arena[index].node.as_mut() {
                node.children = children;
                node.slots = slots;
                node.slots_prototype = slots_prototype.map(Box::new);
            }
        }
        arena[0].node.take().expect("the document")
    }
}

/// The AST's annotation of one `annotate` pushed: the attribute object the grammar made, or a
/// hand-made one that is such an object (`{type, name, value}` or `{type, name}`, strings,
/// in that order, a plain object), as its fields; anything else as it is, in `raw`.
fn annotation_value(heap: &Heap, annotation: Annotation, memo: &mut Memo) -> AttributeValue {
    match annotation {
        Annotation::Grammar { kind, name, value } => AttributeValue {
            attr_type: kind.as_str().into(),
            name,
            value: heap.snapshot(&value, memo),
            raw: None,
        },
        Annotation::Raw(attribute) => {
            if let Some((attr_type, name, value)) = heap.attribute_object(&attribute) {
                return AttributeValue {
                    attr_type,
                    name,
                    value: heap.snapshot(&value, memo),
                    raw: None,
                };
            }
            let field = |key: &str| match &attribute {
                HValue::Object(_) => match heap.lookup(&attribute, key) {
                    HFound::Value(v) => v,
                    HFound::Builtin(..) => HValue::Undefined,
                },
                _ => HValue::Undefined,
            };
            let text = |v: HValue| match v {
                HValue::Undefined => String::new(),
                v => heap.display(&v).unwrap_or_default(),
            };
            AttributeValue {
                attr_type: text(field("type")),
                name: text(field("name")),
                value: heap.snapshot(&field("value"), memo),
                raw: Some(Box::new(heap.snapshot(&attribute, memo))),
            }
        }
    }
}

/// YAML to a value, as the old parser converts frontmatter (`crate::frontmatter`): numbers
/// as `f64`, mappings keyed by strings (other keys dropped), tagged values as null. A
/// mapping's keys are in a JS object's order (as a JS YAML loader would make them).
fn yaml_to_scalar(value: serde_yaml::Value) -> Scalar {
    match value {
        serde_yaml::Value::Null => Scalar::Null,
        serde_yaml::Value::Bool(b) => Scalar::Boolean(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Scalar::Number(i as f64)
            } else if let Some(f) = n.as_f64() {
                Scalar::Number(f)
            } else {
                Scalar::Null
            }
        }
        serde_yaml::Value::String(s) => Scalar::String(s),
        serde_yaml::Value::Sequence(seq) => {
            Scalar::Array(seq.into_iter().map(yaml_to_scalar).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut object = IndexMap::new();
            for (k, v) in map {
                if let serde_yaml::Value::String(key) = k {
                    js_append(&mut object, &key, yaml_to_scalar(v));
                }
            }
            js_order(&mut object);
            Scalar::Object(object)
        }
        serde_yaml::Value::Tagged(_) => Scalar::Null,
    }
}

/// A value of upstream's AST JSON as `Ast.fromJSON` (`JSON.parse` with its reviver) makes it:
/// an object whose `$$mdtype` names `Variable`, `Function` or `Node` is that class's instance
/// with the object's properties assigned (`Object.assign(new klass(), value)`: an own
/// `__proto__` key sets the instance's prototype), any other object a plain one (an own
/// `__proto__` key is an own property, as `JSON.parse` makes it), as the AST holds them (the
/// plain [`Scalar`] variants where they can, else [`Scalar::Js`]). `Err` where the reviver
/// throws (`TypeError: klass is not a constructor` for a `$$mdtype` that names a method every
/// object inherits).
pub fn revive(value: &Value) -> std::result::Result<Scalar, String> {
    let mut heap = Heap::new();
    let value = heap.alloc_json(value)?;
    Ok(heap.snapshot(&value, &mut Memo::default()))
}

/// An annotation of upstream's AST JSON, revived as [`revive`] does: the attribute object
/// as its fields, any other value (a hand-made token's) as it is, in `raw`.
pub fn revive_annotation(value: &Value) -> std::result::Result<AttributeValue, String> {
    let mut heap = Heap::new();
    let value = heap.alloc_json(value)?;
    Ok(annotation_value(
        &heap,
        Annotation::Raw(value),
        &mut Memo::default(),
    ))
}

/// `parser(tokens, args)`: the AST of `tokens`, which markdown-it (with Markdoc's plugins)
/// made from `source`. `source` is read only for positions and frontmatter; upstream's
/// parser reads tokens alone.
///
/// `Err` only where upstream's parser throws.
pub fn parse(tokens: &[Token], source: &str, args: &ParserArgs) -> Result<Node> {
    // This crate's spans, when asked for (they need upstream's locations on).
    let spans = args.spans && args.location;
    // `new Node('document')`: its `lines` are an empty array of its own, and it has no
    // `location`.
    let mut document = Node::new(NodeType::Document, IndexMap::new(), Vec::new(), None);
    let mut parser = Parser {
        // As many as the tokens and their children could make (one node each at most).
        arena: Vec::with_capacity(
            tokens
                .iter()
                .map(|t| 1 + t.children.as_ref().map_or(0, Vec::len))
                .sum::<usize>()
                + 1,
        ),
        nodes: Vec::new(),
        file: args.file.clone(),
        shared_file: args.file.as_deref().map(Arc::from),
        handle_slots: args.slots,
        add_location: args.location,
        parse_frontmatter: args.parse_frontmatter,
        positions: spans.then(|| Positions::new(source)),
        index: spans.then(|| LineIndex::new(source)),
        source,
        max_token_level: usize::try_from(crate::depth::left() / TOKEN_COST).unwrap_or(usize::MAX),
        heap: Heap::new(),
        memo: Memo::default(),
    };
    // This crate's: the document spans the whole source (with spans on).
    document.span = parser.span_of(Extent {
        start: 0,
        end: source.len(),
    });
    parser.arena.push(Built::new(document, None));
    parser.nodes.push(0);

    for i in 0..tokens.len() {
        let (span, child_spans) = match parser.positions.as_mut() {
            Some(positions) => positions.token(tokens, i),
            None => (Extent::at(0), None),
        };
        parser.handle_tree(&tokens[i], span, child_spans)?;
    }

    if parser.nodes.len() > 1 {
        for &index in &parser.nodes[1..] {
            let node = parser.arena[index].node_mut();
            let name = node
                .tag
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| node.node_type.as_str().to_string());
            let span = node.span.clone();
            node.errors.push(ValidationError {
                id: "missing-closing".into(),
                level: ValidationLevel::Critical,
                message: format!("Node '{name}' is missing closing"),
                location: None,
                raw: None,
                span,
            });
        }
    }

    let mut document = parser.assemble();
    for transform in super::transforms::TRANSFORMS {
        transform(&mut document, args.conditional_tags.as_deref())?;
    }
    Ok(document)
}

#[cfg(test)]
mod tests;
