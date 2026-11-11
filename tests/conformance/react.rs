//! Our renderable tree, put through upstream's React renderer and the
//! corpus's React shim -- the shape `expected:` is written in.
//!
//! The rules, each from upstream source at 9440a1d (read as text):
//!
//! * react.ts:49-51  an array child becomes `createElement(Fragment, null,
//!   ...items.map(render))`; the shim's Fragment is the string 'Fragment'.
//! * react.ts:53-54  anything that is not a Tag is returned unchanged
//!   (strings, numbers, booleans, null, plain objects).
//! * react.ts:56-62  `class` is pulled out and re-added as `className` only
//!   if truthy; a falsy `class` disappears.
//! * react.ts:66     no attributes left -> `null`; otherwise `deepRender`,
//!   which keeps scalars, maps arrays, recurses into plain objects.
//! * react.ts:13-24  `resolveTagName`: a name whose first character is not
//!   changed by toUpperCase() is looked up in `components` (empty in the
//!   runner) and so becomes `undefined`.
//! * react-shim.ts:9-18 `{tag?, attributes?, children?}`: `tag` only if
//!   truthy, `attributes` only if truthy (null -> absent), `children` only if
//!   non-empty, flattened.
//!
//! An attribute value is a `Scalar`, and a `Scalar::Tag` in one is rendered
//! as deepRender renders a Tag it meets (react.ts:41).

use markdoc::types::{RenderableTreeNode, Scalar, Tag};

use crate::value::V;

pub fn render(node: &RenderableTreeNode) -> V {
    match node {
        RenderableTreeNode::Tag(tag) => render_tag(tag),
        RenderableTreeNode::Scalar(s) => render_scalar(s),
    }
}

fn render_scalar(s: &Scalar) -> V {
    match s {
        Scalar::Array(items) => shim(
            Some("Fragment".to_string()),
            None,
            items.iter().map(render_scalar).collect(),
        ),
        other => deep(other),
    }
}

/// `deepRender` over a Scalar (and the unchanged pass-through of non-Tags).
fn deep(s: &Scalar) -> V {
    match s {
        // A render tree is JSON: it holds no `undefined`.
        Scalar::Undefined | Scalar::Null => V::Null,
        Scalar::Boolean(b) => V::Bool(*b),
        Scalar::Number(n) => V::Num(*n),
        Scalar::String(s) => V::Str(s.clone()),
        Scalar::Array(a) => V::Arr(a.iter().map(deep).collect()),
        Scalar::Object(o) => V::Obj(o.iter().map(|(k, v)| (k.clone(), deep(v))).collect()),
        Scalar::Tag(tag) => render_tag(tag),
        // Resolved before anything renders: a render tree never holds one.
        Scalar::Variable(_) | Scalar::Function(_) => V::Null,
        // A render tree is JSON (`Value::to_render`): it holds no JavaScript object of its own.
        Scalar::Js(_) => V::Null,
    }
}

fn js_truthy(s: &Scalar) -> bool {
    match s {
        Scalar::Undefined | Scalar::Null => false,
        Scalar::Boolean(b) => *b,
        Scalar::Number(n) => *n != 0.0 && !n.is_nan(),
        Scalar::String(s) => !s.is_empty(),
        Scalar::Array(_)
        | Scalar::Object(_)
        | Scalar::Variable(_)
        | Scalar::Function(_)
        | Scalar::Tag(_)
        | Scalar::Js(_) => true,
    }
}

fn resolve_tag_name(name: &str) -> Option<String> {
    let first = name.chars().next()?;
    let upper: String = first.to_uppercase().collect();
    if upper != first.to_string() {
        Some(name.to_string())
    } else {
        // components[name], and the runner passes no components.
        None
    }
}

fn render_tag(tag: &Tag) -> V {
    let mut attrs: Vec<(String, V)> = Vec::new();
    let mut class_name = None;
    // The attributes' own order (diffJson ignores object key order anyway).
    for (k, v) in &tag.attributes {
        if k == "class" {
            if js_truthy(v) {
                class_name = Some(deep(v));
            }
            continue;
        }
        attrs.push((k.clone(), deep(v)));
    }
    if let Some(c) = class_name {
        attrs.push(("className".to_string(), c));
    }
    let children = tag.children.iter().map(render).collect();
    shim(
        resolve_tag_name(&tag.name),
        if attrs.is_empty() { None } else { Some(attrs) },
        children,
    )
}

fn shim(tag: Option<String>, attributes: Option<Vec<(String, V)>>, children: Vec<V>) -> V {
    let mut out = Vec::new();
    if let Some(t) = tag.filter(|t| !t.is_empty()) {
        out.push(("tag".to_string(), V::Str(t)));
    }
    if let Some(a) = attributes {
        out.push(("attributes".to_string(), V::Obj(a)));
    }
    if !children.is_empty() {
        let mut flat = Vec::new();
        flatten(children, &mut flat);
        out.push(("children".to_string(), V::Arr(flat)));
    }
    V::Obj(out)
}

fn flatten(items: Vec<V>, out: &mut Vec<V>) {
    for i in items {
        match i {
            V::Arr(inner) => flatten(inner, out),
            other => out.push(other),
        }
    }
}

/// index.ts:61-62: `(render(...) || {}).children || []`.
pub fn article_children(tree: &RenderableTreeNode) -> V {
    match render(tree) {
        V::Obj(entries) => entries
            .into_iter()
            .find(|(k, _)| k == "children")
            .map(|(_, v)| v)
            .unwrap_or(V::Arr(Vec::new())),
        _ => V::Arr(Vec::new()),
    }
}
