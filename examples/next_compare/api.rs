//! The api stages' way into `markdoc::next`: `format` (the formatter), `reactStatic` and
//! `react` (the React renderers, with a fake React that records its calls as
//! tests/next/oracle.cjs's does).

use indexmap::IndexMap;
use markdoc::ast::Node;
use markdoc::markdown_it::js;
use markdoc::next::formatter::{self, FormatOptions, OrderedListMode};
use markdoc::next::index;
use markdoc::next::renderers::react::{self, Components, ElementType, Prop, Rendered};
use markdoc::next::renderers::react_static;
use markdoc::next::schema::value::{JsString, Value as JsValue, own_entries, quote_json_js_string};
use serde_json::{Map, Value};

/// JavaScript's truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Markdoc.format(ast, formatOptions)`. The options as the request has them:
/// `allowIndentation` (truthiness), `maxTagOpeningWidth` (a number, or a string read as
/// `Number()` reads it: `"Infinity"`), `orderedListMode`, `indent` (a number).
pub fn format(ast: &Node, options: &Map<String, Value>) -> Result<String, String> {
    let options = FormatOptions {
        allow_indentation: options.get("allowIndentation").is_some_and(truthy),
        max_tag_opening_width: match options.get("maxTagOpeningWidth") {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => Some(js::string_to_number(&js::to_utf16(s))),
            _ => None,
        },
        ordered_list_mode: options.get("orderedListMode").map(|m| {
            if m.as_str() == Some("increment") {
                OrderedListMode::Increment
            } else {
                OrderedListMode::Repeat
            }
        }),
        parent: None,
        indent: options.get("indent").and_then(Value::as_f64),
    };
    formatter::format(ast, &options).map_err(|e| index::thrown(&e).to_owned())
}

/// `Markdoc.renderers.reactStatic(tree)`, with upstream's default `resolveTagName` (which the
/// oracle passes explicitly; see oracle.cjs).
pub fn react_static(tree: &JsValue) -> Result<String, String> {
    react_static::render(tree, &react_static::StaticOptions::default())
        .map_err(|e| index::thrown(&e).to_owned())
}

/// A fake React: each `createElement` call recorded as `{type, props, children}`, an element
/// being the index of the call that made it.
struct Recorder {
    calls: Vec<String>,
}

impl react::React for Recorder {
    type Element = usize;
    type Component = Value;

    fn create_element(
        &mut self,
        element_type: ElementType<'_, Value>,
        props: Option<Prop<'_, usize>>,
        children: Vec<Rendered<'_, usize>>,
    ) -> usize {
        let mut out = String::from("{\"type\":");
        match element_type {
            ElementType::Fragment => out.push_str("{\"$fragment\":true}"),
            ElementType::Name(name) => write_value(&mut out, &name.to_value()),
            ElementType::Component(Some(component)) => out.push_str(&component.to_string()),
            ElementType::Component(None) => out.push_str("{\"$undefined\":true}"),
            // Object.prototype (no own enumerable keys), or one of its methods
            ElementType::Inherited(name) if name == "__proto__" => out.push_str("{}"),
            ElementType::Inherited(name) => {
                out.push_str("{\"$function\":");
                quote_json_js_string(&JsString::from(name.as_str()), &mut out);
                out.push('}');
            }
        }
        out.push_str(",\"props\":");
        match props {
            None => out.push_str("null"),
            Some(props) => write_prop(&mut out, &props),
        }
        out.push_str(",\"children\":[");
        for (i, child) in children.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_rendered(&mut out, child);
        }
        out.push_str("]}");
        self.calls.push(out);
        self.calls.len() - 1
    }
}

fn write_rendered(out: &mut String, rendered: &Rendered<'_, usize>) {
    match rendered {
        Rendered::Element(i) => out.push_str(&format!("{{\"$element\":{i}}}")),
        Rendered::Value(v) => write_value(out, v),
    }
}

fn write_props(out: &mut String, props: &react::Props<'_, usize>) {
    out.push('{');
    for (i, (k, v)) in props.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        quote_json_js_string(&JsString::from(k.as_ref()), out);
        out.push(':');
        write_prop(out, v);
    }
    out.push('}');
}

fn write_prop(out: &mut String, prop: &Prop<'_, usize>) {
    match prop {
        Prop::Value(v) => write_value(out, v),
        Prop::Element(i) => out.push_str(&format!("{{\"$element\":{i}}}")),
        Prop::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_prop(out, item);
            }
            out.push(']');
        }
        Prop::Object(props) => write_props(out, props),
    }
}

/// A JavaScript value as oracle.cjs's `enc` writes it, at any depth (a loop).
fn write_value(out: &mut String, value: &JsValue) {
    enum Item {
        Value(JsValue),
        Text(&'static str),
        Key(String),
    }
    let mut stack = vec![Item::Value(value.clone())];
    while let Some(item) = stack.pop() {
        let value = match item {
            Item::Text(t) => {
                out.push_str(t);
                continue;
            }
            Item::Key(k) => {
                quote_json_js_string(&JsString::from(k.as_str()), out);
                out.push(':');
                continue;
            }
            Item::Value(v) => v,
        };
        match &value {
            JsValue::Undefined => out.push_str("{\"$undefined\":true}"),
            JsValue::Null => out.push_str("null"),
            JsValue::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
            JsValue::Number(n) => {
                if n.is_nan() {
                    out.push_str("{\"$number\":\"NaN\"}");
                } else if n.is_infinite() {
                    out.push_str(if *n > 0.0 {
                        "{\"$number\":\"Infinity\"}"
                    } else {
                        "{\"$number\":\"-Infinity\"}"
                    });
                } else if *n == 0.0 && n.is_sign_negative() {
                    out.push_str("{\"$number\":\"-0\"}");
                } else {
                    out.push_str(&js::number_to_string(*n));
                }
            }
            JsValue::String(s) => quote_json_js_string(s, out),
            JsValue::Array(items) => {
                out.push('[');
                stack.push(Item::Text("]"));
                // (A hole is `null`, as JSON writes it.)
                for i in (0..items.len()).rev() {
                    stack.push(Item::Value(items.get(i).cloned().unwrap_or(JsValue::Null)));
                    if i > 0 {
                        stack.push(Item::Text(","));
                    }
                }
            }
            // A built-in function a document reached: as the recorder writes a function,
            // `{$function: f.name}` (its own `name`).
            JsValue::Builtin(b) if b.is_callable() => {
                let name = match b.own("name").map(|p| &p.slot) {
                    Some(markdoc::realm::Slot::Data(markdoc::realm::Data::String(name))) => *name,
                    _ => "",
                };
                out.push_str("{\"$function\":");
                quote_json_js_string(&JsString::from(name), out);
                out.push('}');
            }
            // `Array.prototype` is an array (`Array.isArray`), with no elements: `[]`.
            JsValue::Builtin(b) if b.is_array() => out.push_str("[]"),
            // (Any other built-in object: its own enumerable properties.)
            JsValue::Object(_) | JsValue::Tag(_) | JsValue::Builtin(_) => {
                out.push('{');
                stack.push(Item::Text("}"));
                let entries = own_entries(&value);
                for (i, (k, v)) in entries.into_iter().enumerate().rev() {
                    stack.push(Item::Value(v));
                    stack.push(Item::Key(k));
                    if i > 0 {
                        stack.push(Item::Text(","));
                    }
                }
            }
        }
    }
}

/// `Markdoc.renderers.react(tree, React, {components})` with the recording React: the JSON
/// text `{calls, result}`. `components` is the request's: an object of JSON values, or
/// `"$function"` for `(name) => ({$component: name})`.
pub fn react(tree: &JsValue, components: Option<&Value>) -> Result<String, String> {
    let mut recorder = Recorder { calls: Vec::new() };
    let map: IndexMap<String, Value> = match components {
        Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => IndexMap::new(),
    };
    let function = |name: &JsString| {
        let mut o = Map::new();
        o.insert(
            "$component".into(),
            Value::String(name.to_string_lossy().into_owned()),
        );
        Some(Value::Object(o))
    };
    let components = match components {
        Some(Value::String(s)) if s == "$function" => Components::Function(&function),
        _ => Components::Map(&map),
    };
    let options = react::RenderOptions {
        components: Some(components),
        resolve_tag_name: None,
    };
    let result =
        react::render(tree, &mut recorder, &options).map_err(|e| index::thrown(&e).to_owned())?;
    let mut out = String::from("{\"calls\":[");
    out.push_str(&recorder.calls.join(","));
    out.push_str("],\"result\":");
    write_rendered(&mut out, &result);
    out.push('}');
    Ok(out)
}
