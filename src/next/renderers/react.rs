//! Port of upstream Markdoc v0.5.11's `src/renderers/react/react.ts`: `renderers.react(node,
//! React, {components, resolveTagName})`, a render tree as React elements.
//!
//! Owner: the api agent.
//!
//! React is the caller's: [`React`] is the two things upstream takes from it,
//! `createElement(type, props, ...children)` and `Fragment`, so any element type (a UI
//! framework's, or a recording of the calls) can be built. [`render`] calls them in the order
//! upstream does: for a tag, its `resolveTagName`, then the tags among its attributes'
//! values (each an element, made by `deepRender`), then its children, then the tag's own
//! `createElement`; for an array, its items, then a `Fragment` element.
//!
//! As upstream's, `render` takes a node as it is:
//!
//! - an array is a `Fragment` element of its items rendered;
//! - a value `Tag.isTag` does not accept (a string, a number, `null`, an object that is not
//!   a tag) is returned as it is ([`Rendered::Value`]);
//! - a tag (a `Tag`, or an object whose `$$mdtype` is `'Tag'`) is read as `const {name,
//!   attributes: {class: className, ...attrs} = {}, children = []} = node`: `class`, when
//!   truthy, becomes `className` (in place of an own `className`, else last), the props are
//!   `null` when nothing is left and otherwise `deepRender(attrs)` (arrays and plain objects
//!   copied, tags rendered to elements, anything else as it is), and the children are
//!   `children.map(render)`.
//!
//! The element type is `resolveTagName(name, components)`; the default ([`tag_name`]) passes
//! a name that is not a string, or whose first UTF-16 code unit `toUpperCase()` changes, as
//! it is ([`ElementType::Name`]), and looks any other up in `components`
//! ([`ElementType::Component`]): `components(name)` when it is a function, else
//! `components[name]` (`None` where that is `undefined`).
//!
//! What it builds borrows from the tree it renders (a [`Cow`]): a tag's name, its attributes'
//! keys and values, and the children that are not tags, as upstream hands its own objects on.
//!
//! `Err` is where upstream throws: `attributes` that is `null`, `children` that has no `map`
//! (`null`, a string), an empty name (its first code unit is `undefined`), and nesting past
//! node's stack (`RangeError: Maximum call stack size exceeded`): each `render` and
//! `deepRender` is charged to [`crate::depth`]'s budget, at costs solved from how deep a
//! fresh node process nests each shape ([`NODE_LIMITS`]), and grows the stack as it goes
//! (where the stack cannot grow, a 2 MiB debug stack bounds it instead, below node's limits).
//!
//! A name that `components` (an object) does not have as its own key, but `Object.prototype`
//! has (`__proto__`, `__defineGetter__`, ... : the names whose first character `toUpperCase()`
//! leaves alone), finds what every object inherits: [`ElementType::Inherited`].

use crate::markdown_it::js;
use crate::next::schema::value::{self, JsString, TagValue, Value};
use crate::next::stack;
use crate::types::{JsObject, MarkdocError, Result};
use indexmap::IndexMap;
use std::borrow::Cow;

/// How deep node 22 (V8's default stack, called from the top of a script) renders each shape
/// before `RangeError: Maximum call stack size exceeded`, in a fresh process and a warmed-up
/// one: `(shape, fresh, warm)`. The shapes (`findings/conformance/work/api/depth/shapes.cjs`):
/// tags in tags, arrays in arrays, and an attribute value of arrays, of objects and of tags
/// (each in the one before's attributes). The port stops at the fresh limit of each.
pub const NODE_LIMITS: [(&str, usize, usize); 5] = [
    ("tags", 2674, 4823),
    ("arrays", 2674, 4823),
    ("attr_arrays", 1843, 8643),
    ("attr_objects", 3799, 4820),
    ("attr_tags", 1383, 2411),
];

/// What a level of each site costs: node's stack (solved from [`NODE_LIMITS`] by
/// `work/api/depth/solve.py`) and the Rust stack it takes in a debug build (for where the
/// stack cannot grow).
#[derive(Clone, Copy)]
pub(crate) struct Cost(pub u64, pub u64);

const KB: u64 = 1024;
/// `render` of a tag or an array from the top, a tag's children or an array's items (a value
/// that is neither returns at once, uncharged).
const COST_RENDER: Cost = Cost(395050272, 4 * KB);
/// `render` of a tag reached through `deepRender` (that call included).
const COST_RENDER_ATTR: Cost = Cost(485851254, 6 * KB);
/// `deepRender` of an object (a tag's attributes too) and of an array.
const COST_DEEP_OBJECT: Cost = Cost(277902093, 5 * KB / 2);
const COST_DEEP_ARRAY: Cost = Cost(572763223, 5 * KB / 2);

/// Opens a level costing `cost`, or upstream's `RangeError`.
pub(crate) fn level(cost: Cost) -> Result<stack::Level> {
    stack::enter(cost.0, cost.1).ok_or_else(|| throw(value::RANGE_ERROR))
}

/// What upstream takes from React: `createElement` and `Fragment` (the type
/// [`ElementType::Fragment`] stands for).
pub trait React {
    /// What `createElement` returns.
    type Element;
    /// What `components` holds.
    type Component: Clone;

    /// `React.createElement(type, props, ...children)`. `props` is `None` (`null`) for a tag
    /// with no attributes left, else `deepRender` of them: a [`Prop::Object`], or, when the
    /// attributes are a tag themselves (`$$mdtype: 'Tag'`), its [`Prop::Element`].
    fn create_element(
        &mut self,
        element_type: ElementType<'_, Self::Component>,
        props: Option<Prop<'_, Self::Element>>,
        children: Vec<Rendered<'_, Self::Element>>,
    ) -> Self::Element;
}

/// A tag's `name`, as `resolveTagName` gets it.
#[derive(Clone, Debug)]
pub enum Name<'a> {
    /// A `Tag`'s name.
    String(&'a JsString),
    /// The `name` of an object read as a tag (`undefined` when it has none).
    Value(Cow<'a, Value>),
}

impl Name<'_> {
    /// The name as a value.
    pub fn to_value(&self) -> Value {
        match self {
            Name::String(s) => Value::String((*s).clone()),
            Name::Value(v) => v.as_ref().clone(),
        }
    }

    /// The name when it is a string.
    pub fn as_js_str(&self) -> Option<&JsString> {
        match self {
            Name::String(s) => Some(s),
            Name::Value(v) => v.as_js_str(),
        }
    }
}

/// The `type` an element is created with.
#[derive(Clone, Debug)]
pub enum ElementType<'a, C> {
    /// `React.Fragment`: an array's items.
    Fragment,
    /// The tag's name as it is (see the module's documentation).
    Name(Name<'a>),
    /// What `components` gives for the name: `None` where upstream's is `undefined`.
    Component(Option<C>),
    /// What `components[name]` inherits from `Object.prototype` when `components` is an
    /// object without that key: `Object.prototype` itself for `__proto__`, else its method of
    /// that name (`__defineGetter__`, `__lookupSetter__`, ...).
    Inherited(String),
}

/// The props `deepRender` makes of a tag's attributes: an object's own properties, in
/// order.
pub type Props<'a, E> = Vec<(Cow<'a, str>, Prop<'a, E>)>;

/// A value `deepRender` made.
#[derive(Clone, Debug)]
pub enum Prop<'a, E> {
    /// A value that is not an object (`undefined`, `null`, a boolean, a number, a string),
    /// as it is.
    Value(Cow<'a, Value>),
    /// An array, its items deep-rendered.
    Array(Vec<Prop<'a, E>>),
    /// A plain object, its members deep-rendered (a new object: an `__proto__` member sets
    /// its prototype, which is no member, so it is left out).
    Object(Props<'a, E>),
    /// A tag, rendered.
    Element(E),
}

impl<E> Prop<'_, E> {
    fn into_owned(self) -> Prop<'static, E> {
        match self {
            Prop::Value(v) => Prop::Value(Cow::Owned(v.into_owned())),
            Prop::Array(items) => Prop::Array(items.into_iter().map(Prop::into_owned).collect()),
            Prop::Object(props) => Prop::Object(
                props
                    .into_iter()
                    .map(|(k, v)| (Cow::Owned(k.into_owned()), v.into_owned()))
                    .collect(),
            ),
            Prop::Element(e) => Prop::Element(e),
        }
    }
}

/// What `render` returns for a node.
#[derive(Clone, Debug)]
pub enum Rendered<'a, E> {
    /// An element: a tag's, or a `Fragment` of an array's items.
    Element(E),
    /// A node that is neither a tag nor an array, as it is.
    Value(Cow<'a, Value>),
}

/// `components`: an object of components, or a function from a name to one.
pub enum Components<'a, C> {
    /// `components[name]`.
    Map(&'a IndexMap<String, C>),
    /// `components(name)` (`components instanceof Function`).
    Function(&'a dyn Fn(&JsString) -> Option<C>),
}

impl<C> Clone for Components<'_, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C> Copy for Components<'_, C> {}

/// A `resolveTagName` of the caller's: `(name, components) => type`.
pub type ResolveTagName<'a, C> =
    &'a dyn for<'n> Fn(&Name<'n>, Components<'_, C>) -> Result<ElementType<'n, C>>;

/// `renderers.react`'s options.
pub struct RenderOptions<'a, C> {
    /// `components` (default `{}`).
    pub components: Option<Components<'a, C>>,
    /// `resolveTagName` (default [`tag_name`]).
    pub resolve_tag_name: Option<ResolveTagName<'a, C>>,
}

impl<C> Default for RenderOptions<'_, C> {
    fn default() -> Self {
        RenderOptions {
            components: None,
            resolve_tag_name: None,
        }
    }
}

fn throw(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

/// The default `resolveTagName`, `tagName(name, components)`.
pub fn tag_name<'n, C: Clone>(
    name: &Name<'n>,
    components: Components<'_, C>,
) -> Result<ElementType<'n, C>> {
    let Some(string) = name.as_js_str() else {
        // This can be an object, e.g. when React.forwardRef is used
        return Ok(ElementType::Name(name.clone()));
    };
    // `name[0] !== name[0].toUpperCase()`
    let Some(first) = string.code_units().next() else {
        return Err(throw(
            "TypeError: Cannot read properties of undefined (reading 'toUpperCase')",
        ));
    };
    let changes = match u8::try_from(first) {
        Ok(ascii) if ascii.is_ascii() => ascii.is_ascii_lowercase(),
        _ => js::to_upper_case(&[first]) != [first],
    };
    if changes {
        return Ok(ElementType::Name(name.clone()));
    }
    Ok(match components {
        Components::Function(f) => ElementType::Component(f(string)),
        Components::Map(map) => match string.as_str() {
            Some(n) if map.contains_key(n) => ElementType::Component(map.get(n).cloned()),
            Some(n) if value::is_object_prototype_name(n) => ElementType::Inherited(n.into()),
            _ => ElementType::Component(None),
        },
    })
}

/// `dynamic(node, React, {components, resolveTagName})`: the node rendered (see the module's
/// documentation).
pub fn render<'a, R: React>(
    node: &'a Value,
    react: &mut R,
    options: &RenderOptions<'_, R::Component>,
) -> Result<Rendered<'a, R::Element>> {
    let empty = IndexMap::new();
    let mut renderer = Renderer {
        react,
        components: options.components.unwrap_or(Components::Map(&empty)),
        resolve_tag_name: options.resolve_tag_name,
    };
    renderer.render(node, COST_RENDER)
}

struct Renderer<'r, 'o, R: React> {
    react: &'r mut R,
    components: Components<'o, R::Component>,
    resolve_tag_name: Option<ResolveTagName<'o, R::Component>>,
}

impl<R: React> Renderer<'_, '_, R> {
    /// `deepRender(value)`.
    fn deep_render<'a>(&mut self, value: &'a Value) -> Result<Prop<'a, R::Element>> {
        let cost = match value {
            Value::Array(_) => COST_DEEP_ARRAY,
            Value::Object(_) if !TagValue::is_tag(value) => COST_DEEP_OBJECT,
            // a tag is charged as `render`'s
            Value::Object(_) | Value::Tag(_) => {
                return Ok(match self.render(value, COST_RENDER_ATTR)? {
                    Rendered::Element(e) => Prop::Element(e),
                    Rendered::Value(v) => Prop::Value(v),
                });
            }
            // `value == null || typeof value !== 'object'`
            _ => return Ok(Prop::Value(Cow::Borrowed(value))),
        };
        let _level = level(cost)?;
        stack::grow(|| match value {
            // (`map` skips an array's holes.)
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.present_len());
                for (_, item) in items.present() {
                    out.push(self.deep_render(item)?);
                }
                Ok(Prop::Array(out))
            }
            Value::Object(o) => self.deep_render_entries(o.iter().map(|(k, v)| (k.as_str(), v))),
            _ => unreachable!("an array or an object"),
        })
    }

    /// `const output = {}; for (const [k, v] of Object.entries(value)) output[k] =
    /// deepRender(v); return output;` over the entries given.
    fn deep_render_entries<'a>(
        &mut self,
        entries: impl Iterator<Item = (&'a str, &'a Value)>,
    ) -> Result<Prop<'a, R::Element>> {
        let mut output: Props<'a, R::Element> = Vec::new();
        for (k, v) in entries {
            let rendered = self.deep_render(v)?;
            // `output[k] = ...`: `__proto__` sets the new object's prototype (or, for a
            // primitive, nothing); either way it is no own member.
            if k != "__proto__" {
                set(&mut output, Cow::Borrowed(k), rendered);
            }
        }
        Ok(Prop::Object(output))
    }

    /// `render(node)`, at `cost` (see [`COST_RENDER_ATTR`]).
    fn render<'a>(&mut self, node: &'a Value, cost: Cost) -> Result<Rendered<'a, R::Element>> {
        if !matches!(node, Value::Array(_)) && !TagValue::is_tag(node) {
            // `node === null || typeof node !== 'object' || !Tag.isTag(node)`
            return Ok(Rendered::Value(Cow::Borrowed(node)));
        }
        let _level = level(cost)?;
        stack::grow(|| self.render_inner(node))
    }

    fn render_inner<'a>(&mut self, node: &'a Value) -> Result<Rendered<'a, R::Element>> {
        if let Value::Array(items) = node {
            // (`map` skips an array's holes.)
            let mut children = Vec::with_capacity(items.present_len());
            for (_, item) in items.present() {
                children.push(self.render(item, COST_RENDER)?);
            }
            let element = self
                .react
                .create_element(ElementType::Fragment, None, children);
            return Ok(Rendered::Element(element));
        }

        if !TagValue::is_tag(node) {
            return Ok(Rendered::Value(Cow::Borrowed(node)));
        }

        let Destructured {
            name,
            mut attrs,
            class_name,
            children,
        } = destructure(node)?;

        if class_name.to_boolean() {
            set(&mut attrs, Cow::Borrowed("className"), class_name);
        }

        let element_type = match self.resolve_tag_name {
            Some(resolve) => resolve(&name, self.components)?,
            None => tag_name(&name, self.components)?,
        };
        let props = if attrs.is_empty() {
            None
        } else {
            Some(self.deep_render_attrs(attrs)?)
        };
        let items = children.items()?;
        let mut rendered = Vec::with_capacity(items.len());
        for child in items {
            rendered.push(self.render(child, COST_RENDER)?);
        }
        let element = self.react.create_element(element_type, props, rendered);
        Ok(Rendered::Element(element))
    }

    /// `deepRender(attrs)` of a tag's attributes (`attrs` a new object: its own properties).
    fn deep_render_attrs<'a>(&mut self, attrs: Attrs<'a>) -> Result<Prop<'a, R::Element>> {
        let is_tag = attrs
            .iter()
            .any(|(k, v)| k == "$$mdtype" && matches!(v.as_str(), Some("Tag")));
        if is_tag || attrs.iter().any(|(_, v)| matches!(v, Cow::Owned(_))) {
            // The attributes are a tag (`render(attrs)`), or hold values made here (the own
            // properties of a string, say): rendered from an object of their own.
            let object = Value::object(
                attrs
                    .into_iter()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect::<JsObject<Value>>(),
            );
            return Ok(self.deep_render(&object)?.into_owned());
        }
        let _level = level(COST_DEEP_OBJECT)?;
        stack::grow(|| {
            let mut output: Props<'a, R::Element> = Vec::with_capacity(attrs.len());
            for (k, v) in attrs {
                let Cow::Borrowed(v) = v else {
                    unreachable!("borrowed values only")
                };
                let rendered = self.deep_render(v)?;
                if k != "__proto__" {
                    set(&mut output, k, rendered);
                }
            }
            Ok(Prop::Object(output))
        })
    }
}

/// `object[key] = value` on a new plain object's own properties: an existing key keeps its
/// place.
fn set<'a, T>(object: &mut Vec<(Cow<'a, str>, T)>, key: Cow<'a, str>, value: T) {
    match object.iter_mut().find(|(k, _)| *k == key) {
        Some((_, v)) => *v = value,
        None => object.push((key, value)),
    }
}

/// A tag's `children` (`[]` when `undefined`).
pub(crate) enum Children<'a> {
    Items(&'a [Value]),
    /// Any other value: `children.map` throws.
    Other(&'a Value),
}

impl<'a> Children<'a> {
    /// `children.map`'s receiver: an array's items, or the `TypeError` for a value without
    /// `map`.
    pub(crate) fn items(&self) -> Result<&'a [Value]> {
        match self {
            Children::Items(items) => Ok(items),
            Children::Other(children @ (Value::Undefined | Value::Null)) => Err(throw(format!(
                "TypeError: Cannot read properties of {} (reading 'map')",
                if children.is_undefined() {
                    "undefined"
                } else {
                    "null"
                }
            ))),
            Children::Other(_) => Err(throw("TypeError: children.map is not a function")),
        }
    }
}

/// What `const {name, attributes: {class: className, ...attrs} = {}, children = []} = node`
/// gives, shared by the dynamic and the static renderer, borrowed from the node.
pub(crate) struct Destructured<'a> {
    pub name: Name<'a>,
    /// `attrs`: the attributes' own properties but `class`, in order.
    pub attrs: Attrs<'a>,
    pub class_name: Cow<'a, Value>,
    pub children: Children<'a>,
}

/// `const {name, attributes: {class: className, ...attrs} = {}, children = []} = node` of a
/// tag (a `Tag`, or an object whose `$$mdtype` is `'Tag'`).
pub(crate) fn destructure(node: &Value) -> Result<Destructured<'_>> {
    const UNDEFINED: &Value = &Value::Undefined;
    let (name, attributes, children): (Name<'_>, Option<&Value>, Children<'_>) = match node {
        Value::Tag(tag) => {
            let (class_name, attrs) = attributes_of(&tag.attributes);
            return Ok(Destructured {
                name: match &tag.name {
                    Value::String(name) => Name::String(name),
                    other => Name::Value(Cow::Borrowed(other)),
                },
                attrs,
                class_name,
                children: Children::Items(&tag.children),
            });
        }
        Value::Object(o) => (
            Name::Value(Cow::Borrowed(o.get("name").unwrap_or(UNDEFINED))),
            o.get("attributes"),
            match o.get("children") {
                None | Some(Value::Undefined) => Children::Items(&[]),
                // A dense array's items (an array with holes is read as any other value).
                Some(array @ Value::Array(items)) => match items.as_dense() {
                    Some(items) => Children::Items(items),
                    None => Children::Other(array),
                },
                Some(other) => Children::Other(other),
            },
        ),
        _ => unreachable!("a tag is a Tag or an object"),
    };
    let (class_name, attrs) = match attributes {
        None | Some(Value::Undefined) => (Cow::Borrowed(UNDEFINED), Vec::new()),
        Some(Value::Null) => {
            return Err(throw(
                "TypeError: Cannot destructure property 'class' of '{}' as it is null.",
            ));
        }
        Some(Value::Object(o)) => attributes_of(o),
        Some(other) => {
            let class_name = value::get_property(other, "class").map_err(throw)?;
            let attrs = value::own_entries(other)
                .into_iter()
                .filter(|(k, _)| k != "class")
                .map(|(k, v)| (Cow::Owned(k), Cow::Owned(v)))
                .collect();
            (Cow::Owned(class_name), attrs)
        }
    };
    Ok(Destructured {
        name,
        attrs,
        class_name,
        children,
    })
}

/// A tag's attributes but `class`, borrowed where they can be: `(key, value)` in order.
pub(crate) type Attrs<'a> = Vec<(Cow<'a, str>, Cow<'a, Value>)>;

/// `{class: className, ...attrs}` of an object's own properties.
fn attributes_of(attributes: &value::Object) -> (Cow<'_, Value>, Attrs<'_>) {
    const UNDEFINED: &Value = &Value::Undefined;
    // `{class: className, ...attrs}`: `class` is read through the attributes' prototype chain
    // (a render name `__proto__` gives them one), the rest are their own properties.
    let class_name = match attributes.get("class") {
        Some(class) => Cow::Borrowed(class),
        None if attributes.proto().is_object_prototype() => Cow::Borrowed(UNDEFINED),
        None => Cow::Owned(value::object_get(attributes, "class").unwrap_or(Value::Undefined)),
    };
    let attrs = attributes
        .iter()
        .filter(|(k, _)| *k != "class")
        .map(|(k, v)| (Cow::Borrowed(k.as_str()), Cow::Borrowed(v)))
        .collect();
    (class_name, attrs)
}

#[cfg(test)]
mod tests {
    //! What node does for the same tree (`findings/conformance/work/api/unit_values.cjs`, with
    //! a React that records its calls). Upstream's own `react.test.ts` runs through the
    //! harness (`tests/next/suites/upstream-renderers-react-react.jsonl`) and
    //! `tests/next_suites.rs`.
    use super::*;
    use crate::next::index::{self, thrown};
    use crate::next::parser::ParserArgs;
    use crate::next::schema::Config;

    /// A React that records `createElement`'s calls as text; an element is its call's index.
    #[derive(Default)]
    struct Recorder(Vec<String>);

    fn text(value: &Value) -> String {
        value::json_stringify(value, "")
            .unwrap()
            .unwrap_or("undefined".into())
    }

    fn prop(p: &Prop<'_, usize>) -> String {
        match p {
            Prop::Value(v) => text(v),
            Prop::Element(e) => format!("el{e}"),
            Prop::Array(items) => {
                format!("[{}]", items.iter().map(prop).collect::<Vec<_>>().join(","))
            }
            Prop::Object(o) => format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{k}:{}", prop(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    impl React for Recorder {
        type Element = usize;
        type Component = String;

        fn create_element(
            &mut self,
            element_type: ElementType<'_, String>,
            props: Option<Prop<'_, usize>>,
            children: Vec<Rendered<'_, usize>>,
        ) -> usize {
            let t = match element_type {
                ElementType::Fragment => "Fragment".to_string(),
                ElementType::Name(n) => text(&n.to_value()),
                ElementType::Component(c) => c.unwrap_or("undefined".into()),
                ElementType::Inherited(name) => format!("fn {name}"),
            };
            let children: Vec<String> = children
                .iter()
                .map(|c| match c {
                    Rendered::Element(e) => format!("el{e}"),
                    Rendered::Value(v) => text(v),
                })
                .collect();
            self.0.push(format!(
                "{t} {} [{}]",
                props.as_ref().map_or("null".into(), prop),
                children.join(",")
            ));
            self.0.len() - 1
        }
    }

    fn tree(json: serde_json::Value) -> Value {
        Value::from_json(&json)
    }

    #[test]
    fn a_document_as_upstream_renders_it() {
        // node: createElement('h1', {className: 'a'}, 'Hi '), ('em', null, 'em'),
        // ('p', null, 'Some ', el1, ' text'), ('article', null, el0, el2).
        let ast =
            index::parse("# Hi {% .a %}\n\nSome *em* text\n", &ParserArgs::default()).unwrap();
        let values = index::transform(&ast, &Config::default()).unwrap();
        let mut react = Recorder::default();
        let out = render(&values, &mut react, &RenderOptions::default()).unwrap();
        assert!(matches!(out, Rendered::Element(3)));
        assert_eq!(
            react.0,
            [
                r#""h1" {className:"a"} ["Hi "]"#,
                r#""em" null ["em"]"#,
                r#""p" null ["Some ",el1," text"]"#,
                r#""article" null [el0,el2]"#,
            ]
        );
    }

    #[test]
    fn components_class_name_and_tags_in_attributes() {
        // node: ('b', null), then ('MyCallout', {className: 'c', x: [1, el0]}, 't'): `class`
        // takes `className`'s place, a tag in an attribute is rendered first.
        let t = tree(serde_json::json!({"$$mdtype": "Tag", "name": "Callout",
            "attributes": {"class": "c", "className": "k",
                "x": [1, {"$$mdtype": "Tag", "name": "b", "attributes": {}, "children": []}]},
            "children": ["t"]}));
        let map: IndexMap<String, String> =
            [("Callout".to_string(), "MyCallout".to_string())].into();
        let options = RenderOptions {
            components: Some(Components::Map(&map)),
            resolve_tag_name: None,
        };
        let mut react = Recorder::default();
        render(&t, &mut react, &options).unwrap();
        assert_eq!(
            react.0,
            [
                r#""b" null []"#,
                r#"MyCallout {className:"c",x:[1,el0]} ["t"]"#
            ]
        );
        // components as a function; a name only Object.prototype has
        let f = |n: &JsString| Some(format!("C:{}", n.to_string_lossy()));
        let options = RenderOptions {
            components: Some(Components::Function(&f)),
            resolve_tag_name: None,
        };
        let foo = tree(
            serde_json::json!({"$$mdtype": "Tag", "name": "Foo", "attributes": {}, "children": []}),
        );
        let mut react = Recorder::default();
        render(&foo, &mut react, &options).unwrap();
        assert_eq!(react.0, ["C:Foo null []"]);
        let getter = tree(
            serde_json::json!({"$$mdtype": "Tag", "name": "__defineGetter__",
            "attributes": {}, "children": []}),
        );
        let mut react = Recorder::default();
        render(&getter, &mut react, &RenderOptions::default()).unwrap();
        assert_eq!(react.0, ["fn __defineGetter__ null []"]);
    }

    #[test]
    fn what_upstream_throws() {
        let cases = [
            (
                serde_json::json!({"$$mdtype": "Tag", "name": "a", "attributes": null}),
                "TypeError: Cannot destructure property 'class' of '{}' as it is null.",
            ),
            (
                serde_json::json!({"$$mdtype": "Tag", "name": "a", "children": "xy"}),
                "TypeError: children.map is not a function",
            ),
            (
                serde_json::json!({"$$mdtype": "Tag", "name": ""}),
                "TypeError: Cannot read properties of undefined (reading 'toUpperCase')",
            ),
        ];
        for (json, message) in cases {
            let error = render(
                &tree(json),
                &mut Recorder::default(),
                &RenderOptions::default(),
            )
            .unwrap_err();
            assert_eq!(thrown(&error), message);
        }
    }

    #[test]
    fn nesting_stops_where_a_fresh_node_does() {
        // node 22, fresh: 2,674 tags in tags render, one more throws RangeError.
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let tags = |n: usize| {
                    let mut x = Value::from("x");
                    for _ in 0..n {
                        x = Value::from(TagValue::new("div", JsObject::new(), vec![x]));
                    }
                    x
                };
                let n = NODE_LIMITS[0].1;
                let o = RenderOptions::default();
                if stack::can_grow() {
                    assert!(render(&tags(n), &mut Recorder::default(), &o).is_ok());
                } else {
                    assert!(render(&tags(50), &mut Recorder::default(), &o).is_ok());
                }
                let error = render(&tags(n + 1), &mut Recorder::default(), &o).unwrap_err();
                assert_eq!(
                    thrown(&error),
                    "RangeError: Maximum call stack size exceeded"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
