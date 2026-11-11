//! Port of upstream Markdoc v0.5.11's `src/renderers/html.ts`.
//!
//! Owner: the semantics agent; reading values through their prototype chains: the model agent.
//!
//! [`render`] renders a render tree as upstream's `transform` returns it: JavaScript values
//! ([`Value`]), not their JSON, because `render` reads what JSON leaves out (`String(v)` of an
//! attribute's value sees an own `toString` member that is `undefined`, an inherited property
//! through a prototype). Like upstream's, it takes a node as it is, whatever it holds:
//!
//! - a string or a number is escaped text; an array is `node.map(render).join('')` (its
//!   `map` looked up as JavaScript does: an own `map` that is not a function throws, and an
//!   own `constructor` that is not one makes `map`'s `ArraySpeciesCreate` throw; a hole renders
//!   nothing); anything else that `Tag.isTag` does not accept (`undefined`, `null`, a boolean,
//!   an object that is not a tag) is nothing;
//! - a tag is a `Tag` or any object whose `$$mdtype` is `'Tag'`, own or inherited (a variable
//!   can hold one; an object whose prototype is a tag is one), read with `const { name,
//!   attributes, children = [] } = node` (each read through the object's prototype chain): a
//!   falsy `name` renders the children alone, any other is printed as `String(name)` does; the
//!   attributes are `Object.entries(attributes ?? {})` (an array's or a string's are its
//!   indices, a number's or a boolean's none), each key lower-cased and each value
//!   `String(v)`, escaped; a name that is one of the void elements (the string itself) ends
//!   the tag there; otherwise the children are rendered when `children.length` is truthy (a
//!   string's length, an object's `length`, `null` throws).
//!
//! The output is built as JavaScript builds it, code unit by code unit: a string value with a
//! lone surrogate (see [`crate::next::schema::value`]'s "Strings") is written as its code
//! units, so two halves that meet (`{% $s[0] %}{% $s[1] %}` with `s` = "😀") are the
//! character they make, as in node's output string. The `String` returned is that output as
//! UTF-8.
//!
//! `Err` is where upstream throws: a value `String(...)` cannot convert (an object neither of
//! whose `toString` and `valueOf` gives a primitive; arrays nested past what is left of node's
//! stack, see [`crate::next::schema::value::MAX_JOIN_DEPTH`]), `children` that is `null`, an
//! array whose `map` is not a function or whose `constructor` is not a constructor, and a tree
//! nested past node's stack (`RangeError`: see [`MAX_RENDER_DEPTH`]). [`render_tree`] renders
//! a tree read from JSON (upstream's own output, or [`Value::to_render`]'s). Rendering is a
//! loop: any depth is safe for Rust's stack.
//!
//! # Divergences from upstream
//!
//! - A lone surrogate still alone in the output, which a Rust string cannot hold, is U+FFFD:
//!   the bytes node writes for it in UTF-8. Every route by which a value holds one ends here,
//!   in text, in attributes (a slot rendered into one included), in class names and in tag
//!   names (`# H {% level=$s[0] %}`): a variable path into a string (`{% $s[0] %}`, `{% foo
//!   a=$s[0] /%}`, `class=$s[0]`), a partial's `variables` given a string (each code unit a
//!   variable), values passed through functions (`default($s[0])`), and `Object.entries` of a
//!   string a tag-shaped object holds as its attributes (`{$$mdtype: 'Tag', name: 'b',
//!   attributes: '😀'}` is `<b 0="\ud83d" 1="\ude00"></b>` in node). Halves that meet are
//!   joined first.
//! - Call stack depth: upstream's `render` recurses (`render(children)` for a tag, then each
//!   child) until V8's stack runs out; the port charges each call's frames on the depth budget
//!   ([`crate::depth`]: [`cost::ARRAY`] for an array, [`cost::TAG`] for a tag besides its
//!   children array, a string's `String()` in an attribute what is left) and throws a
//!   `RangeError` where a fresh node 22 process does at the top level of a script: 2,005 nested
//!   arrays around a string ([`MAX_RENDER_DEPTH`]), 1,253 nested tags, 3,301 arrays in an
//!   attribute's value. Node renders deeper once V8 has compiled `render` (2,090 arrays after a
//!   few hundred renders, 7,839 arrays and 3,919 tags after a few thousand; 7,375 for
//!   `{% $v %}`), as its optimized frames are smaller.

use crate::markdown_it::js;
use crate::next::schema::value::{self, JsString, JsStringBuilder, Method, TagValue, Value};
use crate::types::{MarkdocError, RenderableTreeNode, Result};
use std::borrow::Cow;

/// How many arrays nested around a string `render` takes from the top of a thread: node 22
/// renders 2,005 in a fresh process called from the top level of a script (the first render
/// in the process), 2,090 after a few hundred renders and 7,839 after a few thousand, as V8
/// compiles `render`; `{% $v %}`'s tree with `v` 2,003 deep (warm 7,375); 1,253 nested tags
/// (warm 3,919). See [`cost`].
pub const MAX_RENDER_DEPTH: usize = 2006;

/// What each `render` call that nests costs on the depth budget ([`crate::depth`]).
pub mod cost {
    use crate::depth::{extra, per_level};

    /// `render` of an array (`node.map(render).join('')`): [`super::MAX_RENDER_DEPTH`].
    pub const ARRAY: u64 = per_level(super::MAX_RENDER_DEPTH as u64);
    /// `render` of a tag, besides `render(children)` of its children array (an [`ARRAY`]):
    /// 1,253 nested tags; a string's `String()` in an attribute is called from here.
    pub const TAG: u64 = extra(super::MAX_RENDER_DEPTH as u64, 1_253);
}

/// HTML elements that do not have a matching close tag
/// (<https://html.spec.whatwg.org/#void-elements>).
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// markdown-it's `escapeHtml`: `&`, `<`, `>` and `"`.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_text_into(s, &mut out);
    out
}

fn escape_text_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// `escapeHtml(s)` of a JS string, appended code unit by code unit.
fn escape_into(s: &JsString, out: &mut JsStringBuilder) {
    match s.as_str() {
        Some(text) if !text.contains(['&', '<', '>', '"']) => out.push_str(text),
        Some(text) => out.push_str(&escape_html(text)),
        None => {
            let mut units = Vec::with_capacity(s.len_utf16());
            for unit in s.code_units() {
                match unit {
                    0x26 => units.extend("&amp;".encode_utf16()),
                    0x3C => units.extend("&lt;".encode_utf16()),
                    0x3E => units.extend("&gt;".encode_utf16()),
                    0x22 => units.extend("&quot;".encode_utf16()),
                    unit => units.push(unit),
                }
            }
            out.push_units(&units);
        }
    }
}

fn throw(message: String) -> MarkdocError {
    MarkdocError::TransformError(message)
}

/// `render(node)` (see the module's documentation).
pub fn render(node: &Value) -> Result<String> {
    let mut out = JsStringBuilder::new();
    render_into(node, &mut out)?;
    Ok(out.finish_lossy())
}

/// [`render`] of a tree read from JSON.
pub fn render_tree(node: &RenderableTreeNode) -> Result<String> {
    render(&Value::from_render(node))
}

/// What is left to do, last first: render a value the tree holds, or one read through a
/// prototype chain (held here), below a path that costs so much of the depth budget (its
/// callers' calls), or close a tag.
enum Work<'a> {
    Render(&'a Value, u64),
    Owned(Box<Value>, u64),
    Close(Cow<'a, JsString>),
}

fn range_error() -> MarkdocError {
    throw(value::RANGE_ERROR.into())
}

/// The depth budget a call adds to a path that costs `path`, unless it would take more than
/// the `left` of it the render started with (where node's stack would run out).
fn deeper(path: u64, cost: u64, left: u64) -> Result<u64> {
    let path = path + cost;
    if path > left {
        return Err(range_error());
    }
    Ok(path)
}

/// `render(children)` below `path` for a tag's `children` array (a plain array: its `map` and
/// `constructor` are `Array`'s): the call, then each item below it (pushed so that the first
/// is rendered first).
fn push_children<'a>(
    children: &'a [Value],
    path: u64,
    left: u64,
    work: &mut Vec<Work<'a>>,
) -> Result<()> {
    let path = deeper(path, cost::ARRAY, left)?;
    work.extend(children.iter().rev().map(|child| Work::Render(child, path)));
    Ok(())
}

fn render_into(node: &Value, out: &mut JsStringBuilder) -> Result<()> {
    let left = crate::depth::left();
    let mut work = vec![Work::Render(node, 0)];
    while let Some(item) = work.pop() {
        match item {
            Work::Close(name) => {
                out.push_str("</");
                out.push_js(&name);
                out.push('>');
            }
            Work::Render(node, path) => render_one(node, path, left, out, &mut work)?,
            Work::Owned(node, path) => render_owned(*node, path, left, out, &mut work)?,
        }
    }
    Ok(())
}

/// `node.map(render)`'s checks for an array: its `map` is `Array.prototype.map` and its
/// species constructor is `Array` (V8 names the callee by the dist's source text, `node2`).
fn check_map(node: &Value) -> Result<()> {
    value::array_method(node, "map", Method::ArrayMap, "node2").map_err(throw)?;
    value::species_check(node).map_err(throw)
}

/// One `render(node)` call below `path` of a value the tree holds: what it writes before its
/// first nested call, and the rest as work.
fn render_one<'a>(
    node: &'a Value,
    path: u64,
    left: u64,
    out: &mut JsStringBuilder,
    work: &mut Vec<Work<'a>>,
) -> Result<()> {
    match node {
        // `escapeHtml(String(node))`.
        Value::String(s) => escape_into(s, out),
        Value::Number(n) => out.push_str(&escape_html(&js::number_to_string(*n))),
        // `node.map(render).join('')`: each element present, a hole nothing.
        Value::Array(items) => {
            check_map(node)?;
            let path = deeper(path, cost::ARRAY, left)?;
            match items.as_dense() {
                Some(items) => work.extend(items.iter().rev().map(|item| Work::Render(item, path))),
                None => {
                    let start = work.len();
                    work.extend(items.present().map(|(_, item)| Work::Render(item, path)));
                    work[start..].reverse();
                }
            }
        }
        Value::Tag(tag) => render_tag(tag, deeper(path, cost::TAG, left)?, left, out, work)?,
        Value::Object(_) if TagValue::is_tag(node) => {
            render_tag_like(node, deeper(path, cost::TAG, left)?, left, out, work)?;
        }
        // `null`, `undefined`, a boolean, an object that is not a tag; a built-in object (a
        // function is no object to `typeof`, and none has a `$$mdtype`; `Array.prototype` is
        // an array, of no items).
        Value::Undefined
        | Value::Null
        | Value::Boolean(_)
        | Value::Object(_)
        | Value::Builtin(_) => {}
    }
    Ok(())
}

/// [`render_one`] of a value read through a prototype chain (held here, not by the tree).
fn render_owned(
    node: Value,
    path: u64,
    left: u64,
    out: &mut JsStringBuilder,
    work: &mut Vec<Work<'_>>,
) -> Result<()> {
    match &node {
        Value::Array(items) => {
            check_map(&node)?;
            let path = deeper(path, cost::ARRAY, left)?;
            let start = work.len();
            work.extend(
                items
                    .present()
                    .map(|(_, item)| Work::Owned(Box::new(item.clone()), path)),
            );
            work[start..].reverse();
        }
        Value::Tag(tag) => {
            let mut inner = Vec::new();
            render_tag(tag, deeper(path, cost::TAG, left)?, left, out, &mut inner)?;
            work.extend(inner.into_iter().map(own_work));
        }
        Value::Object(_) if TagValue::is_tag(&node) => {
            render_tag_like(&node, deeper(path, cost::TAG, left)?, left, out, work)?;
        }
        _ => {
            let mut inner = Vec::new();
            render_one(&node, path, left, out, &mut inner)?;
            work.extend(inner.into_iter().map(own_work));
        }
    }
    Ok(())
}

/// Work that borrows from a value about to be dropped, made to hold what it borrows.
fn own_work<'b>(work: Work<'_>) -> Work<'b> {
    match work {
        Work::Render(node, path) => Work::Owned(Box::new(node.clone()), path),
        Work::Owned(node, path) => Work::Owned(node, path),
        Work::Close(name) => Work::Close(Cow::Owned(name.into_owned())),
    }
}

/// ` ${k.toLowerCase()}="${escapeHtml(String(v))}"`.
fn render_attribute(k: &str, v: &Value, out: &mut JsStringBuilder) -> Result<()> {
    let v = v.to_js_string().map_err(throw)?;
    let k: Vec<u16> = k.encode_utf16().collect();
    out.push(' ');
    out.push_units(&js::to_lower_case(&k));
    out.push_str("=\"");
    escape_into(&v, out);
    out.push('"');
    Ok(())
}

/// A `Tag` (its call's `path` charged): `$$mdtype`, `name`, `attributes` and `children` are
/// its own properties.
fn render_tag<'a>(
    tag: &'a TagValue,
    path: u64,
    left: u64,
    out: &mut JsStringBuilder,
    work: &mut Vec<Work<'a>>,
) -> Result<()> {
    if !tag.name.to_boolean() {
        // `return render(children)`.
        return push_children(&tag.children, path, left, work);
    }

    let name: Cow<'a, JsString> = {
        // `String(v)` of the name and of each value runs on this call's stack.
        let _path = crate::depth::enter(path).ok_or_else(range_error)?;
        let name = match &tag.name {
            Value::String(s) => Cow::Borrowed(s),
            other => Cow::Owned(other.to_js_string().map_err(throw)?),
        };
        out.push('<');
        out.push_js(&name);
        // `Object.entries(attributes ?? {})`: the attributes object's own properties.
        for (k, v) in tag.attributes.iter() {
            render_attribute(k, v, out)?;
        }
        name
    };
    out.push('>');

    if matches!(&tag.name, Value::String(s) if s.as_str().is_some_and(|n| VOID_ELEMENTS.contains(&n)))
    {
        return Ok(());
    }

    // `if (children.length) output += render(children)`, then the closing tag.
    work.push(Work::Close(name));
    if !tag.children.is_empty() {
        push_children(&tag.children, path, left, work)?;
    }
    Ok(())
}

/// An object that `Tag.isTag` accepts (its `$$mdtype` is `'Tag'`, own or inherited; its call's
/// `path` charged): its fields are whatever its prototype chain gives.
fn render_tag_like(
    node: &Value,
    path: u64,
    left: u64,
    out: &mut JsStringBuilder,
    work: &mut Vec<Work<'_>>,
) -> Result<()> {
    // `const { name, attributes, children = [] } = node;`, in that order.
    let name = value::get(node, "name").map_err(throw)?;
    let attributes = value::get(node, "attributes").map_err(throw)?;
    let children = value::get(node, "children").map_err(throw)?;
    // `children = []`: `None` is the default.
    let children = (!children.is_undefined()).then_some(children);

    if !name.to_boolean() {
        // `return render(children)`; `render([])` is a call that writes nothing.
        match children {
            Some(children) => work.push(Work::Owned(Box::new(children), path)),
            None => {
                deeper(path, cost::ARRAY, left)?;
            }
        }
        return Ok(());
    }

    // `String(v)` of the name and of each value runs on this call's stack.
    let _path = crate::depth::enter(path).ok_or_else(range_error)?;
    let name_text = name.to_js_string().map_err(throw)?;
    out.push('<');
    out.push_js(&name_text);
    // `Object.entries(attributes ?? {})`.
    let attributes = if attributes.is_nullish() {
        Value::empty_object()
    } else {
        attributes
    };
    for (k, v) in value::own_entries_of(&attributes).map_err(throw)? {
        render_attribute(&k, &v, out)?;
    }
    out.push('>');

    // `voidElements.has(name)`: a string, compared as it is.
    if name
        .as_str()
        .is_some_and(|name| VOID_ELEMENTS.contains(&name))
    {
        return Ok(());
    }

    // `if (children.length) output += render(children)`, then the closing tag.
    let render_children = match &children {
        Some(children) => value::get(children, "length").map_err(throw)?.to_boolean(),
        None => false,
    };
    work.push(Work::Close(Cow::Owned(name_text)));
    if let Some(children) = children.filter(|_| render_children) {
        work.push(Work::Owned(Box::new(children), path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! renderers/html.test.ts, over trees read from JSON; then trees only JavaScript values
    //! hold, each against what node 22 prints for `Markdoc.renderers.html(tree)` (0.5.11).
    use super::*;
    use crate::types::{Scalar, Tag, js_insert};
    use indexmap::IndexMap;

    fn render_ok(node: &RenderableTreeNode) -> String {
        render_tree(node).expect("renders")
    }

    fn render_all_ok(nodes: &[RenderableTreeNode]) -> String {
        render(&Value::array(
            nodes.iter().map(Value::from_render).collect(),
        ))
        .expect("renders")
    }

    #[test]
    fn an_attribute_that_cannot_be_a_string_throws() {
        // node: Markdoc.renderers.html(new Tag('a', {x: {toString: 1}})) throws a TypeError.
        let object = Scalar::Object(IndexMap::from([(
            "toString".to_string(),
            Scalar::Number(1.0),
        )]));
        assert!(render_tree(&tag("a", vec![("x", object.clone())], vec![])).is_err());
        // As a child it is not a tag, and renders as nothing.
        assert_eq!(
            render_ok(&tag("a", vec![], vec![RenderableTreeNode::Scalar(object)])),
            "<a></a>"
        );
    }

    fn tag(
        name: &str,
        attributes: Vec<(&str, Scalar)>,
        children: Vec<RenderableTreeNode>,
    ) -> RenderableTreeNode {
        RenderableTreeNode::Tag(Box::new(Tag {
            name: name.into(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            children,
        }))
    }

    fn text(s: &str) -> RenderableTreeNode {
        RenderableTreeNode::Scalar(Scalar::String(s.into()))
    }

    #[test]
    fn rendering_a_tag() {
        assert_eq!(
            render_ok(&tag("h1", vec![], vec![text("test")])).trim(),
            "<h1>test</h1>"
        );
    }

    #[test]
    fn rendering_string_child_nodes() {
        assert_eq!(
            render_ok(&tag("h1", vec![], vec![text("test "), text("1")])),
            "<h1>test 1</h1>"
        );
    }

    #[test]
    fn rendering_nested_tags() {
        let example = tag("div", vec![], vec![tag("p", vec![], vec![text("test")])]);
        assert_eq!(render_ok(&example), "<div><p>test</p></div>");
    }

    #[test]
    fn rendering_parallel_tags() {
        let example = [
            tag("p", vec![], vec![text("foo")]),
            tag("p", vec![], vec![text("bar")]),
        ];
        assert_eq!(render_all_ok(&example), "<p>foo</p><p>bar</p>");
    }

    #[test]
    fn rendering_a_tag_with_an_invalid_child() {
        let object = Scalar::Object(IndexMap::from([(
            "foo".to_string(),
            Scalar::String("bar".into()),
        )]));
        let example = tag(
            "div",
            vec![],
            vec![text("test"), RenderableTreeNode::Scalar(object)],
        );
        assert_eq!(render_ok(&example), "<div>test</div>");
    }

    #[test]
    fn rendering_a_void_element() {
        assert_eq!(render_ok(&tag("hr", vec![], vec![])), "<hr>");
    }

    #[test]
    fn rendering_a_tag_with_numeric_children() {
        let content = tag(
            "p",
            vec![],
            vec![RenderableTreeNode::Scalar(Scalar::Number(1.0))],
        );
        assert_eq!(render_ok(&content), "<p>1</p>");
    }

    #[test]
    fn lowercase_attributes() {
        let content = tag(
            "td",
            vec![
                ("colSpan", Scalar::Number(2.0)),
                ("rowSpan", Scalar::Number(3.0)),
            ],
            vec![text("Data")],
        );
        assert_eq!(
            render_ok(&content),
            "<td colspan=\"2\" rowspan=\"3\">Data</td>"
        );
    }

    #[test]
    fn attributes_with_basic_value() {
        let example = tag("foo", vec![("bar", Scalar::String("baz".into()))], vec![]);
        assert_eq!(render_ok(&example), "<foo bar=\"baz\"></foo>");
    }

    #[test]
    fn attributes_with_an_id_attribute() {
        let example = tag(
            "h1",
            vec![
                ("id", Scalar::String("foo".into())),
                ("test", Scalar::String("bar".into())),
            ],
            vec![text("test")],
        );
        assert_eq!(render_ok(&example), "<h1 id=\"foo\" test=\"bar\">test</h1>");
    }

    #[test]
    fn attributes_with_a_number_attribute_value() {
        let example = tag(
            "h1",
            vec![("data-foo", Scalar::Number(42.0))],
            vec![text("test")],
        );
        assert_eq!(render_ok(&example), "<h1 data-foo=\"42\">test</h1>");
    }

    #[test]
    fn values_print_as_js_prints_them() {
        let example = tag(
            "x",
            vec![
                (
                    "a",
                    Scalar::Array(vec![
                        Scalar::Number(1.0),
                        Scalar::Null,
                        Scalar::Array(vec![Scalar::Boolean(true)]),
                    ]),
                ),
                ("b", Scalar::Object(IndexMap::new())),
                ("c", Scalar::Null),
                ("d", Scalar::String("<\"&'>".into())),
            ],
            vec![
                RenderableTreeNode::Scalar(Scalar::Boolean(true)),
                text("<&>"),
            ],
        );
        assert_eq!(
            render_ok(&example),
            "<x a=\"1,,true\" b=\"[object Object]\" c=\"null\" d=\"&lt;&quot;&amp;'&gt;\">&lt;&amp;&gt;</x>"
        );
        // A tag without a name renders its children alone.
        assert_eq!(
            render_ok(&tag("", vec![("a", Scalar::Null)], vec![text("x")])),
            "x"
        );
    }

    fn obj(entries: Vec<(&str, Value)>) -> Value {
        let mut out = IndexMap::new();
        for (k, v) in entries {
            js_insert(&mut out, k, v);
        }
        Value::object(out)
    }

    /// `{$$mdtype: 'Tag', ...entries}`: an object that says it is a tag.
    fn pseudo(entries: Vec<(&str, Value)>) -> Value {
        let mut all = vec![("$$mdtype", Value::from("Tag"))];
        all.extend(entries);
        obj(all)
    }

    /// `new Tag(name, attributes, children)`.
    fn new_tag(name: &str, attributes: Vec<(&str, Value)>, children: Vec<Value>) -> Value {
        let Value::Object(attributes) = obj(attributes) else {
            unreachable!()
        };
        TagValue::new(name, (*attributes).clone(), children).into()
    }

    fn arr(items: Vec<Value>) -> Value {
        Value::array(items)
    }

    fn st(s: &str) -> Value {
        Value::from(s)
    }

    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    #[test]
    fn objects_that_say_they_are_tags_render_as_upstream_reads_them() {
        let to_string_1 = || obj(vec![("toString", num(1.0))]);
        let cases: Vec<(Value, Option<&str>)> = vec![
            (
                pseudo(vec![
                    ("name", num(5.0)),
                    ("attributes", obj(vec![])),
                    ("children", arr(vec![st("x")])),
                ]),
                Some("<5>x</5>"),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("attributes", arr(vec![st("q")])),
                    ("children", st("str")),
                ]),
                Some("<b 0=\"q\">str</b>"),
            ),
            // `null.length` throws, after the void-element check.
            (
                pseudo(vec![("name", st("b")), ("children", Value::Null)]),
                None,
            ),
            (
                pseudo(vec![("name", st("hr")), ("children", Value::Null)]),
                Some("<hr>"),
            ),
            (
                pseudo(vec![("name", st("")), ("children", st("abc"))]),
                Some("abc"),
            ),
            (
                pseudo(vec![
                    ("name", st("i")),
                    ("attributes", st("ab")),
                    ("children", arr(vec![])),
                ]),
                Some("<i 0=\"a\" 1=\"b\"></i>"),
            ),
            (
                pseudo(vec![
                    ("name", st("hr")),
                    ("attributes", obj(vec![("x", num(1.0))])),
                    ("children", arr(vec![st("no")])),
                ]),
                Some("<hr x=\"1\">"),
            ),
            (
                pseudo(vec![
                    ("name", Value::Boolean(true)),
                    ("children", arr(vec![st("t")])),
                ]),
                Some("<true>t</true>"),
            ),
            (
                pseudo(vec![
                    ("name", arr(vec![st("a"), st("b")])),
                    ("children", arr(vec![st("t")])),
                ]),
                Some("<a,b>t</a,b>"),
            ),
            (
                pseudo(vec![
                    ("name", obj(vec![])),
                    ("children", arr(vec![st("t")])),
                ]),
                Some("<[object Object]>t</[object Object]>"),
            ),
            // Only the string is a void element.
            (
                pseudo(vec![
                    ("name", arr(vec![st("hr")])),
                    ("children", arr(vec![st("t")])),
                ]),
                Some("<hr>t</hr>"),
            ),
            (pseudo(vec![("name", st("b"))]), Some("<b></b>")),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("children", obj(vec![("length", num(1.0)), ("0", st("x"))])),
                ]),
                Some("<b></b>"),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    (
                        "children",
                        pseudo(vec![("name", st("i")), ("length", num(1.0))]),
                    ),
                ]),
                Some("<b><i></i></b>"),
            ),
            (
                pseudo(vec![("name", st("b")), ("children", num(5.0))]),
                Some("<b></b>"),
            ),
            (
                pseudo(vec![("name", st("b")), ("children", Value::Boolean(true))]),
                Some("<b></b>"),
            ),
            (
                pseudo(vec![("name", st("b")), ("children", st(""))]),
                Some("<b></b>"),
            ),
            (
                pseudo(vec![("name", st("b")), ("children", st("<x>"))]),
                Some("<b>&lt;x&gt;</b>"),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("attributes", Value::Null),
                    ("children", arr(vec![st("k")])),
                ]),
                Some("<b>k</b>"),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("attributes", num(5.0)),
                    ("children", arr(vec![st("k")])),
                ]),
                Some("<b>k</b>"),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("attributes", Value::Boolean(true)),
                    ("children", arr(vec![st("k")])),
                ]),
                Some("<b>k</b>"),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    (
                        "attributes",
                        new_tag("i", vec![("a", num(1.0))], vec![st("c")]),
                    ),
                    ("children", arr(vec![])),
                ]),
                Some(
                    "<b $$mdtype=\"Tag\" name=\"i\" attributes=\"[object Object]\" children=\"c\"></b>",
                ),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("attributes", obj(vec![("X", st("1")), ("0", num(2.0))])),
                    (
                        "children",
                        arr(vec![pseudo(vec![
                            ("name", st("i")),
                            ("children", arr(vec![st("n")])),
                        ])]),
                    ),
                ]),
                Some("<b 0=\"2\" x=\"1\"><i>n</i></b>"),
            ),
            (
                pseudo(vec![
                    ("name", num(0.0)),
                    ("children", arr(vec![st("zero")])),
                ]),
                Some("zero"),
            ),
            (
                pseudo(vec![
                    ("name", Value::Null),
                    ("children", arr(vec![st("nul")])),
                ]),
                Some("nul"),
            ),
            (
                pseudo(vec![("children", arr(vec![st("noname")]))]),
                Some("noname"),
            ),
            // A falsy name renders the children alone: `render(null)` is nothing.
            (pseudo(vec![("children", Value::Null)]), Some("")),
            (
                pseudo(vec![
                    ("name", st("b")),
                    (
                        "attributes",
                        obj(vec![
                            ("a", arr(vec![num(1.0), arr(vec![num(2.0)])])),
                            ("b", Value::Null),
                            ("c", Value::Boolean(true)),
                            ("d", Value::Undefined),
                        ]),
                    ),
                    (
                        "children",
                        arr(vec![
                            num(1.0),
                            Value::Null,
                            Value::Boolean(false),
                            arr(vec![num(2.0), st("<")]),
                            Value::Undefined,
                        ]),
                    ),
                ]),
                Some("<b a=\"1,2\" b=\"null\" c=\"true\" d=\"undefined\">12&lt;</b>"),
            ),
            (
                pseudo(vec![("name", to_string_1()), ("children", arr(vec![]))]),
                None,
            ),
            (
                pseudo(vec![
                    ("name", st("")),
                    ("children", arr(vec![to_string_1()])),
                ]),
                Some(""),
            ),
            // `$$mdtype` must be `'Tag'` exactly.
            (
                obj(vec![
                    ("$$mdtype", st("tag")),
                    ("name", st("b")),
                    ("children", arr(vec![st("x")])),
                ]),
                Some(""),
            ),
            (
                pseudo(vec![
                    ("name", st("b")),
                    ("attributes", st("\u{1F600}")),
                    ("children", arr(vec![])),
                ]),
                // node prints the lone surrogates `\ud83d` and `\ude00`; in UTF-8 they are
                // U+FFFD, as node writes them.
                Some("<b 0=\"\u{FFFD}\" 1=\"\u{FFFD}\"></b>"),
            ),
        ];
        for (tree, expected) in cases {
            let got = render(&tree);
            match expected {
                Some(html) => assert_eq!(got.as_deref().ok(), Some(html), "{tree:?}"),
                None => assert!(got.is_err(), "{tree:?} rendered {got:?}"),
            }
        }
    }

    #[test]
    fn attribute_values_are_strings_as_js_makes_them() {
        let undefined_to_string = || obj(vec![("toString", Value::Undefined)]);
        let cases: Vec<(Value, Option<&str>)> = vec![
            // An own `toString` that is not a function: `valueOf` gives the object back, and
            // `String()` throws "Cannot convert object to primitive value".
            (undefined_to_string(), None),
            (obj(vec![("toString", Value::Null)]), None),
            (
                obj(vec![("toString", num(1.0)), ("valueOf", num(2.0))]),
                None,
            ),
            (arr(vec![num(1.0), undefined_to_string()]), None),
            (
                arr(vec![arr(vec![obj(vec![("toString", num(2.0))])])]),
                None,
            ),
            (
                pseudo(vec![("name", st("i")), ("toString", num(1.0))]),
                None,
            ),
            // `valueOf` is not asked: the inherited `toString` answers first.
            (
                obj(vec![("valueOf", Value::Undefined)]),
                Some("[object Object]"),
            ),
            (obj(vec![("valueOf", num(1.0))]), Some("[object Object]")),
            (
                arr(vec![num(1.0), obj(vec![("valueOf", Value::Undefined)])]),
                Some("1,[object Object]"),
            ),
            (new_tag("i", vec![], vec![]), Some("[object Object]")),
        ];
        for (value, expected) in cases {
            let tree = new_tag("div", vec![("a", value.clone())], vec![]);
            let got = render(&tree);
            match expected {
                Some(text) => assert_eq!(
                    got.as_deref().ok(),
                    Some(format!("<div a=\"{text}\"></div>").as_str()),
                    "{value:?}"
                ),
                None => assert!(got.is_err(), "{value:?} rendered {got:?}"),
            }
        }
        // A child is not converted: an object that is not a tag renders as nothing.
        assert_eq!(
            render(&new_tag("div", vec![], vec![undefined_to_string()])).ok(),
            Some("<div></div>".into())
        );
        // Keys lower-cased as JS does (final sigma, U+0130, U+1E9E, the Kelvin sign).
        assert_eq!(
            render(&new_tag(
                "div",
                vec![
                    ("ΑΣ", num(1.0)),
                    ("İd", num(2.0)),
                    ("ẞ", num(3.0)),
                    ("\u{212A}", num(4.0)),
                ],
                vec![],
            ))
            .ok(),
            Some("<div ας=\"1\" i̇d=\"2\" ß=\"3\" k=\"4\"></div>".into())
        );
        assert_eq!(
            render(&new_tag(
                "p",
                vec![],
                vec![num(1e21), num(-0.0), num(0.1), num(1.5e-7)]
            ))
            .ok(),
            Some("<p>1e+2100.11.5e-7</p>".into())
        );
    }

    /// `'😀'[0]` and `'😀'[1]`, as a variable path reads them.
    fn halves() -> (Value, Value) {
        let e = st("\u{1F600}");
        (
            value::get_property(&e, "0").unwrap(),
            value::get_property(&e, "1").unwrap(),
        )
    }

    fn concat(a: &str, b: &Value, c: &str) -> Value {
        let Value::String(b) = b else {
            unreachable!("a string")
        };
        Value::String(JsString::from(a).concat(b).concat(&JsString::from(c)))
    }

    #[test]
    fn halves_that_meet_in_the_output_are_the_character() {
        // Each against node 22's Markdoc.renderers.html (UTF-8: a lone surrogate is U+FFFD).
        let (l, r) = halves();
        let f = "\u{FFFD}";
        let cases: Vec<(Value, String)> = vec![
            // html([L, R]) === '😀'
            (arr(vec![l.clone(), r.clone()]), "\u{1F600}".into()),
            // html(new Tag('p', {}, [L, R])) === '<p>😀</p>'
            (
                new_tag("p", vec![], vec![l.clone(), r.clone()]),
                "<p>\u{1F600}</p>".into(),
            ),
            // html([R, L]) === '\ude00\ud83d'
            (arr(vec![r.clone(), l.clone()]), format!("{f}{f}")),
            // html(new Tag('div', {a: L})) === '<div a="\ud83d"></div>'
            (
                new_tag("div", vec![("a", l.clone())], vec![]),
                format!("<div a=\"{f}\"></div>"),
            ),
            // html([L, new Tag('b', {}, [R])]) === '\ud83d<b>\ude00</b>'
            (
                arr(vec![l.clone(), new_tag("b", vec![], vec![r.clone()])]),
                format!("{f}<b>{f}</b>"),
            ),
            // html([L, '&', R]) === '\ud83d&amp;\ude00'
            (
                arr(vec![l.clone(), st("&"), r.clone()]),
                format!("{f}&amp;{f}"),
            ),
            // html(['<' + L, R + '>']) === '&lt;😀&gt;'
            (
                arr(vec![concat("<", &l, ""), concat("", &r, ">")]),
                "&lt;\u{1F600}&gt;".into(),
            ),
            // html(new Tag('', {}, [L, new Tag('', {}, [R])])) === '😀'
            (
                new_tag(
                    "",
                    vec![],
                    vec![l.clone(), new_tag("", vec![], vec![r.clone()])],
                ),
                "\u{1F600}".into(),
            ),
            // html({$$mdtype: 'Tag', name: L, children: [R]}) === '<\ud83d>\ude00</\ud83d>'
            (
                pseudo(vec![
                    ("name", l.clone()),
                    ("children", arr(vec![r.clone()])),
                ]),
                format!("<{f}>{f}</{f}>"),
            ),
            // html(new Tag('p', {x: [L, R]})) === '<p x="\ud83d,\ude00"></p>'
            (
                new_tag("p", vec![("x", arr(vec![l.clone(), r.clone()]))], vec![]),
                format!("<p x=\"{f},{f}\"></p>"),
            ),
            // html([[[L]], [[R]]]) === '😀'
            (
                arr(vec![
                    arr(vec![arr(vec![l.clone()])]),
                    arr(vec![arr(vec![r])]),
                ]),
                "\u{1F600}".into(),
            ),
        ];
        for (tree, expected) in cases {
            assert_eq!(render(&tree).ok(), Some(expected), "{tree:?}");
        }
    }

    /// Runs `f` on a thread with the robustness policy's 2 MiB stack.
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(f)
            .expect("a thread")
            .join()
            .expect("no panic")
    }

    fn is_range_error(result: Result<String>) -> bool {
        matches!(result, Err(MarkdocError::TransformError(ref e)) if e == value::RANGE_ERROR)
    }

    #[test]
    fn renders_as_deep_as_a_fresh_node_and_throws_past_it() {
        on_small_stack(|| {
            let wrap = |depth: usize, inner: Value, f: &dyn Fn(Value) -> Value| {
                (0..depth).fold(inner, |v, _| f(v))
            };
            let array = |v: Value| arr(vec![v]);
            let tag = |v: Value| new_tag("b", vec![], vec![v]);
            let nameless = |v: Value| new_tag("", vec![], vec![v]);
            let pseudo_tag = |v: Value| pseudo(vec![("name", st("b")), ("children", arr(vec![v]))]);
            // node (a fresh process, the first render, from the top level of a script):
            // 2,005 arrays around a string; the port's cost is a level more (see `cost`).
            let n = MAX_RENDER_DEPTH;
            let deep = wrap(n, st("x"), &array);
            assert_eq!(render(&deep).unwrap(), "x");
            assert!(is_range_error(render(&wrap(n + 1, st("x"), &array))));
            // An empty array at the bottom is a call too; `null` is one that writes nothing.
            assert!(render(&wrap(n - 1, arr(vec![]), &array)).is_ok());
            assert!(is_range_error(render(&wrap(n, arr(vec![]), &array))));
            // A tag is its own call and its children array's: 1,253 tags, as node renders.
            let tags = 1_253;
            assert_eq!(
                (crate::depth::STACK / (cost::TAG + cost::ARRAY)) as usize,
                tags
            );
            let ok = render(&wrap(tags, st("x"), &tag)).unwrap();
            assert!(ok.starts_with("<b><b>") && ok.ends_with("</b></b>"));
            assert!(is_range_error(render(&wrap(tags + 1, st("x"), &tag))));
            assert_eq!(render(&wrap(tags, st("x"), &nameless)).unwrap(), "x");
            assert!(is_range_error(render(&wrap(tags + 1, st("x"), &nameless))));
            assert!(render(&wrap(tags, st("x"), &pseudo_tag)).is_ok());
            assert!(is_range_error(render(&wrap(
                tags + 1,
                st("x"),
                &pseudo_tag
            ))));
            // The trees drop in a loop on this stack, as deep as they come.
            drop(wrap(500_000, st("x"), &tag));
            assert!(is_range_error(render(&wrap(200_000, st("x"), &pseudo_tag))));
            // From a render tree read from JSON, any depth.
            let mut node = RenderableTreeNode::Scalar(Scalar::String("x".into()));
            for _ in 0..tags {
                node = RenderableTreeNode::Tag(Box::new(Tag {
                    name: "i".into(),
                    attributes: IndexMap::new(),
                    children: vec![node],
                }));
            }
            assert!(render_tree(&node).is_ok());
            // (`node` is the render tree's recursive type: built here only as deep as its
            // derived drop allows on this stack.)
        });
    }

    #[test]
    fn values_are_read_through_their_prototypes() {
        use crate::next::schema::value::{Object, Proto};
        use crate::types::JsObject;
        // node: html(Object.setPrototypeOf({}, new Tag('p', {a: 1}, ['x']))) is
        // `<p a="1">x</p>`: Tag.isTag and `{name, attributes, children}` read through it.
        let tag: Value = TagValue::new(
            "p",
            Object::from([("a", Value::Number(1.0))]),
            vec![Value::from("x")],
        )
        .into();
        let v = Value::object_with_proto(JsObject::new(), Proto::Value(tag));
        assert_eq!(render(&v).unwrap(), "<p a=\"1\">x</p>");
        // html(Object.setPrototypeOf({x: 1}, [1, 'a'])) is "": not an array.
        let v = Value::object_with_proto(
            JsObject::from([("x", Value::Number(1.0))]),
            Proto::Value(Value::array(vec![Value::Number(1.0), Value::from("a")])),
        );
        assert_eq!(render(&v).unwrap(), "");
        // html([1, , 'b']) is "1b": a hole renders nothing.
        let holes = Value::sparse_array(
            3,
            [(0, Value::Number(1.0)), (2, Value::from("b"))],
            indexmap::IndexMap::new(),
        );
        assert_eq!(render(&holes).unwrap(), "1b");
        // html(Object.assign(['a'], {map: 1})): TypeError: node2.map is not a function.
        let mut named = indexmap::IndexMap::new();
        named.insert("map".to_string(), Value::Number(1.0));
        let a = Value::sparse_array(1, [(0, Value::from("a"))], named);
        assert!(matches!(
            render(&a),
            Err(MarkdocError::TransformError(m)) if m == "TypeError: node2.map is not a function"
        ));
    }
}
