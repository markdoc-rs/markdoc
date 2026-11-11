//! Upstream's `src/ast/node.test.ts` (the `transform` cases) and `src/ast/variable.test.ts`,
//! ported. The async cases are not: nothing here is asynchronous.

use super::*;
use crate::ast::PathSegment;
use crate::next::index;
use crate::next::schema::value::{JsString, TagValue};
use crate::next::schema::{
    BuiltinType, ConfigFunction, CustomType, SchemaAttribute, TransformFn, Variables,
};
use crate::types::{NodeType, RenderableTreeNode, Tag};
use serde_json::json;

fn node(t: &str, attributes: Vec<(&str, Scalar)>, children: Vec<Node>) -> Node {
    Node::new(
        NodeType::from_name(t),
        attributes
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Vec<_>>(),
        children,
        None,
    )
}

fn text(content: &str) -> Node {
    node(
        "text",
        vec![("content", Scalar::String(content.into()))],
        vec![],
    )
}

fn s(v: &str) -> Scalar {
    Scalar::String(v.into())
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

fn rs(v: &str) -> RenderableTreeNode {
    RenderableTreeNode::Scalar(s(v))
}

fn transform(content: &Node, config: &Config) -> RenderableTreeNode {
    index::transform(content, config)
        .expect("transform")
        .to_render()
        .expect("to_render")
}

fn schemas(list: Vec<(&str, Schema)>) -> IndexMap<String, Option<Schema>> {
    list.into_iter()
        .map(|(k, v)| (k.to_string(), Some(v)))
        .collect()
}

fn attribute(t: Option<BuiltinType>, render: Option<Value>) -> SchemaAttribute {
    SchemaAttribute {
        attr_type: t.map(TypeSpec::Builtin),
        render,
        ..Default::default()
    }
}

fn attributes(list: Vec<(&str, SchemaAttribute)>) -> Option<IndexMap<String, SchemaAttribute>> {
    Some(list.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

// built-in nodes

#[test]
fn a_heading_node() {
    let example = node(
        "heading",
        vec![("level", Scalar::Number(1.0))],
        vec![node("inline", vec![], vec![text("This is a heading")])],
    );
    assert_eq!(
        transform(&example, &Config::default()),
        tag("h1", vec![], vec![rs("This is a heading")])
    );
}

#[test]
fn fenced_code_block_with_language() {
    let example = node(
        "fence",
        vec![("language", s("ruby")), ("content", s("test"))],
        vec![],
    );
    assert_eq!(
        transform(&example, &Config::default()),
        tag("pre", vec![("data-language", s("ruby"))], vec![rs("test")])
    );
}

// transform functions

#[test]
fn with_a_simple_render_function() {
    let example = node("foo", vec![], vec![]);
    let transform_fn: TransformFn =
        Arc::new(|_node, _config| Ok(TagValue::new("foo", IndexMap::new(), vec![]).into()));
    let foo = Schema {
        transform: Some(transform_fn),
        ..Default::default()
    };
    let config = Config {
        nodes: schemas(vec![("foo", foo)]),
        ..Default::default()
    };
    assert_eq!(transform(&example, &config), tag("foo", vec![], vec![]));
}

#[test]
fn with_a_render_function_that_renders_attributes() {
    let example = node(
        "foo",
        vec![("bar", s("baz")), ("test", Scalar::Number(100.0))],
        vec![],
    );
    let transform_fn: TransformFn = Arc::new(|node, config| {
        let mut out = IndexMap::new();
        out.insert("name".to_string(), Value::from("foo"));
        out.insert(
            "attributes".to_string(),
            Value::object(node.transform_attributes(config)?),
        );
        Ok(Value::object(out))
    });
    let foo = Schema {
        attributes: attributes(vec![
            (
                "bar",
                attribute(Some(BuiltinType::String), Some(Value::Boolean(true))),
            ),
            (
                "test",
                attribute(Some(BuiltinType::Number), Some(Value::from("example"))),
            ),
        ]),
        transform: Some(transform_fn),
        ..Default::default()
    };
    let config = Config {
        nodes: schemas(vec![("foo", foo)]),
        ..Default::default()
    };
    assert_eq!(
        transform(&example, &config).to_json(),
        json!({"name": "foo", "attributes": {"bar": "baz", "example": 100}})
    );
}

// tags

fn foo_tag_config() -> Config {
    Config::from_json(&json!({"tags": {"foo": {
        "render": "foo",
        "attributes": {"bar": {"type": "String", "render": true}}
    }}}))
    .unwrap()
}

#[test]
fn with_a_tag() {
    let mut example = node(
        "tag",
        vec![("bar", s("baz"))],
        vec![node("inline", vec![], vec![text("test")])],
    );
    example.tag = Some("foo".into());
    assert_eq!(
        transform(&example, &foo_tag_config()),
        tag("foo", vec![("bar", s("baz"))], vec![rs("test")])
    );
}

#[test]
fn with_a_non_existing_tag() {
    let example = node(
        "container",
        vec![("bar", s("baz"))],
        vec![node("inline", vec![], vec![text("test")])],
    );
    assert_eq!(
        transform(&example, &foo_tag_config()),
        RenderableTreeNode::Scalar(Scalar::Array(vec![s("test")]))
    );
}

// tag name

#[test]
fn tag_name_with_a_string_value() {
    let example = node("foo", vec![], vec![]);
    let config = Config::from_json(&json!({"nodes": {"foo": {"render": "foo"}}})).unwrap();
    let RenderableTreeNode::Tag(output) = transform(&example, &config) else {
        panic!("a tag")
    };
    assert_eq!(output.name, "foo");
}

#[test]
fn tag_name_with_no_value() {
    let example = node("foo", vec![], vec![node("bar", vec![], vec![])]);
    let config =
        Config::from_json(&json!({"nodes": {"foo": {}, "bar": {"render": "bar"}}})).unwrap();
    assert_eq!(
        transform(&example, &config),
        RenderableTreeNode::Scalar(Scalar::Array(vec![Scalar::Tag(Box::new(Tag {
            name: "bar".into(),
            attributes: IndexMap::new(),
            children: vec![],
        }))]))
    );
}

// attributes

#[test]
fn with_an_id() {
    let example = node("paragraph", vec![("id", s("bar"))], vec![]);
    let RenderableTreeNode::Tag(output) = transform(&example, &Config::default()) else {
        panic!("a tag")
    };
    assert_eq!(output.name, "p");
    assert_eq!(output.attributes.get("id"), Some(&s("bar")));
}

#[test]
fn with_a_class() {
    let class = Scalar::Object(IndexMap::from([
        ("foo".to_string(), Scalar::Boolean(true)),
        ("bar".to_string(), Scalar::Boolean(false)),
    ]));
    let example = node("paragraph", vec![("class", class)], vec![]);
    assert_eq!(
        transform(&example, &Config::default()),
        tag("p", vec![("class", s("foo"))], vec![])
    );
}

fn foo_node_config(attrs: serde_json::Value) -> Config {
    Config::from_json(&json!({"nodes": {"foo": {"render": "foo", "attributes": attrs}}})).unwrap()
}

#[test]
fn with_boolean_render_attribute() {
    let example = node(
        "foo",
        vec![("bar", Scalar::Number(1.0)), ("baz", s("test"))],
        vec![],
    );
    let config = foo_node_config(json!({
        "bar": {"type": {"$type": "Number"}, "render": true},
        "baz": {"type": {"$type": "String"}, "render": true}
    }));
    assert_eq!(
        transform(&example, &config),
        tag(
            "foo",
            vec![("bar", Scalar::Number(1.0)), ("baz", s("test"))],
            vec![]
        )
    );
}

#[test]
fn with_string_render_attribute() {
    let example = node(
        "foo",
        vec![("bar", Scalar::Number(1.0)), ("baz", s("test"))],
        vec![],
    );
    let config = foo_node_config(json!({
        "bar": {"type": {"$type": "Number"}, "render": "data-bar"},
        "baz": {"type": {"$type": "String"}, "render": "data-baz"}
    }));
    assert_eq!(
        transform(&example, &config),
        tag(
            "foo",
            vec![("data-bar", Scalar::Number(1.0)), ("data-baz", s("test"))],
            vec![]
        )
    );
}

#[test]
fn with_a_non_rendered_attribute() {
    let example = node(
        "foo",
        vec![("bar", Scalar::Number(1.0)), ("baz", s("test"))],
        vec![],
    );
    let config = foo_node_config(json!({"bar": {"type": {"$type": "Number"}, "render": true}}));
    let output = transform(&example, &config);
    let RenderableTreeNode::Tag(t) = &output else {
        panic!("a tag")
    };
    assert!(!t.attributes.contains_key("baz"));
    assert_eq!(
        output,
        tag("foo", vec![("bar", Scalar::Number(1.0))], vec![])
    );
}

#[test]
fn with_a_default_attribute_value() {
    let example = node("foo", vec![], vec![]);
    let config = foo_node_config(
        json!({"bar": {"type": {"$type": "Number"}, "render": true, "default": 10}}),
    );
    let RenderableTreeNode::Tag(output) = transform(&example, &config) else {
        panic!("a tag")
    };
    assert_eq!(output.attributes["bar"], Scalar::Number(10.0));
}

#[test]
fn with_an_overriden_default_attribute_value() {
    let example = node("foo", vec![("bar", Scalar::Number(1.0))], vec![]);
    let config = foo_node_config(
        json!({"bar": {"type": {"$type": "Number"}, "render": true, "default": 10}}),
    );
    let RenderableTreeNode::Tag(output) = transform(&example, &config) else {
        panic!("a tag")
    };
    assert_eq!(output.attributes["bar"], Scalar::Number(1.0));
}

#[test]
fn variable_nested_in_a_hash() {
    let bar = Scalar::Variable(Variable::new(vec!["a".into(), "b".into(), "c".into()]));
    let example = node("foo", vec![("bar", bar)], vec![]);
    let config = Config::from_json(&json!({
        "nodes": {"foo": {"render": "foo", "attributes": {"bar": {"type": {"$type": "String"}, "render": true}}}},
        "variables": {"a": {"b": {"c": "example"}}}
    }))
    .unwrap();
    assert_eq!(
        transform(&example, &config),
        tag("foo", vec![("bar", s("example"))], vec![])
    );
}

struct NonNullString;
impl CustomType for NonNullString {
    fn name(&self) -> &str {
        "NonNullString"
    }
    fn transform(&self, value: Value, _config: &Config) -> Option<Result<Value>> {
        // value || ''
        Some(Ok(if value.to_boolean() {
            value
        } else {
            Value::from("")
        }))
    }
}

struct UpperCaseString;
impl CustomType for UpperCaseString {
    fn name(&self) -> &str {
        "UpperCaseString"
    }
    fn transform(&self, value: Value, _config: &Config) -> Option<Result<Value>> {
        // (value || '').toUpperCase()
        let s = if value.to_boolean() {
            value
                .to_js_string()
                .map(JsString::into_string_lossy)
                .unwrap_or_default()
        } else {
            String::new()
        };
        Some(Ok(Value::string(s.to_uppercase())))
    }
}

fn custom_type_output(t: Arc<dyn CustomType>, value: Option<&str>) -> Scalar {
    let attrs: Vec<(&str, Scalar)> = value.map(|v| vec![("bar", s(v))]).unwrap_or_default();
    let example = node("tag", attrs, vec![]);
    let config = Config {
        nodes: schemas(vec![(
            "tag",
            Schema {
                render: Some("tag".into()),
                attributes: attributes(vec![(
                    "bar",
                    SchemaAttribute {
                        render: Some(Value::Boolean(true)),
                        attr_type: Some(TypeSpec::Custom(t)),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    let RenderableTreeNode::Tag(output) = transform(&example, &config) else {
        panic!("a tag")
    };
    output.attributes["bar"].clone()
}

#[test]
fn custom_types_should_support_default_values() {
    assert_eq!(custom_type_output(Arc::new(NonNullString), None), s(""));
}

#[test]
fn custom_types_should_support_transforms() {
    assert_eq!(
        custom_type_output(Arc::new(UpperCaseString), Some("test")),
        s("TEST")
    );
}

// ast/variable.test.ts

fn variable_config() -> Config {
    Config::from_json(
        &json!({"variables": {"foo": {"bar": "example", "baz": [1, {"qux": "value"}, 2]}}}),
    )
    .unwrap()
}

#[test]
fn variable_basic_resolution() {
    let example = Variable::new(vec!["foo".into(), "bar".into()]);
    let output = resolve_variable(&example, &variable_config()).unwrap();
    assert_eq!(output.as_str(), Some("example"));
}

#[test]
fn variable_resolution_with_a_number_in_the_path() {
    let example = Variable::new(vec![
        "foo".into(),
        "baz".into(),
        PathSegment::Number(1.0),
        "qux".into(),
    ]);
    let output = resolve_variable(&example, &variable_config()).unwrap();
    assert_eq!(output.as_str(), Some("value"));
}

#[test]
fn variable_resolution_with_a_function() {
    let variables: crate::next::schema::VariablesFn = Arc::new(|path| {
        assert_eq!(path, [PathSegment::from("foo"), PathSegment::from("bar")]);
        Value::from("example")
    });
    let config = Config {
        variables: Some(Variables::Fn(variables)),
        ..Default::default()
    };
    let example = Variable::new(vec!["foo".into(), "bar".into()]);
    assert_eq!(
        resolve_variable(&example, &config).unwrap().as_str(),
        Some("example")
    );
}

// Beyond upstream's tests: the JS semantics the port relies on.

#[test]
fn a_missing_variable_is_undefined_and_a_path_through_null_throws() {
    let config = Config::from_json(&json!({"variables": {"n": null, "s": "ab"}})).unwrap();
    let path =
        |steps: &[&str]| Variable::new(steps.iter().map(|s| PathSegment::from(*s)).collect());
    assert!(
        resolve_variable(&path(&["missing", "x"]), &config)
            .unwrap()
            .is_undefined()
    );
    assert!(resolve_variable(&path(&["n", "x"]), &config).is_err());
    assert!(matches!(
        resolve_variable(&path(&["s", "length"]), &config),
        Ok(Value::Number(n)) if n == 2.0
    ));
    // Without variables, the path starts at `{}`.
    assert!(
        resolve_variable(&path(&["a", "b"]), &Config::default())
            .unwrap()
            .is_undefined()
    );
}

#[test]
fn a_function_the_config_lacks_is_null_and_one_without_transform_undefined() {
    let mut config = Config::default();
    config
        .functions
        .insert("f".into(), Some(ConfigFunction::default()));
    config.functions.insert("off".into(), None);
    let call = |name: &str| Function::new(name.into(), IndexMap::new());
    assert!(matches!(
        resolve_function(&call("nope"), &config),
        Ok(Value::Null)
    ));
    assert!(matches!(
        resolve_function(&call("off"), &config),
        Ok(Value::Null)
    ));
    assert!(
        resolve_function(&call("f"), &config)
            .unwrap()
            .is_undefined()
    );
    // A name every object inherits is a function without `transform`.
    assert!(
        resolve_function(&call("toString"), &config)
            .unwrap()
            .is_undefined()
    );
}

#[test]
fn else_without_primary_is_true_and_an_undefined_primary_is_false() {
    // {% if $no %}a{% else $missing /%}b{% else /%}c{% /if %}
    let else_tag = |primary: Option<Scalar>| {
        let mut n = node(
            "tag",
            primary.map(|p| vec![("primary", p)]).unwrap_or_default(),
            vec![],
        );
        n.tag = Some("else".into());
        n
    };
    let var = |name: &str| Scalar::Variable(Variable::new(vec![name.into()]));
    let mut example = node(
        "tag",
        vec![("primary", var("no"))],
        vec![
            text("a"),
            else_tag(Some(var("missing"))),
            text("b"),
            else_tag(None),
            text("c"),
        ],
    );
    example.tag = Some("if".into());
    let config = Config::from_json(&json!({"variables": {"no": false}})).unwrap();
    assert_eq!(
        transform(&example, &config),
        RenderableTreeNode::Scalar(Scalar::Array(vec![s("c")]))
    );
}

// `__proto__`: an assignment (`output[name] = value`) against a key (an object literal's own
// `__proto__`). Expected outputs are node 22's (Markdoc 0.5.11) for the same AST and config.

fn foo_tag(attributes: Vec<(&str, Scalar)>) -> Node {
    let mut n = node("tag", attributes, vec![]);
    n.tag = Some("foo".into());
    n
}

fn foo_config(attributes: serde_json::Value, slots: Option<serde_json::Value>) -> Config {
    let mut foo = json!({"render": "div", "attributes": attributes});
    if let Some(slots) = slots {
        foo["slots"] = slots;
    }
    Config::from_json(&json!({"tags": {"foo": foo}})).unwrap()
}

fn div_attributes(output: RenderableTreeNode) -> serde_json::Value {
    let RenderableTreeNode::Tag(t) = output else {
        panic!("a tag")
    };
    assert_eq!(t.name, "div");
    RenderableTreeNode::Tag(t).to_json()["attributes"].clone()
}

#[test]
fn a_render_name_proto_sets_the_prototype_until_it_is_null() {
    let two = json!({"a": {"render": "__proto__"}, "b": {"render": "__proto__"}});
    let n = Scalar::Number;
    for (a, b, expected) in [
        // `output.__proto__ = 1` is ignored by the setter ...
        (n(1.0), Scalar::Null, json!({})),
        (Scalar::Boolean(true), Scalar::Boolean(false), json!({})),
        // ... until `null` took the prototype (and the setter) away.
        (Scalar::Null, n(1.0), json!({"__proto__": 1})),
        (Scalar::Null, Scalar::Null, json!({"__proto__": null})),
        (s("x"), s("y"), json!({})),
    ] {
        let example = foo_tag(vec![("a", a.clone()), ("b", b.clone())]);
        assert_eq!(
            div_attributes(transform(&example, &foo_config(two.clone(), None))),
            expected,
            "a={a:?} b={b:?}"
        );
    }
    // An object becomes the prototype (no key); then `null`; then a key, in its place.
    let example = foo_tag(vec![
        (
            "a",
            Scalar::Object(IndexMap::from([("x".to_string(), n(1.0))])),
        ),
        ("b", Scalar::Null),
        ("c", s("z")),
        ("d", Scalar::Array(vec![n(1.0)])),
    ]);
    let config = foo_config(
        json!({"a": {"render": "__proto__"}, "b": {"render": "__proto__"}, "c": {}, "d": {"render": "__proto__"}}),
        None,
    );
    let attributes = div_attributes(transform(&example, &config));
    assert_eq!(attributes, json!({"c": "z", "__proto__": [1]}));
    assert_eq!(
        attributes.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["c", "__proto__"]
    );
    // Defaults go in the same way.
    let config = foo_config(
        json!({"a": {"render": "__proto__", "default": null}, "b": {"render": "__proto__", "default": 5}}),
        None,
    );
    assert_eq!(
        div_attributes(transform(&foo_tag(vec![]), &config)),
        json!({"__proto__": 5})
    );
}

#[test]
fn a_slot_rendered_as_proto_is_no_key_unless_the_prototype_is_null() {
    let slot = |name: &str| {
        let mut s = node(
            "tag",
            vec![("primary", Scalar::String(name.into()))],
            vec![node(
                "paragraph",
                vec![],
                vec![node("inline", vec![], vec![text("hi")])],
            )],
        );
        s.tag = Some("slot".into());
        s
    };
    let slots = json!({"s": {"render": "__proto__"}});
    let mut example = foo_tag(vec![]);
    example.slots.insert("s".into(), slot("s"));
    let config = foo_config(json!({}), Some(slots.clone()));
    assert_eq!(div_attributes(transform(&example, &config)), json!({}));

    let mut example = foo_tag(vec![("a", Scalar::Null)]);
    example.slots.insert("s".into(), slot("s"));
    let config = foo_config(json!({"a": {"render": "__proto__"}}), Some(slots));
    assert_eq!(
        div_attributes(transform(&example, &config)),
        json!({"__proto__": [{"$$mdtype": "Tag", "name": "p", "attributes": {}, "children": ["hi"]}]})
    );
}

#[test]
fn an_object_literal_key_proto_is_the_setter_too() {
    let config = foo_config(json!({"a": {}}), None);
    let literal = |proto: Scalar| {
        Scalar::Object(IndexMap::from([
            ("__proto__".to_string(), proto),
            ("x".to_string(), Scalar::Number(2.0)),
        ]))
    };
    // A primitive is ignored; a plain object becomes a prototype that no output shows.
    for proto in [
        Scalar::Number(1.0),
        Scalar::Object(IndexMap::from([("k".to_string(), Scalar::Number(1.0))])),
    ] {
        let example = foo_tag(vec![("a", literal(proto))]);
        assert_eq!(
            div_attributes(transform(&example, &config)),
            json!({"a": {"x": 2}})
        );
    }
    // A prototype that `String()` reads through. node (0.5.11) for `{% foo a={__proto__: P,
    // x: 2} /%}`: the tree's `a` is `{"x":2}` for each; the HTML renders `a="1"` for `[1]`,
    // and throws `TypeError: Cannot convert object to primitive value` for `null` and for
    // `{toString: 1}`.
    let pseudo_tag = Scalar::Object(IndexMap::from([
        ("$$mdtype".to_string(), s("Tag")),
        ("name".to_string(), s("b")),
    ]));
    for (proto, html) in [
        (
            Scalar::Array(vec![Scalar::Number(1.0)]),
            Ok("<div a=\"1\"></div>"),
        ),
        (
            Scalar::Null,
            Err("TypeError: Cannot convert object to primitive value"),
        ),
        (
            Scalar::Object(IndexMap::from([(
                "toString".to_string(),
                Scalar::Number(1.0),
            )])),
            Err("TypeError: Cannot convert object to primitive value"),
        ),
        (pseudo_tag, Ok("<div a=\"[object Object]\"></div>")),
    ] {
        let example = foo_tag(vec![("a", literal(proto.clone()))]);
        let tree = index::transform(&example, &config).expect("transforms");
        assert_eq!(
            div_attributes(tree.to_render().expect("a tree")),
            json!({"a": {"x": 2}}),
            "{proto:?}"
        );
        let rendered = crate::next::renderers::html::render(&tree).map_err(|e| match e {
            crate::types::MarkdocError::TransformError(m) => m,
            other => other.to_string(),
        });
        assert_eq!(
            rendered.as_deref(),
            html.map_err(String::from).as_deref(),
            "{proto:?}"
        );
    }
}

#[test]
fn an_inherited_property_through_a_literal_prototype() {
    // {% partial file="p" variables={a: {__proto__: {k: 1}}} /%}, p: {% $a.k %}
    // node renders 1: `$a.k` reads `k` through the prototype `resolve` gave the copy of
    // `{__proto__: {k: 1}}` (`output.__proto__ = ...` is the setter).
    let inner = Scalar::Object(IndexMap::from([(
        "__proto__".to_string(),
        Scalar::Object(IndexMap::from([("k".to_string(), Scalar::Number(1.0))])),
    )]));
    let variables = Scalar::Object(IndexMap::from([("a".to_string(), inner)]));
    let mut partial = node(
        "tag",
        vec![("file", s("p")), ("variables", variables)],
        vec![],
    );
    partial.tag = Some("partial".into());
    let document = node("document", vec![], vec![partial]);
    let mut p = node(
        "text",
        vec![(
            "content",
            Scalar::Variable(Variable::new(vec!["a".into(), "k".into()])),
        )],
        vec![],
    );
    p = node("document", vec![], vec![p]);
    let config = Config {
        partials: Some(IndexMap::from([(
            "p".to_string(),
            crate::next::schema::Partial::Node(Arc::new(p)),
        )])),
        ..Default::default()
    };
    assert_eq!(
        transform(&document, &config).to_json(),
        json!({"$$mdtype": "Tag", "name": "article", "attributes": {}, "children": [1]})
    );
}

#[test]
fn a_plain_object_prints_as_object_object() {
    // {% foo a={x: 2} /%} renders `a="[object Object]"`. (With a later `__proto__: [1]`, the
    // grammar's `Object.assign` makes [1] the object's prototype, which the AST holds, and node
    // renders `a="1"`: pinned against node by tests/next_model.rs.)
    let example = foo_tag(vec![(
        "a",
        Scalar::Object(IndexMap::from([("x".to_string(), Scalar::Number(2.0))])),
    )]);
    let config = foo_config(json!({"a": {}}), None);
    let tree = index::transform(&example, &config).unwrap();
    assert_eq!(
        crate::next::renderers::html::render(&tree).unwrap(),
        "<div a=\"[object Object]\"></div>"
    );
}

// Depth: upstream recurses until V8's stack overflows and throws RangeError; the port stops
// with that error where node's fresh process does (the levels' `cost`s on the depth budget)
// where the stack grows, and at its fixed limits (MAX_TRANSFORM_DEPTH, MAX_VALUE_DEPTH) where
// it cannot; on a 2 MiB stack, in debug and release builds alike.

/// `grows` where the stack grows (node's limit, from the levels' costs), else `fixed`.
fn limit(grows: usize, fixed: usize) -> usize {
    if crate::depth::can_grow() {
        grows
    } else {
        fixed
    }
}

const RANGE_ERROR: &str = "Transform error: RangeError: Maximum call stack size exceeded";

/// `f` on a thread with a 2 MiB stack (the robustness policy's).
fn on_2_mib<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn_scoped(s, f)
            .expect("a thread")
            .join()
            .expect("no panic")
    })
}

/// A document over `levels` nested nodes made by `wrap` around a text node: the document,
/// the levels and the text node are `levels + 2` nested `transformer.node` calls.
fn nested(levels: usize, wrap: impl Fn(usize, Node) -> Node) -> Node {
    let mut n = text("a");
    for i in 0..levels {
        n = wrap(i, n);
    }
    node("document", vec![], vec![n])
}

fn x_tag(_: usize, child: Node) -> Node {
    let mut n = node("tag", vec![], vec![child]);
    n.tag = Some("x".into());
    n
}

fn transform_err(doc: &Node, config: &Config) -> String {
    index::transform(doc, config).unwrap_err().to_string()
}

#[test]
fn transform_throws_past_its_depth() {
    on_2_mib(|| {
        let config = Config::from_json(&json!({"tags": {"x": {"render": "div"}}})).unwrap();
        // node: 1,397 tags fresh, 1,457 after a deep value was resolved (warm 2,457).
        let levels = limit(1_457, MAX_TRANSFORM_DEPTH - 2);
        assert!(index::transform(&nested(levels, x_tag), &config).is_ok());
        assert_eq!(
            transform_err(&nested(levels + 1, x_tag), &config),
            RANGE_ERROR
        );
        // `if` tags (node: 1,113, warm 1,898) and lists and items in turn (1,208, warm
        // 2,103) cost more where the stack grows, and count the same where it cannot; a
        // failed transform leaves the depth as it found it.
        let if_tag = |_: usize, child: Node| {
            let mut n = node("tag", vec![("primary", Scalar::Boolean(true))], vec![child]);
            n.tag = Some("if".into());
            n
        };
        let list = |i: usize, child: Node| {
            if i % 2 == 1 {
                node(
                    "list",
                    vec![("ordered", Scalar::Boolean(false))],
                    vec![child],
                )
            } else {
                node("item", vec![], vec![child])
            }
        };
        // (Where the stack cannot grow, a debug build's list levels take enough stack that
        // the measured bound, STACK_BUDGET_BYTES, stops them some 25 levels before the count.)
        let fixed = MAX_TRANSFORM_DEPTH - 2;
        for (wrap, levels, fits) in [
            (
                &if_tag as &dyn Fn(usize, Node) -> Node,
                limit(1_114, fixed),
                limit(1_114, fixed),
            ),
            (&list, limit(1_230, fixed), limit(1_230, 1_000)),
        ] {
            let config = Config::default();
            assert!(index::transform(&nested(fits, wrap), &config).is_ok());
            assert_eq!(
                transform_err(&nested(levels + 1, wrap), &config),
                RANGE_ERROR
            );
        }
        // Where the stack cannot grow, what takes more stack a level than the count allows
        // for (a heading's transform, a slot's) is bounded by the stack it uses.
        let heading = |_: usize, child: Node| {
            node("heading", vec![("level", Scalar::Number(1.0))], vec![child])
        };
        assert_eq!(
            transform_err(&nested(MAX_TRANSFORM_DEPTH, heading), &Config::default()),
            RANGE_ERROR
        );
    });
}

/// `partials: {p0, p1, ...}`, each including the next, the last a text node; the document
/// includes `p0`.
fn partial_chain(count: usize) -> (Node, Config) {
    use crate::next::schema::Partial;
    let include = |i: usize| {
        let mut n = node("tag", vec![("file", s(&format!("p{i}")))], vec![]);
        n.tag = Some("partial".into());
        n
    };
    let mut partials = IndexMap::new();
    for i in 0..count {
        let body = if i + 1 < count {
            include(i + 1)
        } else {
            text("a")
        };
        let doc = node("document", vec![], vec![body]);
        partials.insert(format!("p{i}"), Partial::Node(Arc::new(doc)));
    }
    let config = Config {
        partials: Some(partials),
        ..Default::default()
    };
    (node("document", vec![], vec![include(0)]), config)
}

#[test]
fn a_partial_costs_what_it_costs_node() {
    // node: 903 partials each including the next (warm 1,840). Where the stack cannot grow,
    // a partial counts twice: the document, the text node and each partial's tag and
    // transform, 2 + 2 * 549 = 1,100.
    let partials = limit(904, 549);
    on_2_mib(move || {
        let (doc, config) = partial_chain(partials);
        assert!(index::transform(&doc, &config).is_ok());
        let (doc, config) = partial_chain(partials + 1);
        assert_eq!(transform_err(&doc, &config), RANGE_ERROR);
    });
}

#[test]
fn resolve_throws_past_its_depth() {
    // A loop with node's limit in either case: MAX_RESOLVE_DEPTH nodes on the path through
    // children, from the top of a thread.
    on_2_mib(|| {
        let config = Config::default();
        let doc = nested(MAX_RESOLVE_DEPTH - 2, x_tag);
        assert!(index::resolve(&doc, &config).is_ok());
        let doc = nested(MAX_RESOLVE_DEPTH - 1, x_tag);
        assert_eq!(
            index::resolve(&doc, &config).unwrap_err().to_string(),
            RANGE_ERROR
        );
    });
}

/// A document with one `x` tag whose attribute `a` is `value`.
fn with_attribute(value: Scalar) -> (Node, Config) {
    let mut tag = node("tag", vec![("a", value)], vec![]);
    tag.tag = Some("x".into());
    let config = Config::from_json(&json!({"tags": {"x": {"render": "div",
        "attributes": {"a": {}}}}}))
    .unwrap();
    (node("document", vec![], vec![tag]), config)
}

#[test]
fn a_deep_value_throws_past_its_depth() {
    on_2_mib(|| {
        // Arrays and objects: a level each; the innermost number is one more.
        let arrays = |d: usize| (0..d).fold(Scalar::Number(1.0), |v, _| Scalar::Array(vec![v]));
        let objects = |d: usize| {
            (0..d).fold(Scalar::Number(1.0), |v, _| {
                Scalar::Object(IndexMap::from([("k".to_string(), v)]))
            })
        };
        // node: 1,716 arrays (warm more than 6,928), 3,481 objects (warm 3,496).
        for (value, levels) in [
            (
                &arrays as &dyn Fn(usize) -> Scalar,
                limit(1_718, MAX_VALUE_DEPTH - 1),
            ),
            (&objects, limit(3_483, MAX_VALUE_DEPTH - 1)),
        ] {
            let (doc, config) = with_attribute(value(levels));
            assert!(index::transform(&doc, &config).is_ok());
            let (doc, config) = with_attribute(value(levels + 1));
            assert_eq!(transform_err(&doc, &config), RANGE_ERROR);
        }
        // Calls (`not(not(...(true)))`): two levels each.
        let calls = |d: usize| {
            (0..d).fold(Scalar::Boolean(true), |v, _| {
                Scalar::Function(Function::new(
                    "not".into(),
                    IndexMap::from([("0".to_string(), v)]),
                ))
            })
        };
        // node: 1,381 calls (warm 1,548).
        let levels = limit(1_382, MAX_VALUE_DEPTH / 2 - 1);
        let (doc, config) = with_attribute(calls(levels));
        assert!(index::transform(&doc, &config).is_ok());
        let (doc, config) = with_attribute(calls(levels + 1));
        assert_eq!(transform_err(&doc, &config), RANGE_ERROR);
    });
}

#[test]
fn a_deep_value_deep_in_a_transform_throws_before_the_stack_runs_out() {
    // A partial 500 levels down whose text holds a deep array: 1,000 levels (1,500 where the
    // stack grows) is under the value's own limit, and 500 under the transform's, but their
    // sum is over the stack the two share; 100 levels is not.
    use crate::next::schema::Partial;
    fn document(array_depth: usize) -> (Node, Config) {
        let value = (0..array_depth).fold(Scalar::Number(1.0), |v, _| Scalar::Array(vec![v]));
        let mut text = node("text", vec![("content", value)], vec![]);
        text.inline = true;
        let part = node("document", vec![], vec![text]);
        let mut include = node("tag", vec![("file", s("p"))], vec![]);
        include.tag = Some("partial".into());
        let mut inner = include;
        for _ in 0..500 {
            inner = x_tag(0, inner);
        }
        let config = Config {
            partials: Some(IndexMap::from([(
                "p".to_string(),
                Partial::Node(Arc::new(part)),
            )])),
            tags: schemas(vec![("x", Schema::default())]),
            ..Default::default()
        };
        (node("document", vec![], vec![inner]), config)
    }
    on_2_mib(|| {
        let (doc, config) = document(100);
        assert!(index::transform(&doc, &config).is_ok());
        if crate::depth::can_grow() {
            let (doc, config) = document(1_500);
            assert_eq!(transform_err(&doc, &config), RANGE_ERROR);
        } else {
            // The stack each level takes decides (a debug build's throws at 1,300, a release
            // build's is under the bound): never an overflow, and the value's count past it.
            let (doc, config) = document(1_300);
            let outcome = index::transform(&doc, &config).map(drop);
            assert!(outcome.is_ok() || transform_err(&doc, &config) == RANGE_ERROR);
            let (doc, config) = document(MAX_VALUE_DEPTH);
            assert_eq!(transform_err(&doc, &config), RANGE_ERROR);
        }
    });
}

#[test]
fn a_slot_named_after_an_object_method_throws_upstreams_message() {
    // node 22: `slots: {toString: {}}` and a tag without that slot: `node.slots.toString` is
    // a built-in function, and transforming it throws reading its (undefined) `children`.
    let mut tag = node("tag", vec![], vec![]);
    tag.tag = Some("foo".into());
    let doc = node("document", vec![], vec![tag]);
    let config =
        Config::from_json(&json!({"tags": {"foo": {"render": "div", "slots": {"toString": {}}}}}))
            .unwrap();
    assert_eq!(
        transform_err(&doc, &config),
        "Transform error: TypeError: Cannot read properties of undefined (reading 'flatMap')"
    );
}

#[test]
fn a_schema_transform_written_in_rust_recurses_through_transform() {
    // A tag whose transform is a Rust closure with a 16 KiB frame of its own that transforms
    // its children (as a heading's does) and wraps them, nested far deeper than a 2 MiB stack
    // holds such frames: where the stack grows, as deep as node takes a schema transform's
    // levels (1,064 headings), each level on a stack with room for it; where it cannot, the
    // stack it measurably uses stops it. Never an overflow.
    let transform: TransformFn = Arc::new(|node, config| {
        let scratch = std::hint::black_box([7u8; 16 * 1024]);
        let children = node.transform_children(config)?;
        let mut attributes = IndexMap::new();
        attributes.insert(
            "n".to_string(),
            Value::Number(f64::from(scratch[children.len() % 1024])),
        );
        Ok(TagValue::new("r", attributes, children).into())
    });
    let config = Config {
        tags: schemas(vec![(
            "r",
            Schema {
                transform: Some(transform),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    let r = |_: usize, child: Node| {
        let mut n = node("tag", vec![], vec![child]);
        n.tag = Some("r".into());
        n
    };
    on_2_mib(move || {
        if crate::depth::can_grow() {
            let tree = index::transform(&nested(1_064, r), &config).expect("transforms");
            let html = crate::next::renderers::html::render(&tree).expect("renders");
            assert_eq!(html.matches("<r n=\"7\">").count(), 1_064);
            assert_eq!(transform_err(&nested(1_065, r), &config), RANGE_ERROR);
        } else {
            // A level takes the closure's 16 KiB and the transform's own frames: in a debug
            // build of the merged crate 50 levels exceed the measured budget, 30 fit it.
            assert!(index::transform(&nested(30, r), &config).is_ok());
            assert_eq!(transform_err(&nested(1_064, r), &config), RANGE_ERROR);
        }
        // The depth budget is whole again after each call, failed or not.
        assert_eq!(crate::depth::used(), 0);
    });
}
