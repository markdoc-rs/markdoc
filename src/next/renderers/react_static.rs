//! Port of upstream Markdoc v0.5.11's `src/renderers/react/static.ts`:
//! `renderers.reactStatic(node, {resolveTagName})`, the JavaScript source of a function that
//! builds a render tree's React elements.
//!
//! Owner: the api agent.
//!
//! The source is upstream's, character for character:
//!
//! ```text
//!
//!   (({components = {}} = {}) => {
//!     <resolveTagName's source>
//!     return <the tree>;
//!   })
//!
//! ```
//!
//! where a tag is `React.createElement(\n    tagName(<name as JSON>, components),\n
//! <props>,\n    <children>)`, an array `React.createElement(React.Fragment, null,
//! <items>)`, any other value its `JSON.stringify` (nothing where that is `undefined`), the
//! props `null` or `deepRender(attrs)` (an object `{"key": value, ...}`, an array `[...]`, a
//! tag its element, anything else its JSON), and items and children joined with `, `. A tag
//! is read as the React renderer reads it ([`super::react`]).
//!
//! `resolveTagName` is a JavaScript function in upstream, and only its name and its source
//! are used: the name must be `tagName` (`Error: resolveTagName must be named tagName`
//! otherwise) and the source is written into the output. [`ResolveTagName`] is that name and
//! that source; [`DEFAULT_RESOLVE_TAG_NAME`] is upstream's default.
//!
//! `Err` is also where upstream throws while it renders (see [`super::react`]), and where
//! node's stack would run out (`RangeError: Maximum call stack size exceeded`), with the
//! React renderer's model ([`NODE_LIMITS`] are this renderer's fresh-process limits).
//!
//! # Divergences from upstream
//!
//! - The default `resolveTagName`: upstream's source names it `tagName`, but the published
//!   dist (0.5.11, bundled by esbuild) renamed it `tagName2`, so `reactStatic(tree)` there
//!   always throws `Error: resolveTagName must be named tagName`. The port follows the source:
//!   its default is the dist's function under the source's name (its text is the dist's), and
//!   renders. This is a JavaScript bundling artifact, not upstream's design.

use super::react::{Attrs, Cost, Name, destructure, level};
use crate::next::schema::value::{self, TagValue, Value};
use crate::next::stack;
use crate::types::JsObject;
use crate::types::{MarkdocError, Result};
use std::borrow::Cow;

/// How deep node 22 renders each shape of [`super::react::NODE_LIMITS`] with `reactStatic`
/// before `RangeError: Maximum call stack size exceeded`, fresh and warmed up.
pub const NODE_LIMITS: [(&str, usize, usize); 5] = [
    ("tags", 2280, 10477),
    ("arrays", 2280, 5127),
    ("attr_arrays", 2458, 3684),
    ("attr_objects", 1989, 9183),
    ("attr_tags", 1254, 1937),
];

const KB: u64 = 1024;
/// What a level of each site costs (see [`super::react`]'s costs).
const COST_RENDER: Cost = Cost(463302764, 4 * KB);
const COST_RENDER_ATTR: Cost = Cost(311527064, 6 * KB);
const COST_DEEP_OBJECT: Cost = Cost(530569531, 5 * KB / 2);
const COST_DEEP_ARRAY: Cost = Cost(429354518, 5 * KB / 2);

/// A `resolveTagName` function: what upstream reads of it, its `name` and its source
/// (`Function.prototype.toString`).
#[derive(Clone, Copy, Debug)]
pub struct ResolveTagName<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

/// Upstream's default `resolveTagName` (`tagName`), with the source text the dist has for it.
pub const DEFAULT_RESOLVE_TAG_NAME: ResolveTagName<'static> = ResolveTagName {
    name: "tagName",
    source: "function tagName(name, components) {\n  return typeof name !== \"string\" ? \
             \"Fragment\" : name[0] !== name[0].toUpperCase() ? name : components instanceof \
             Function ? components(name) : components[name];\n}",
};

/// `reactStatic`'s options.
#[derive(Clone, Copy, Debug, Default)]
pub struct StaticOptions<'a> {
    /// `resolveTagName` (default [`DEFAULT_RESOLVE_TAG_NAME`]).
    pub resolve_tag_name: Option<ResolveTagName<'a>>,
}

fn throw(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

/// `reactStatic(node, {resolveTagName})` (see the module's documentation).
pub fn render(node: &Value, options: &StaticOptions<'_>) -> Result<String> {
    let resolve_tag_name = options.resolve_tag_name.unwrap_or(DEFAULT_RESOLVE_TAG_NAME);
    // the resolveTagName function *must* be called tagName
    // throw an error if it does not
    if resolve_tag_name.name != "tagName" {
        return Err(throw("Error: resolveTagName must be named tagName"));
    }
    let mut out = String::from("\n  (({components = {}} = {}) => {\n    ");
    out.push_str(resolve_tag_name.source);
    out.push_str("\n    return ");
    if !render_node(node, COST_RENDER, &mut out)? {
        out.push_str("undefined");
    }
    out.push_str(";\n  })\n");
    Ok(out)
}

/// `JSON.stringify(value)` written to `out`: `false` (nothing written) where it is
/// `undefined`.
fn json(value: &Value, out: &mut String) -> Result<bool> {
    if let Value::String(s) = value {
        value::quote_json_js_string(s, out);
        return Ok(true);
    }
    match value::json_stringify(value, "").map_err(throw)? {
        Some(json) => {
            out.push_str(&json);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `renderArray(children)`: `children.map(render).join(', ')`.
fn render_array(children: &[Value], out: &mut String) -> Result<()> {
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        render_node(child, COST_RENDER, out)?;
    }
    Ok(())
}

/// `deepRender(value)` written to `out` (`false` where it is `undefined`).
fn deep_render(value: &Value, out: &mut String) -> Result<bool> {
    // A built-in object a document reached (`typeof` `object`, not a function): an array's
    // elements (`Array.prototype` has none) or `Object.entries` (a wrapper prototype's
    // primitive value is no entry: `Boolean.prototype` is `{}`, not `false`).
    if let Value::Builtin(b) = value
        && !b.is_callable()
    {
        if b.is_array() {
            out.push_str("[]");
        } else {
            let entries = value::own_entries(value);
            deep_render_entries(entries.iter().map(|(k, v)| (k.as_str(), v)), out)?;
        }
        return Ok(true);
    }
    let cost = match value {
        Value::Array(_) => COST_DEEP_ARRAY,
        Value::Object(_) if !TagValue::is_tag(value) => COST_DEEP_OBJECT,
        // a tag is charged as `render`'s
        Value::Object(_) | Value::Tag(_) => return render_node(value, COST_RENDER_ATTR, out),
        // `value == null || typeof value !== 'object'`
        _ => return json(value, out),
    };
    let _level = level(cost)?;
    stack::grow(|| {
        match value {
            Value::Array(items) => {
                out.push('[');
                // (`map` skips an array's holes.)
                for (i, (_, item)) in items.present().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    deep_render(item, out)?;
                }
                out.push(']');
            }
            Value::Object(o) => deep_render_entries(o.iter().map(|(k, v)| (k.as_str(), v)), out)?,
            _ => unreachable!("an array or an object"),
        }
        Ok(true)
    })
}

/// `` `{${Object.entries(value).map(([k, v]) => [JSON.stringify(k), deepRender(v)].join(': '))
/// .join(', ')}}` `` over the entries given.
fn deep_render_entries<'a>(
    entries: impl Iterator<Item = (&'a str, &'a Value)>,
    out: &mut String,
) -> Result<()> {
    out.push('{');
    for (i, (k, v)) in entries.enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        value::quote_json_string(k, out);
        out.push_str(": ");
        deep_render(v, out)?;
    }
    out.push('}');
    Ok(())
}

/// `render(node)` written to `out` (`false` where it is `undefined`), at `cost` (a value that
/// is neither a tag nor an array is written at once, uncharged).
fn render_node(node: &Value, cost: Cost, out: &mut String) -> Result<bool> {
    if !matches!(node, Value::Array(_)) && !TagValue::is_tag(node) {
        return json(node, out);
    }
    let _level = level(cost)?;
    stack::grow(|| render_inner(node, out))
}

fn render_inner(node: &Value, out: &mut String) -> Result<bool> {
    if let Value::Array(items) = node {
        out.push_str("React.createElement(React.Fragment, null, ");
        // (`map` skips an array's holes.)
        match items.as_dense() {
            Some(items) => render_array(items, out)?,
            None => render_array(
                &items.present().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
                out,
            )?,
        }
        out.push(')');
        return Ok(true);
    }

    if !TagValue::is_tag(node) {
        return json(node, out);
    }

    let mut d = destructure(node)?;
    if d.class_name.to_boolean() {
        let class_name = d.class_name.clone();
        match d.attrs.iter_mut().find(|(k, _)| k == "className") {
            Some((_, v)) => *v = class_name,
            None => d.attrs.push((Cow::Borrowed("className"), class_name)),
        }
    }

    out.push_str("React.createElement(\n    tagName(");
    let name = match &d.name {
        Name::String(s) => {
            value::quote_json_js_string(s, out);
            true
        }
        Name::Value(v) => json(v, out)?,
    };
    if !name {
        out.push_str("undefined");
    }
    out.push_str(", components),\n    ");
    if d.attrs.is_empty() {
        out.push_str("null");
    } else {
        deep_render_attrs(d.attrs, out)?;
    }
    out.push_str(",\n    ");
    render_array(d.children.items()?, out)?;
    out.push(')');
    Ok(true)
}

/// `deepRender(attrs)` of a tag's attributes (`attrs` a new object: its own properties).
fn deep_render_attrs(attrs: Attrs<'_>, out: &mut String) -> Result<()> {
    let is_tag = attrs
        .iter()
        .any(|(k, v)| k == "$$mdtype" && matches!(v.as_str(), Some("Tag")));
    if is_tag {
        // the attributes are a tag: `render(attrs)`
        let object = Value::object(
            attrs
                .into_iter()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect::<JsObject<Value>>(),
        );
        deep_render(&object, out)?;
        return Ok(());
    }
    let _level = level(COST_DEEP_OBJECT)?;
    stack::grow(|| deep_render_entries(attrs.iter().map(|(k, v)| (k.as_ref(), v.as_ref())), out))
}

#[cfg(test)]
mod tests {
    //! What node writes for the same tree (`findings/conformance/work/api/unit_values.cjs`,
    //! with upstream's default `tagName` under its source name).
    use super::*;
    use crate::next::index::{self, thrown};
    use crate::next::parser::ParserArgs;
    use crate::next::schema::Config;

    const TAG_NAME: ResolveTagName<'static> = ResolveTagName {
        name: "tagName",
        source: "function tagName(name, components) { return typeof name !== 'string' ? \
                 'Fragment' : name[0] !== name[0].toUpperCase() ? name : components instanceof \
                 Function ? components(name) : components[name]; }",
    };

    fn wrap(tree: &str) -> String {
        format!(
            "\n  (({{components = {{}}}} = {{}}) => {{\n    {}\n    return {tree};\n  }})\n",
            TAG_NAME.source
        )
    }

    #[test]
    fn a_document_and_odd_values() {
        let options = StaticOptions {
            resolve_tag_name: Some(TAG_NAME),
        };
        let ast =
            index::parse("# Hi {% .a %}\n\nSome *em* text\n", &ParserArgs::default()).unwrap();
        let values = index::transform(&ast, &Config::default()).unwrap();
        assert_eq!(
            render(&values, &options).unwrap(),
            wrap(
                "React.createElement(\n    tagName(\"article\", components),\n    null,\n    \
                 React.createElement(\n    tagName(\"h1\", components),\n    {\"className\": \
                 \"a\"},\n    \"Hi \"), React.createElement(\n    tagName(\"p\", components),\n    \
                 null,\n    \"Some \", React.createElement(\n    tagName(\"em\", components),\n    \
                 null,\n    \"em\"), \" text\"))"
            )
        );
        // undefined writes nothing (a member's value, an item), or `undefined` at the top.
        let mut attributes = JsObject::new();
        attributes.insert("u", Value::Undefined);
        attributes.insert("n", Value::Null);
        attributes.insert("f", Value::Number(1.5));
        attributes.insert("s", Value::from("q\""));
        let tree = Value::from(crate::next::schema::value::TagValue::new(
            "a",
            attributes,
            vec![
                Value::Undefined,
                Value::from("x"),
                Value::Null,
                Value::Number(1.0),
            ],
        ));
        assert_eq!(
            render(&tree, &options).unwrap(),
            wrap(
                "React.createElement(\n    tagName(\"a\", components),\n    {\"u\": , \"n\": null, \
                 \"f\": 1.5, \"s\": \"q\\\"\"},\n    , \"x\", null, 1)"
            )
        );
        assert_eq!(
            render(&Value::Undefined, &options).unwrap(),
            wrap("undefined")
        );
    }

    #[test]
    fn the_resolve_tag_name_must_be_named_tag_name() {
        let options = StaticOptions {
            resolve_tag_name: Some(ResolveTagName {
                name: "tagName2",
                source: "function tagName2() {}",
            }),
        };
        assert_eq!(
            thrown(&render(&Value::Null, &options).unwrap_err()),
            "Error: resolveTagName must be named tagName"
        );
        // The default is upstream's source's `tagName` (the dist renamed it: see the module's
        // documentation).
        assert!(render(&Value::Null, &StaticOptions::default()).is_ok());
    }
}
