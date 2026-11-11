//! Upstream's own test suites through `markdoc::next`, without node: what the harness cannot
//! run as a document (tests/next/suites/README.md says how each file was mined, and
//! findings/conformance/work/api/SUITES.md what became of every test).
//!
//! - `upstream-direct.jsonl`: the calls upstream's tests make to something other than a
//!   document's pipeline (`Node.walk`, `Variable.resolve`, `Function.resolve`, `Tag.isTag`,
//!   `findTagEnd`, the tag grammar, a renderer with a `resolveTagName` of the test's own), each
//!   with what node returned from upstream's published dist; the calls whose config holds
//!   functions are Rust tests of their modules (`src/next/validator/tests.rs`,
//!   `src/next/transformer/tests.rs`), whose configs are written in Rust.
//! - `linkify-fixtures.jsonl`: linkify-it 6.1.0's `test/fixtures/links.txt` and
//!   `not_links.txt`, run as its own `test/api.test.mjs` runs them, with node's answers.
//!
//! The files are written by `findings/conformance/work/api/suites/` (`run_test.cjs` runs
//! upstream's test files against the dist and records every call, `make_suites.py` sorts the
//! records into input sets and these files, `linkify_fixtures.cjs` runs the fixtures).

use markdoc::ast::{Function, PathSegment, Variable};
use markdoc::grammar;
use markdoc::markdown_it::js;
use markdoc::markdown_it::linkify_it::{LinkifyIt, LinkifyOptions};
use markdoc::next::compare::{first_difference, node_from_json, node_json};
use markdoc::next::functions::default_functions;
use markdoc::next::index;
use markdoc::next::renderers::{react, react_static};
use markdoc::next::schema::value::{TagValue, Value as JsValue, json_stringify, strict_equals};
use markdoc::next::schema::{Config, Variables};
use markdoc::next::tokenizer::utils::find_tag_end;
use markdoc::next::transformer::{resolve_function, resolve_variable};
use markdoc::types::Scalar;
use serde_json::{Map, Value};

fn suite(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/tests/next/suites/golden/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// The records of one kind (and method).
fn records(kind: &str, method: Option<&str>) -> Vec<Value> {
    suite("upstream-direct.jsonl")
        .into_iter()
        .filter(|r| r["kind"] == kind && method.is_none_or(|m| r["method"] == m))
        .collect()
}

fn label(r: &Value) -> String {
    format!(
        "{} > {}",
        r["file"].as_str().unwrap(),
        r["test"].as_str().unwrap()
    )
}

/// A node's JSON with the positions left out (`lines`, `location`) and its errors as
/// `{id, level, message}`, on both sides: what the positions mean is not this test's.
fn without_positions(value: &Value) -> Value {
    match value {
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, v) in o {
                match k.as_str() {
                    "lines" | "location" => {}
                    "errors" if o.get("$$mdtype") == Some(&"Node".into()) => {
                        let errors = v.as_array().into_iter().flatten().map(|e| {
                            let mut e2 = Map::new();
                            for key in ["id", "level", "message"] {
                                if let Some(x) = e.get(key) {
                                    e2.insert(key.into(), x.clone());
                                }
                            }
                            Value::Object(e2)
                        });
                        out.insert(k.clone(), Value::Array(errors.collect()));
                    }
                    _ => {
                        out.insert(k.clone(), without_positions(v));
                    }
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(without_positions).collect()),
        other => other.clone(),
    }
}

fn check(failures: &mut Vec<String>, what: String, expected: &Value, got: &Value) {
    if let Some((at, a, b)) = first_difference(expected, got) {
        failures.push(format!("{what}{at}: upstream {a} | port {b}"));
    }
}

fn assert_none(failures: Vec<String>, checked: usize, at_least: usize) {
    assert!(
        failures.is_empty(),
        "{} difference(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked >= at_least, "{checked} checked");
}

#[test]
fn node_walk_as_upstreams_tests_call_it() {
    // node.test.ts's traversal and parser.test.ts's `walk()` of parsed documents.
    let mut failures = Vec::new();
    let all = records("method", Some("Node.walk"));
    for r in &all {
        let node = node_from_json(&r["self"]).expect("upstream's node");
        let walked: Vec<Value> = node.walk().map(node_json).collect();
        check(
            &mut failures,
            label(r),
            &without_positions(&r["result"]),
            &without_positions(&Value::Array(walked)),
        );
    }
    assert_none(failures, all.len(), 122);
}

#[test]
fn variable_resolve() {
    // variable.test.ts: `new Variable(path).resolve({variables})`.
    let mut failures = Vec::new();
    let all = records("method", Some("Variable.resolve"));
    for r in &all {
        let path = r["self"]["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| match p {
                Value::String(s) => PathSegment::from(s.as_str()),
                Value::Number(n) => PathSegment::from(n.as_f64().unwrap()),
                other => panic!("a path segment: {other}"),
            })
            .collect();
        let variables = match r["args"][0].get("value") {
            Some(config) => Variables::Value(JsValue::from_json(&config["variables"])),
            // 'resolution with a function': `variables(path)` checks the path and returns
            // 'example'.
            None => Variables::Fn(std::sync::Arc::new(|path: &[PathSegment]| {
                assert_eq!(path, [PathSegment::from("foo"), PathSegment::from("bar")]);
                JsValue::from("example")
            })),
        };
        let config = Config {
            variables: Some(variables),
            ..Config::default()
        };
        let got = resolve_variable(&Variable::new(path), &config).expect("resolves");
        let json = json_stringify(&got, "").unwrap().unwrap_or("null".into());
        check(
            &mut failures,
            label(r),
            &r["result"],
            &serde_json::from_str(&json).unwrap(),
        );
    }
    assert_none(failures, all.len(), 3);
}

#[test]
fn function_resolve_with_the_built_in_functions() {
    // functions/index.test.ts: `new Function('equals', {0: new Variable(['foo']), 1: 'bar'})
    // .resolve({functions, variables: {foo: 'bar' | 'baz'}})` is true, then false.
    let all = records("method", Some("Function.resolve"));
    assert_eq!(all.len(), 2);
    for (r, foo) in all.iter().zip(["bar", "baz"]) {
        let parameters = [
            (
                "0".to_string(),
                Scalar::Variable(Variable::new(vec![PathSegment::from("foo")])),
            ),
            ("1".to_string(), Scalar::String("bar".into())),
        ];
        let function = Function::new("equals".into(), parameters.into_iter().collect());
        let config = Config {
            functions: default_functions().clone(),
            variables: Some(Variables::Value(JsValue::from_json(
                &serde_json::json!({ "foo": foo }),
            ))),
            ..Config::default()
        };
        let got = resolve_function(&function, &config).expect("resolves");
        assert!(
            strict_equals(&JsValue::Boolean(r["result"].as_bool().unwrap()), &got),
            "{}",
            label(r)
        );
    }
}

#[test]
fn tag_is_tag() {
    // tag.test.ts: `Tag.isTag(value)`.
    let all = records("Tag.isTag", None);
    for r in &all {
        let value = if r["undefined"] == true {
            JsValue::Undefined
        } else {
            JsValue::from_json(&r["value"])
        };
        assert_eq!(
            TagValue::is_tag(&value),
            r["result"].as_bool().unwrap(),
            "{}",
            label(r)
        );
    }
    assert!(all.len() >= 8);
}

#[test]
fn find_tag_end_as_utils_test_calls_it() {
    let all = records("findTagEnd", None);
    for r in &all {
        let content = js::to_utf16(r["content"].as_str().unwrap());
        let start = r["start"].as_u64().unwrap_or(0) as usize;
        let got = find_tag_end(&content, start).map(|n| n as u64);
        assert_eq!(got, r["result"].as_u64(), "{}", label(r));
    }
    assert!(all.len() >= 15);
}

#[test]
fn the_tag_grammar_as_tag_test_calls_it() {
    // grammar/tag.test.ts (also in tests/grammar/golden.jsonl): `parse(content)`, or the
    // SyntaxError it throws.
    let mut failures = Vec::new();
    let all = records("grammar", None);
    for r in &all {
        match grammar::parse(r["content"].as_str().unwrap()) {
            Ok(parsed) => {
                let mut out = String::new();
                parsed.write_json(&mut out);
                let got: Value = serde_json::from_str(&out).unwrap();
                match r.get("result") {
                    Some(expected) => check(&mut failures, label(r), expected, &got),
                    None => failures.push(format!("{}: upstream threw, port parsed", label(r))),
                }
            }
            Err(e) => {
                if r.get("threw").is_none() {
                    failures.push(format!("{}: port threw {e:?}", label(r)));
                }
            }
        }
    }
    assert_none(failures, all.len(), 44);
}

#[test]
fn react_static_with_a_resolve_tag_name_of_the_tests() {
    // react.test.ts: `reactStatic(tree, {resolveTagName: function tagName(name, components)
    // {return components.Foo;}})`, the function's source written into the output.
    let all = records("reactStatic", None);
    assert_eq!(all.len(), 1);
    for r in &all {
        let tree = JsValue::from_json(&r["tree"]);
        let resolve = react_static::ResolveTagName {
            name: r["resolveTagName"]["name"].as_str().unwrap(),
            source: r["resolveTagName"]["source"].as_str().unwrap(),
        };
        let options = react_static::StaticOptions {
            resolve_tag_name: Some(resolve),
        };
        let got = react_static::render(&tree, &options).expect("renders");
        assert_eq!(got, r["result"].as_str().unwrap(), "{}", label(r));
    }
    // A resolveTagName of any other name throws, as upstream's does.
    let options = react_static::StaticOptions {
        resolve_tag_name: Some(react_static::ResolveTagName {
            name: "resolve",
            source: "function resolve() {}",
        }),
    };
    let error = react_static::render(&JsValue::Null, &options).unwrap_err();
    assert_eq!(
        index::thrown(&error),
        "Error: resolveTagName must be named tagName"
    );
}

/// A React whose elements are `{name, attributes, children}` JSON, as react.test.ts's own mock
/// (`createElement(name, attributes, ...children) => ({name, attributes, children})`).
struct Mock;

impl react::React for Mock {
    type Element = Value;
    type Component = Value;

    fn create_element(
        &mut self,
        element_type: react::ElementType<'_, Value>,
        props: Option<react::Prop<'_, Value>>,
        children: Vec<react::Rendered<'_, Value>>,
    ) -> Value {
        fn prop(p: react::Prop<'_, Value>) -> Value {
            match p {
                react::Prop::Value(v) => {
                    serde_json::from_str(&json_stringify(&v, "").unwrap().unwrap_or("null".into()))
                        .unwrap()
                }
                react::Prop::Array(items) => Value::Array(items.into_iter().map(prop).collect()),
                react::Prop::Object(o) => Value::Object(
                    o.into_iter()
                        .map(|(k, v)| (k.into_owned(), prop(v)))
                        .collect(),
                ),
                react::Prop::Element(e) => e,
            }
        }
        let name = match element_type {
            react::ElementType::Fragment => Value::String("Fragment".into()),
            react::ElementType::Name(n) => serde_json::from_str(
                &json_stringify(&n.to_value(), "")
                    .unwrap()
                    .unwrap_or("null".into()),
            )
            .unwrap(),
            react::ElementType::Component(c) => c.unwrap_or(Value::Null),
            react::ElementType::Inherited(name) => Value::String(name),
        };
        let children = children
            .into_iter()
            .map(|c| match c {
                react::Rendered::Element(e) => e,
                react::Rendered::Value(v) => {
                    serde_json::from_str(&json_stringify(&v, "").unwrap().unwrap_or("null".into()))
                        .unwrap()
                }
            })
            .collect::<Vec<Value>>();
        serde_json::json!({"name": name, "attributes": props.map(prop), "children": children})
    }
}

#[test]
fn react_with_a_resolve_tag_name_of_the_tests() {
    // react.test.ts: `dynamic(tree, React, {components: {Foo: 'bar'}, resolveTagName:
    // (name, components) => components.Foo})` makes a `bar` element.
    let all = records("react", None);
    assert_eq!(all.len(), 1);
    for r in &all {
        let tree = JsValue::from_json(&r["tree"]);
        let components: indexmap::IndexMap<String, Value> = r["options"]["components"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        fn resolve<'n>(
            _: &react::Name<'n>,
            c: react::Components<'_, Value>,
        ) -> markdoc::types::Result<react::ElementType<'n, Value>> {
            Ok(react::ElementType::Component(match c {
                react::Components::Map(map) => map.get("Foo").cloned(),
                react::Components::Function(f) => f(&"Foo".into()),
            }))
        }
        let options = react::RenderOptions {
            components: Some(react::Components::Map(&components)),
            resolve_tag_name: Some(&resolve),
        };
        let got = match react::render(&tree, &mut Mock, &options).expect("renders") {
            react::Rendered::Element(e) => e,
            react::Rendered::Value(_) => panic!("an element"),
        };
        assert_eq!(got, r["result"], "{}", label(r));
    }
}

#[test]
fn linkify_it_fixtures() {
    // linkify-it 6.1.0's links.txt and not_links.txt, with the normalizer killed as its own
    // test kills it (`l.normalize = function () {}`).
    let all = suite("linkify-fixtures.jsonl");
    let mut failures = Vec::new();
    for r in &all {
        let o = &r["options"];
        let flag = |k: &str, default: bool| o.get(k).and_then(Value::as_bool).unwrap_or(default);
        let mut l = LinkifyIt::with_options(LinkifyOptions {
            fuzzy_link: flag("fuzzyLink", false),
            fuzzy_email: flag("fuzzyEmail", true),
            fuzzy_ip: flag("fuzzyIP", false),
            url_auth: flag("urlAuth", false),
            ..LinkifyOptions::default()
        });
        l.set_normalizer(Some(|_| {}));
        let text = r["text"].as_str().unwrap();
        let at = format!("{} line {}", r["file"].as_str().unwrap(), r["line"]);
        let units = js::to_utf16(text);
        // (linkify-it's calls throw where V8's do: `maxLength` 0 alone; none here.)
        let url = l
            .match_all(&units)
            .expect("linkify-it's match")
            .map(|m| String::from_utf16_lossy(&m[0].url));
        if l.test(&units).expect("linkify-it's test") != r["test"].as_bool().unwrap() {
            failures.push(format!("{at}: test({text:?})"));
        }
        let expected_url = r.get("url").or_else(|| r.get("match")).unwrap().as_str();
        if url.as_deref() != expected_url {
            failures.push(format!(
                "{at}: match({text:?}) {url:?}, upstream {expected_url:?}"
            ));
        }
        if let Some(test_nl) = r.get("test_nl") {
            let nl = js::to_utf16(&format!("\n{text}\n"));
            if l.test(&nl).expect("linkify-it's test") != test_nl.as_bool().unwrap() {
                failures.push(format!("{at}: test(\\n{text:?}\\n)"));
            }
            // linkify-it's own expectation holds too
            if expected_url != r["expected"].as_str() {
                failures.push(format!("{at}: node's url is not the fixture's"));
            }
        }
    }
    assert_none(failures, all.len(), 171);
}

#[test]
fn formatter_test_ts() {
    // formatter.test.ts: every `format(ast, options)` it makes, with node's result (the ASTs
    // are upstream's parse of the test's documents, or the test's own hand-made nodes), and
    // `format(null)`.
    use markdoc::next::formatter::{self, FormatOptions, OrderedListMode};
    let mut failures = Vec::new();
    let all = suite("upstream-format.jsonl");
    for r in &all {
        let ast = node_from_json(&r["ast"]).expect("upstream's AST");
        let o = &r["options"];
        let options = FormatOptions {
            allow_indentation: o["allowIndentation"].as_bool().unwrap_or(false),
            // a number JSON cannot hold is written as its String() ("Infinity")
            max_tag_opening_width: match &o["maxTagOpeningWidth"] {
                Value::String(s) => Some(js::string_to_number(&js::to_utf16(s))),
                other => other.as_f64(),
            },
            ordered_list_mode: match o["orderedListMode"].as_str() {
                Some("increment") => Some(OrderedListMode::Increment),
                Some(_) => Some(OrderedListMode::Repeat),
                None => None,
            },
            parent: None,
            indent: o["indent"].as_f64(),
        };
        match formatter::format(&ast, &options) {
            Ok(got) if got == r["result"].as_str().unwrap() => {}
            Ok(got) => failures.push(format!("{}: {got:?}, upstream {}", label(r), r["result"])),
            Err(e) => failures.push(format!("{}: {}", label(r), index::thrown(&e))),
        }
    }
    assert_none(failures, all.len(), 61);
    let null = records("format", None);
    assert_eq!(null.len(), 1);
    let got = formatter::format_value(&Scalar::Null, &FormatOptions::default()).unwrap();
    assert_eq!(got, null[0]["result"].as_str().unwrap());
}
