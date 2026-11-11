use crate::types::*;
use indexmap::IndexMap;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::Cell;
use std::fmt;

/// A document's tree.
///
/// A tree may be of any depth (a hand-made one; [`crate::next::parser::parse`] returns
/// none deeper than [`crate::next::parser::MAX_AST_DEPTH`]), so nothing a `Node` does for
/// itself recurses on the thread's stack without a bound: dropping, cloning,
/// [`Node::walk`] keeps its own stack on the heap, and `Debug`,
/// `Serialize` and `Deserialize`, which `fmt` and serde make recursive, stop at a depth: below
/// [`MAX_DEBUG_DEPTH`] `Debug` writes a node's children and slots as `[.. n]`, and serde gets
/// the error `RangeError: Maximum call stack size exceeded` (what `JSON.stringify` of a deep
/// tree throws upstream) for a tree deeper than any the parser returns
/// ([`crate::next::parser::MAX_AST_DEPTH`]), each level on a stack with room for it
/// ([`crate::depth`]), or, where the stack cannot grow ([`crate::depth::can_grow`]),
/// past [`MAX_SERIALIZE_DEPTH`] and [`MAX_DESERIALIZE_DEPTH`], which fit a 2 MiB stack in a
/// debug build.
pub struct Node {
    /// Serialized as `type`.
    pub node_type: NodeType,
    /// In the order upstream's object holds them (see [`js_insert`]): the
    /// order shows in validation errors, render-tree attributes and HTML.
    pub attributes: IndexMap<String, Scalar>,
    pub children: Vec<Node>,
    pub errors: Vec<ValidationError>,
    /// Upstream's `lines`: the 0-based line map (`token.map`, `[first, last + 1]`) of the
    /// token the node was made of, its parent's when the token had none, and the closing
    /// token's appended when a tag closes on lines of its own (`[2, 3, 4, 5]`); empty for the
    /// document, for the nodes the table transform makes, and (but for those closings) when
    /// the parser was told `location: false`. Upstream's nodes share one array where a token
    /// has no map, so what a closing appends shows in each of them: so it does here (each
    /// node holds the array as it is at the end). See [`crate::next::parser`].
    pub lines: Vec<usize>,
    pub tag: Option<String>,
    pub annotations: Vec<AttributeValue>,
    pub inline: bool,
    /// Upstream's `location`: `{file, start: {line, character}, end: {...}}` from `lines` and
    /// the token's `position` as they were when the node was made ([`Location`]); `None`
    /// for the document, for the nodes the table transform makes, and with `location: false`.
    pub location: Option<Location>,
    /// This crate's own position of the node: where it starts and ends in the source, as
    /// 1-based lines, UTF-16 columns and byte offsets ([`Span`]). Upstream has no such
    /// thing. Opt-in: the parser fills it only with [`crate::parser::ParserArgs::spans`] on
    /// (and `location` not `false`); otherwise `None`, which JSON leaves out.
    pub span: Option<Span>,
    /// In the order the document fills them, as upstream's object holds them
    /// (`walk` visits them in this order, before the children).
    pub slots: IndexMap<String, Node>,
    /// The prototype of `attributes`, when an attribute named `__proto__` replaced it
    /// (upstream's `node.attributes.__proto__ = value` calls `Object.prototype`'s setter):
    /// `None` is `Object.prototype`, `Some(Scalar::Null)` is `null`, any other value an
    /// object the attributes inherit from (an object, an array, a variable, ...). No JSON shows
    /// it; what reads the attributes reads through it (the parser's own checks, `validate()`,
    /// `transform`).
    pub attributes_prototype: Option<Box<Scalar>>,
    /// The node `slots` inherit from, when a slot named `__proto__` was filed (upstream's
    /// `parent.slots['__proto__'] = node` makes that node the slots' prototype instead of a
    /// slot): it is in no list of slots or children, so nothing walks, validates or transforms
    /// it, and no JSON shows it; `validate()`'s check of a required slot reads through it.
    pub slots_prototype: Option<Box<Node>>,
}

impl Node {
    /// `new Node(type, attributes, children, tag)`. `attributes` is anything
    /// that yields `(name, value)` pairs -- an `IndexMap`, a `HashMap`, a
    /// `Vec` -- kept in the order it yields them.
    pub fn new(
        node_type: NodeType,
        attributes: impl IntoIterator<Item = (String, Scalar)>,
        children: Vec<Node>,
        tag: Option<String>,
    ) -> Self {
        Self {
            node_type,
            attributes: attributes.into_iter().collect(),
            children,
            tag,
            errors: Vec::new(),
            lines: Vec::new(),
            annotations: Vec::new(),
            inline: false,
            location: None,
            span: None,
            slots: IndexMap::new(),
            attributes_prototype: None,
            slots_prototype: None,
        }
    }

    pub fn push(&mut self, child: Node) {
        self.children.push(child);
    }

    /// Every node under this one, as upstream's `walk()` yields them: each slot, then
    /// each child, every one before the nodes under it.
    pub fn walk(&self) -> impl Iterator<Item = &Node> {
        let mut nodes = Vec::new();
        let mut stack: Vec<&Node> = Vec::new();
        push_walked(self, &mut stack);
        while let Some(node) = stack.pop() {
            nodes.push(node);
            push_walked(node, &mut stack);
        }
        nodes.into_iter()
    }

    /// The node without its children and slots (room made for them).
    fn without_children(&self) -> Node {
        Node {
            node_type: self.node_type.clone(),
            attributes: self.attributes.clone(),
            children: Vec::with_capacity(self.children.len()),
            errors: self.errors.clone(),
            lines: self.lines.clone(),
            tag: self.tag.clone(),
            annotations: self.annotations.clone(),
            inline: self.inline,
            location: self.location.clone(),
            span: self.span.clone(),
            slots: IndexMap::with_capacity(self.slots.len()),
            attributes_prototype: self.attributes_prototype.clone(),
            slots_prototype: None,
        }
    }

    /// Has the node children, slots or a slots' prototype node?
    fn has_descendants(&self) -> bool {
        !self.children.is_empty() || !self.slots.is_empty() || self.slots_prototype.is_some()
    }
}

/// `node`'s slots and children onto `stack`, so that they come off it in [`Node::walk`]'s
/// order.
fn push_walked<'a>(node: &'a Node, stack: &mut Vec<&'a Node>) {
    stack.extend(node.children.iter().rev());
    stack.extend(node.slots.values().rev());
}

/// How many levels [`Node`]'s drop recurses before it takes the rest of a tree apart from a
/// stack on the heap. Recursing is what the derived drop did, and costs less time; a level
/// of it takes about 2.1 KB of stack in a debug build (measured: 255 levels need 539 KB),
/// so this keeps it near 70 KB, deeper than documents nest but for pathological ones.
const DROP_RECURSION: usize = 32;

impl Drop for Node {
    /// Depth first, recursing at most `DROP_RECURSION` levels, below which a stack on the
    /// heap takes over, so that no tree is too deep to drop: each node's children and slots
    /// are taken out of it and emptied in turn before they are dropped, each then dropping
    /// without recursion.
    fn drop(&mut self) {
        if self.has_descendants() {
            drop_descendants(self, 0);
        }
    }
}

/// Drops `node`'s children and slots, and so everything under it (see [`Node`]'s `Drop`).
fn drop_descendants(node: &mut Node, depth: usize) {
    if depth >= DROP_RECURSION {
        drop_descendants_from_the_heap(node);
        return;
    }
    let has_descendants = |n: &Node| n.has_descendants();
    if let Some(mut proto) = node.slots_prototype.take()
        && has_descendants(&proto)
    {
        drop_descendants(&mut proto, depth + 1);
    }
    if !node.children.is_empty() {
        for mut child in std::mem::take(&mut node.children) {
            if has_descendants(&child) {
                drop_descendants(&mut child, depth + 1);
            }
        }
    }
    if !node.slots.is_empty() {
        for (_, mut slot) in std::mem::take(&mut node.slots) {
            if has_descendants(&slot) {
                drop_descendants(&mut slot, depth + 1);
            }
        }
    }
}

/// [`drop_descendants`] with a stack on the heap: a node taken out of its parent has its
/// children and slots put on the stack before it drops.
fn drop_descendants_from_the_heap(node: &mut Node) {
    fn detach(node: &mut Node, stack: &mut Vec<std::vec::IntoIter<Node>>) {
        if let Some(proto) = node.slots_prototype.take() {
            stack.push(vec![*proto].into_iter());
        }
        if !node.slots.is_empty() {
            let slots: Vec<Node> = std::mem::take(&mut node.slots).into_values().collect();
            stack.push(slots.into_iter());
        }
        if !node.children.is_empty() {
            stack.push(std::mem::take(&mut node.children).into_iter());
        }
    }
    let mut stack = Vec::new();
    detach(node, &mut stack);
    while let Some(nodes) = stack.last_mut() {
        match nodes.next() {
            Some(mut node) => detach(&mut node, &mut stack),
            None => {
                stack.pop();
            }
        }
    }
}

impl Clone for Node {
    /// A deep copy, made from a stack on the heap (no tree is too deep for it).
    fn clone(&self) -> Self {
        /// Where a copied node goes in its parent's copy.
        enum Role<'a> {
            Slot(&'a String),
            Child,
            SlotsPrototype,
        }
        /// A node being copied: its copy so far, and where it goes in its parent.
        struct Frame<'a> {
            original: &'a Node,
            copy: Node,
            role: Role<'a>,
            /// How many of its slots, then children, then its slots' prototype, are being or
            /// have been copied.
            next: usize,
        }
        fn open<'a>(original: &'a Node, role: Role<'a>) -> Frame<'a> {
            Frame {
                original,
                copy: original.without_children(),
                role,
                next: 0,
            }
        }
        let mut stack = vec![open(self, Role::Child)];
        loop {
            let top = stack.last_mut().expect("the copy's own frame");
            let (slots, i) = (top.original.slots.len(), top.next);
            let children = top.original.children.len();
            if i < slots + children + usize::from(top.original.slots_prototype.is_some()) {
                top.next += 1;
                let frame = match top.original.slots.get_index(i) {
                    Some((name, slot)) => open(slot, Role::Slot(name)),
                    None if i < slots + children => {
                        open(&top.original.children[i - slots], Role::Child)
                    }
                    None => open(
                        top.original
                            .slots_prototype
                            .as_deref()
                            .expect("counted above"),
                        Role::SlotsPrototype,
                    ),
                };
                stack.push(frame);
                continue;
            }
            let done = stack.pop().expect("the frame just looked at");
            let Some(parent) = stack.last_mut() else {
                return done.copy;
            };
            match done.role {
                Role::Slot(name) => {
                    parent.copy.slots.insert(name.clone(), done.copy);
                }
                Role::Child => parent.copy.children.push(done.copy),
                Role::SlotsPrototype => parent.copy.slots_prototype = Some(Box::new(done.copy)),
            }
        }
    }
}

// The limits below were measured in a debug build, as the smallest thread stack each
// operation needs on a chain of nodes (each with attributes, an error, an annotation and a
// location), and on one value nested 300 deep (how deep the tag grammar nests values, and
// `crate::next::parser::MAX_VALUE_DEPTH`): per level of nodes and for the value,
// `format!("{:?}")` 0.71 KB and 203 KB, `{:#?}` 0.96 KB and 267 KB, `serde_json::to_string`
// 1.56 KB and 247 KB, `serde_json::to_value` 2.76 KB and 407 KB, `serde_json::from_value`
// 14.4 KB and 91 KB. Each limit keeps at least a quarter of a 2 MiB stack spare with the
// value at the deepest node. (A release build takes a fraction of this.)

/// How deep `Debug` writes a tree: a node this deep writes its children and slots as
/// `[.. n]` (how many it leaves out). `{:#?}` indents every line by its depth, through one
/// `PadAdapter` per level, so its output grows with the square of the depth and its time
/// with the cube (a chain 300 deep took 51 s in a debug build): the limit is set by that
/// rather than by the stack (100 levels and a value take about 370 KB).
pub const MAX_DEBUG_DEPTH: usize = 100;

/// How deep a tree serializes where the stack cannot grow ([`crate::depth::can_grow`]): a node
/// deeper fails with `RangeError: Maximum call stack size exceeded`. With
/// `serde_json::to_value`, 350 levels and a value take about 1.37 MB. Where it grows, the
/// deepest tree the parser returns serializes ([`crate::next::parser::MAX_AST_DEPTH`] levels
/// below the document).
pub const MAX_SERIALIZE_DEPTH: usize = 350;

/// How deep a tree deserializes where the stack cannot grow: a node deeper fails with
/// `RangeError: Maximum call stack size exceeded`. With `serde_json::from_value`, 90 levels
/// and a value take about 1.39 MB. (`serde_json::from_str` stops sooner, at its own limit of
/// 128 nested arrays and objects: 63 levels of nodes.) Where it grows, as deep as
/// [`MAX_SERIALIZE_DEPTH`]'s.
pub const MAX_DESERIALIZE_DEPTH: usize = 90;

/// How deep serde goes into a tree on this thread: [`MAX_SERIALIZE_DEPTH`] or
/// [`MAX_DESERIALIZE_DEPTH`] where the stack cannot grow, the parser's deepest tree where it
/// grows.
fn serde_limit(fixed: usize) -> usize {
    if crate::depth::can_grow() {
        crate::next::parser::MAX_AST_DEPTH + 1
    } else {
        fixed
    }
}

thread_local! {
    /// How many `Debug::fmt` calls of nodes are open on this thread.
    static DEBUG_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// How many serializations of nodes are open on this thread.
    static SERIALIZE_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// How many deserializations of nodes are open on this thread.
    static DESERIALIZE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// One more open call on a depth counter, for as long as it lives.
struct Depth(&'static std::thread::LocalKey<Cell<usize>>);

impl Depth {
    /// Opens a call; the depth it is at (1 for the outermost).
    fn enter(counter: &'static std::thread::LocalKey<Cell<usize>>) -> (Depth, usize) {
        let depth = counter.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        (Depth(counter), depth)
    }
}

impl Drop for Depth {
    fn drop(&mut self) {
        self.0.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// What `Debug` at [`MAX_DEBUG_DEPTH`] writes for children or slots it leaves out.
struct Elided(usize);

impl fmt::Debug for Elided {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.write_str("[]"),
            n => write!(f, "[.. {n}]"),
        }
    }
}

impl fmt::Debug for Node {
    /// As `#[derive(Debug)]` writes it, down to [`MAX_DEBUG_DEPTH`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (_open, depth) = Depth::enter(&DEBUG_DEPTH);
        let mut s = f.debug_struct("Node");
        s.field("node_type", &self.node_type)
            .field("attributes", &self.attributes);
        if depth < MAX_DEBUG_DEPTH {
            s.field("children", &self.children);
        } else {
            s.field("children", &Elided(self.children.len()));
        }
        s.field("errors", &self.errors)
            .field("lines", &self.lines)
            .field("tag", &self.tag)
            .field("annotations", &self.annotations)
            .field("inline", &self.inline)
            .field("location", &self.location)
            .field("span", &self.span);
        if depth < MAX_DEBUG_DEPTH {
            s.field("slots", &self.slots);
        } else {
            s.field("slots", &Elided(self.slots.len()));
        }
        if let Some(proto) = &self.attributes_prototype {
            s.field("attributes_prototype", proto);
        }
        if let Some(proto) = &self.slots_prototype {
            if depth < MAX_DEBUG_DEPTH {
                s.field("slots_prototype", proto);
            } else {
                s.field("slots_prototype", &Elided(1));
            }
        }
        s.finish()
    }
}

/// The error past [`MAX_SERIALIZE_DEPTH`] and [`MAX_DESERIALIZE_DEPTH`].
const TOO_DEEP: &str = "RangeError: Maximum call stack size exceeded";

/// A [`Node`] as it serializes, field for field (what `#[derive(Serialize)]` wrote).
#[derive(Serialize)]
#[serde(rename = "Node")]
struct NodeOut<'a> {
    #[serde(rename = "type")]
    node_type: &'a NodeType,
    attributes: &'a IndexMap<String, Scalar>,
    children: &'a Vec<Node>,
    errors: &'a Vec<ValidationError>,
    lines: &'a [usize],
    tag: &'a Option<String>,
    annotations: &'a Vec<AttributeValue>,
    inline: bool,
    location: &'a Option<Location>,
    /// This crate's, and opt-in ([`crate::parser::ParserArgs::spans`]): left out when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    span: &'a Option<Span>,
    slots: &'a IndexMap<String, Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes_prototype: &'a Option<Box<Scalar>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slots_prototype: &'a Option<Box<Node>>,
}

/// A [`Node`] as it deserializes (what `#[derive(Deserialize)]` read).
#[derive(Deserialize)]
#[serde(rename = "Node")]
struct NodeIn {
    #[serde(rename = "type")]
    node_type: NodeType,
    attributes: IndexMap<String, Scalar>,
    children: Vec<Node>,
    errors: Vec<ValidationError>,
    lines: Vec<usize>,
    tag: Option<String>,
    annotations: Vec<AttributeValue>,
    inline: bool,
    location: Option<Location>,
    #[serde(default)]
    span: Option<Span>,
    slots: IndexMap<String, Node>,
    #[serde(default)]
    attributes_prototype: Option<Box<Scalar>>,
    #[serde(default)]
    slots_prototype: Option<Box<Node>>,
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let (_open, depth) = Depth::enter(&SERIALIZE_DEPTH);
        if depth > serde_limit(MAX_SERIALIZE_DEPTH) {
            return Err(serde::ser::Error::custom(TOO_DEEP));
        }
        crate::depth::grow!(self.serialize_here(serializer))
    }
}

impl Node {
    /// [`Serialize::serialize`] of this node (its children each on a stack with room).
    #[inline(always)]
    fn serialize_here<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        NodeOut {
            node_type: &self.node_type,
            attributes: &self.attributes,
            children: &self.children,
            errors: &self.errors,
            lines: &self.lines,
            tag: &self.tag,
            annotations: &self.annotations,
            inline: self.inline,
            location: &self.location,
            span: &self.span,
            slots: &self.slots,
            attributes_prototype: &self.attributes_prototype,
            slots_prototype: &self.slots_prototype,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let (_open, depth) = Depth::enter(&DESERIALIZE_DEPTH);
        if depth > serde_limit(MAX_DESERIALIZE_DEPTH) {
            return Err(de::Error::custom(TOO_DEEP));
        }
        let n = crate::depth::grow!(NodeIn::deserialize(deserializer))?;
        Ok(Node {
            node_type: n.node_type,
            attributes: n.attributes,
            children: n.children,
            errors: n.errors,
            lines: n.lines,
            tag: n.tag,
            annotations: n.annotations,
            inline: n.inline,
            location: n.location,
            span: n.span,
            slots: n.slots,
            attributes_prototype: n.attributes_prototype,
            slots_prototype: n.slots_prototype,
        })
    }
}

/// One step of a [`Variable`]'s path. Upstream's grammar makes `.name` a
/// string step (even `$a.0`) and `[...]` a step of the literal's type: `$a[0]`
/// a number, `$a["b"]` a string.
#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    String(String),
    Number(f64),
}

impl From<&str> for PathSegment {
    fn from(s: &str) -> Self {
        PathSegment::String(s.to_string())
    }
}

impl From<String> for PathSegment {
    fn from(s: String) -> Self {
        PathSegment::String(s)
    }
}

impl From<f64> for PathSegment {
    fn from(n: f64) -> Self {
        PathSegment::Number(n)
    }
}

impl PathSegment {
    fn to_scalar(&self) -> Scalar {
        match self {
            PathSegment::String(s) => Scalar::String(s.clone()),
            PathSegment::Number(n) => Scalar::Number(*n),
        }
    }
}

impl Serialize for PathSegment {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            PathSegment::String(s) => serializer.serialize_str(s),
            PathSegment::Number(n) => serializer.serialize_f64(*n),
        }
    }
}

impl<'de> Deserialize<'de> for PathSegment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match &mut Scalar::deserialize(deserializer)? {
            Scalar::String(s) => Ok(PathSegment::String(std::mem::take(s))),
            Scalar::Number(n) => Ok(PathSegment::Number(*n)),
            other => Err(de::Error::custom(format!(
                "a variable's path step is a string or a number, not {other:?}"
            ))),
        }
    }
}

/// Upstream's `Variable` (`ast/variable.ts`): `$a.b[0]["c"]` is the path
/// `["a", "b", 0, "c"]`. Its JSON is `{"$$mdtype":"Variable","path":[...]}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub path: Vec<PathSegment>,
}

impl Variable {
    pub fn new(path: Vec<PathSegment>) -> Self {
        Self { path }
    }

    /// The variable an object of upstream's JSON describes (its `path`; the
    /// `$$mdtype` is the caller's to check).
    pub fn from_object(object: &IndexMap<String, Scalar>) -> std::result::Result<Self, String> {
        let Some(Scalar::Array(steps)) = object.get("path") else {
            return Err(format!("a Variable has a `path` array: {object:?}"));
        };
        let path = steps
            .iter()
            .map(|step| match step {
                Scalar::String(s) => Ok(PathSegment::String(s.clone())),
                Scalar::Number(n) => Ok(PathSegment::Number(*n)),
                other => Err(format!(
                    "a variable's path step is a string or a number, not {other:?}"
                )),
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self { path })
    }

    /// Upstream's JSON: `{"$$mdtype":"Variable","path":[...]}`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut object = serde_json::Map::new();
        object.insert("$$mdtype".into(), "Variable".into());
        object.insert(
            "path".into(),
            serde_json::Value::Array(self.path.iter().map(|s| s.to_scalar().to_json()).collect()),
        );
        serde_json::Value::Object(object)
    }
}

impl Serialize for Variable {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("$$mdtype", "Variable")?;
        map.serialize_entry("path", &self.path)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Variable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let object = deserializer.deserialize_map(ObjectVisitor)?;
        check_mdtype(&object, "Variable").map_err(de::Error::custom)?;
        Variable::from_object(&object).map_err(de::Error::custom)
    }
}

/// Upstream's `Function` (`ast/function.ts`): `f(1, x=2)` is the name `"f"`
/// and the parameters `{"0": 1, "x": 2}`. A positional argument is keyed by
/// its position among all the arguments, named ones included, and the keys
/// are in a JS object's order (so `g(x=1, 2)` is `{"1": 2, "x": 1}`; see
/// [`js_insert`]). Its JSON is
/// `{"$$mdtype":"Function","name":...,"parameters":{...}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub parameters: IndexMap<String, Scalar>,
}

impl Function {
    pub fn new(name: String, parameters: IndexMap<String, Scalar>) -> Self {
        Self { name, parameters }
    }

    /// The function an object of upstream's JSON describes (its `name` and
    /// `parameters`; the `$$mdtype` is the caller's to check).
    pub fn from_object(object: &IndexMap<String, Scalar>) -> std::result::Result<Self, String> {
        let Some(Scalar::String(name)) = object.get("name") else {
            return Err(format!("a Function has a `name` string: {object:?}"));
        };
        let Some(Scalar::Object(parameters)) = object.get("parameters") else {
            return Err(format!("a Function has a `parameters` object: {object:?}"));
        };
        Ok(Self {
            name: name.clone(),
            parameters: parameters.clone(),
        })
    }

    /// Upstream's JSON: `{"$$mdtype":"Function","name":...,"parameters":{...}}`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut object = serde_json::Map::new();
        object.insert("$$mdtype".into(), "Function".into());
        object.insert("name".into(), self.name.clone().into());
        object.insert(
            "parameters".into(),
            serde_json::Value::Object(
                self.parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
        );
        serde_json::Value::Object(object)
    }
}

impl Serialize for Function {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("$$mdtype", "Function")?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("parameters", &self.parameters)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Function {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let object = deserializer.deserialize_map(ObjectVisitor)?;
        check_mdtype(&object, "Function").map_err(de::Error::custom)?;
        Function::from_object(&object).map_err(de::Error::custom)
    }
}

/// A `$$mdtype`, when the object has one, is `expected`.
fn check_mdtype(
    object: &IndexMap<String, Scalar>,
    expected: &str,
) -> std::result::Result<(), String> {
    match object.get("$$mdtype") {
        None => Ok(()),
        Some(Scalar::String(t)) if t == expected => Ok(()),
        Some(other) => Err(format!("expected a {expected}, found $$mdtype {other:?}")),
    }
}

/// A JSON object's entries as [`Scalar`]s, keys in a JS object's order.
struct ObjectVisitor;

impl<'de> Visitor<'de> for ObjectVisitor {
    type Value = IndexMap<String, Scalar>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut object = IndexMap::new();
        while let Some((key, value)) = map.next_entry::<String, Scalar>()? {
            js_insert(&mut object, key, value);
        }
        Ok(object)
    }
}

#[cfg(test)]
mod tests {
    //! What a `Node` does for itself at any depth, on a thread with a 2 MiB stack (every
    //! test thread's, and the robustness policy's), in debug and in release.
    use super::*;

    fn text(content: &str) -> Node {
        let mut attributes = IndexMap::new();
        attributes.insert("content".to_string(), Scalar::String(content.into()));
        Node::new(NodeType::Text, attributes, Vec::new(), None)
    }

    /// A value nested `depth` deep (arrays and objects in turn).
    fn deep_value(depth: usize) -> Scalar {
        let mut value = Scalar::String("deep".into());
        for i in 0..depth {
            value = if i % 2 == 0 {
                Scalar::Array(vec![value])
            } else {
                let mut object = IndexMap::new();
                object.insert("k".to_string(), value);
                Scalar::Object(object)
            };
        }
        value
    }

    /// `n` tag nodes nested in one another (the innermost holding `leaf`), each with an
    /// error, a location and a slot beside its child every tenth level.
    fn chain(n: usize, leaf: Node) -> Node {
        let edge = |line| crate::types::LocationEdge {
            line: Some(line),
            character: Some(Box::new(serde_json::Value::from(line + 3))),
        };
        let position = |line| crate::types::Position {
            line,
            character: Some(0),
            offset: 0,
        };
        let mut node = leaf;
        for i in 0..n {
            let mut parent =
                Node::new(NodeType::Tag, IndexMap::new(), vec![node], Some("x".into()));
            parent.errors.push(ValidationError {
                id: "e".into(),
                level: ValidationLevel::Error,
                message: "m".into(),
                location: None,
                raw: None,
                span: None,
            });
            parent.lines = vec![i, i + 1];
            parent.location = Some(Location {
                file: None,
                start: edge(i),
                end: edge(i + 1),
            });
            parent.span = Some(Span {
                file: None,
                start: position(i + 1),
                end: position(i + 2),
            });
            if i % 10 == 0 {
                parent.slots.insert("s".into(), text("slot"));
            }
            node = parent;
        }
        node
    }

    fn on_a_2_mib_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(f)
            .expect("a thread")
            .join()
            .expect("no overflow")
    }

    /// How many nodes deep, the node itself counted.
    fn levels(node: &Node) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(node, 1)];
        while let Some((n, d)) = stack.pop() {
            deepest = deepest.max(d);
            stack.extend(n.slots.values().chain(&n.children).map(|c| (c, d + 1)));
        }
        deepest
    }

    #[test]
    fn a_tree_of_any_depth_clones_walks_and_drops() {
        // A derived Drop aborted a 2 MiB thread at 14,002 levels in debug and 40,002 in
        // release.
        let (walked, cloned_levels) = on_a_2_mib_stack(|| {
            let tree = chain(200_000, text("leaf"));
            let copy = tree.clone();
            let walked = tree.walk().count();
            let resolved = tree.clone();
            drop(tree);
            let cloned_levels = (levels(&copy), levels(&resolved));
            (walked, cloned_levels)
        });
        // Under the outermost tag: the other 199,999, the leaf, and a slot on every tenth.
        assert_eq!(walked, 199_999 + 1 + 20_000);
        assert_eq!(cloned_levels, (200_001, 200_001));
    }

    #[test]
    fn a_clone_is_the_tree_and_walk_is_upstreams_order() {
        let mut a = text("a");
        a.slots.insert("s".into(), text("s"));
        a.children.push(text("a1"));
        let mut tree = Node::new(
            NodeType::Document,
            IndexMap::new(),
            vec![a, text("b")],
            None,
        );
        tree.slots.insert("t".into(), text("t"));
        let content = |n: &Node| match n.attributes.get("content") {
            Some(Scalar::String(s)) => s.clone(),
            _ => n.node_type.to_string(),
        };
        // Each slot, then each child, before what is under it.
        let order: Vec<String> = tree.walk().map(content).collect();
        assert_eq!(order, ["t", "a", "s", "a1", "b"]);
        let copy = tree.clone();
        assert_eq!(copy.walk().map(content).collect::<Vec<_>>(), order);
        assert_eq!(
            serde_json::to_string(&copy).unwrap(),
            serde_json::to_string(&tree).unwrap()
        );
    }

    #[test]
    fn debug_writes_a_tree_down_to_its_limit() {
        let (plain, pretty, small, deeper) = on_a_2_mib_stack(|| {
            let mut leaf = text("leaf");
            leaf.attributes.insert("v".into(), deep_value(300));
            let deep = chain(MAX_DEBUG_DEPTH - 1, leaf);
            let elided = chain(10_000, text("leaf"));
            let mut far = text("far");
            far.attributes.insert("v".into(), deep_value(100_000));
            (
                format!("{deep:?}"),
                format!("{:#?}", chain(MAX_DEBUG_DEPTH + 5, text("leaf"))),
                (format!("{:?}", text("a")), format!("{elided:?}").len()),
                format!("{far:?}"),
            )
        });
        // MAX_DEBUG_DEPTH levels are written in full, the deepest value included...
        assert!(plain.contains("\"deep\""));
        assert!(!plain.contains("[.."));
        // ...and a node that deep leaves out what it holds.
        assert!(pretty.contains("children: [.. 1]"));
        assert_eq!(
            small.0,
            "Node { node_type: Text, attributes: {\"content\": String(\"a\")}, \
             children: [], errors: [], lines: [], tag: None, \
             annotations: [], inline: false, location: None, span: None, slots: {} }"
        );
        assert!(small.1 < 1_000_000);
        // A value deeper than Debug writes ends in `..`.
        assert!(deeper.contains("..") && !deeper.contains("\"deep\"") && deeper.len() < 100_000);
    }

    /// Takes a deep JSON value apart without recursing (serde_json's own drop recurses).
    fn drop_json(value: serde_json::Value) {
        let mut stack = vec![value];
        while let Some(value) = stack.pop() {
            match value {
                serde_json::Value::Array(items) => stack.extend(items),
                serde_json::Value::Object(map) => stack.extend(map.into_iter().map(|(_, v)| v)),
                _ => {}
            }
        }
    }

    #[test]
    fn serde_stops_at_its_limits() {
        // Where the stack grows, the deepest tree the parser returns; else the fixed limits.
        let (ser, de) = if crate::depth::can_grow() {
            let deepest = crate::next::parser::MAX_AST_DEPTH + 1;
            (deepest, deepest)
        } else {
            (MAX_SERIALIZE_DEPTH, MAX_DESERIALIZE_DEPTH)
        };
        on_a_2_mib_stack(move || {
            let deep = |levels: usize| {
                let mut leaf = text("leaf");
                leaf.attributes.insert("v".into(), deep_value(300));
                chain(levels - 1, leaf)
            };
            // Serialize: `ser` nodes in one another, a deep value in the innermost, and one
            // more.
            let at = deep(ser);
            assert_eq!(levels(&at), ser);
            let value = serde_json::to_value(&at).expect("serializes");
            assert!(serde_json::to_string(&at).is_ok());
            let past = serde_json::to_string(&deep(ser + 1)).unwrap_err();
            assert!(
                past.to_string()
                    .contains("Maximum call stack size exceeded")
            );
            assert!(serde_json::to_string(&chain(100_000, text("x"))).is_err());

            // Deserialize: `de` nodes back from JSON, and one more.
            let mut json = value;
            let mut node = &mut json;
            for _ in 0..de - 1 {
                node = &mut node["children"][0];
            }
            drop_json(std::mem::replace(
                &mut node["children"],
                serde_json::Value::Array(Vec::new()),
            ));
            // From a reference (`from_value` would take the value and drop it recursively).
            let back = Node::deserialize(&json).expect("deserializes");
            assert_eq!(levels(&back), de);
            let mut node = &mut json;
            for _ in 0..de - 1 {
                node = &mut node["children"][0];
            }
            node["children"] = serde_json::to_value(vec![text("one more")]).unwrap();
            let past = Node::deserialize(&json).unwrap_err();
            assert!(
                past.to_string()
                    .contains("Maximum call stack size exceeded")
            );
            drop_json(json);
        });
    }

    #[test]
    fn values_of_any_depth_clone_compare_write_and_drop() {
        // What the parser keeps of a hand-made token's `meta` where the stack grows: a value
        // of any depth, safe for everything a `Scalar` does on a 2 MiB stack.
        if !crate::depth::can_grow() {
            return;
        }
        on_a_2_mib_stack(|| {
            let value = deep_value(100_000);
            let copy = value.clone();
            assert!(copy == value);
            let json = serde_json::to_string(&value).expect("serializes");
            assert_eq!(
                json.len(),
                100_000 / 2 * ("[".len() + "{\"k\":".len() + "}]".len()) + 6
            );
            let tree = value.to_json();
            let back = Scalar::from_json(&tree).expect("reads back");
            drop_json(tree);
            assert!(back == copy);
            let mut tag = Scalar::Tag(Box::default());
            for _ in 0..50_000 {
                tag = Scalar::Tag(Box::new(crate::types::Tag {
                    name: "b".into(),
                    attributes: IndexMap::new(),
                    children: vec![RenderableTreeNode::from(tag)],
                }));
            }
            let tag_copy = tag.clone();
            assert!(tag_copy == tag);
            drop((value, copy, back, tag, tag_copy));
        });
    }
}
