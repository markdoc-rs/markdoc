//! Upstream's `src/validator.test.ts`, ported. The async case is not: nothing here is
//! asynchronous.
//!
//! Upstream's tests validate `Markdoc.parse(doc)`. The parser is ported elsewhere, so the
//! documents' ASTs are upstream's own, in `asts.json` beside this file: `Markdoc.parse(doc)`
//! for each document, written by node with `@markdoc/markdoc` 0.5.11's `dist/index.js`
//! (`JSON.parse(JSON.stringify(Markdoc.parse(doc)))` per document, keyed by the document).

use super::*;
use crate::next::compare::node_from_json;
use crate::next::index;
use crate::next::schema::{CustomType, Schema, ValidateFn};
use indexmap::IndexMap;
use serde_json::json;
use std::sync::{Arc, LazyLock, Mutex};

static ASTS: LazyLock<serde_json::Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("asts.json")).expect("asts.json"));

fn ast(doc: &str) -> Node {
    let json = ASTS
        .get(doc)
        .unwrap_or_else(|| panic!("no AST for {doc:?} in asts.json"));
    node_from_json(json).expect("an upstream AST")
}

fn config(json: serde_json::Value) -> Config {
    Config::from_json(&json).expect("a config")
}

fn validate(doc: &str, config: &Config) -> Vec<ValidateError> {
    index::validate(&ast(doc), config).expect("validate")
}

/// `expect(output).toDeepEqualSubset([{error: {id, message}}, ...])`.
fn assert_errors(output: &[ValidateError], expected: &[(&str, &str)]) {
    assert!(
        output.len() >= expected.len(),
        "{} errors, expected at least {}: {output:#?}",
        output.len(),
        expected.len()
    );
    for (got, (id, message)) in output.iter().zip(expected) {
        assert_eq!(&got.error.id, id, "{output:#?}");
        if !message.is_empty() {
            assert_eq!(&got.error.message, message, "{output:#?}");
        }
    }
}

// function validation

#[test]
fn ensures_that_function_exists() {
    let output = validate(
        "{% foo bar=baz() /%}",
        &config(json!({
            "validation": {"validateFunctions": true},
            "tags": {"foo": {"attributes": {"bar": {"type": {"$type": "String"}}}}}
        })),
    );
    assert_errors(
        &output,
        &[("function-undefined", "Undefined function: 'baz'")],
    );
}

fn return_type_schema() -> serde_json::Value {
    json!({
        "validation": {"validateFunctions": true},
        "functions": {
            "baz": {"returns": {"$type": "String"}},
            "number": {"returns": {"$type": "Number"}},
            "nested": {
                "returns": {"$type": "String"},
                "parameters": {"0": {"type": {"$type": "String"}}, "1": {"type": {"$type": "Number"}}}
            },
            "withUnion": {"returns": [{"$type": "String"}, {"$type": "Number"}], "parameters": {}}
        },
        "tags": {
            "foo": {"attributes": {"bar": {"type": {"$type": "String"}}}},
            "union-tag-1": {"attributes": {
                "foo": {"type": {"$type": "String"}},
                "bar": {"type": {"$type": "Number"}},
                "baz": {"type": {"$type": "Boolean"}}
            }}
        }
    })
}

#[test]
fn return_types_correctly_handles_union_types() {
    let schema = config(return_type_schema());
    assert!(
        validate(
            "{% union-tag-1 foo=withUnion() bar=withUnion() /%}",
            &schema
        )
        .is_empty()
    );
    let output = validate(
        "{% union-tag-1 foo=withUnion() bar=withUnion() baz=withUnion() /%}",
        &schema,
    );
    assert_errors(
        &output,
        &[(
            "attribute-type-invalid",
            "Attribute 'baz' must be type of 'Boolean'",
        )],
    );
}

#[test]
fn return_types_correctly_handles_nested_function_calls() {
    let schema = config(return_type_schema());
    assert!(validate("{% foo bar=nested(baz(), number()) /%}", &schema).is_empty());
    let output = validate("{% foo bar=nested(number(), baz()) /%}", &schema);
    assert_errors(
        &output,
        &[
            (
                "parameter-type-invalid",
                "Parameter '0' of 'nested' must be type of 'String'",
            ),
            (
                "parameter-type-invalid",
                "Parameter '1' of 'nested' must be type of 'Number'",
            ),
        ],
    );
}

#[test]
fn return_types_accepts_a_correct_return_type() {
    assert!(validate("{% foo bar=baz() /%}", &config(return_type_schema())).is_empty());
}

#[test]
fn return_types_correctly_handles_no_return_type() {
    let mut schema = return_type_schema();
    schema["functions"] = json!({"baz": {}});
    assert!(validate("{% foo bar=baz() /%}", &config(schema)).is_empty());
}

#[test]
fn return_types_identifies_an_incorrect_return_type() {
    let mut schema = return_type_schema();
    schema["functions"] = json!({"baz": {"returns": {"$type": "Number"}}});
    let output = validate("{% foo bar=baz() /%}", &config(schema));
    assert_errors(&output, &[("attribute-type-invalid", "")]);
}

fn parameter_schema() -> Config {
    config(json!({
        "validation": {"validateFunctions": true},
        "functions": {
            "baz": {"returns": {"$type": "String"}, "parameters": {}},
            "qux": {"returns": {"$type": "String"}, "parameters": {"test": {"type": {"$type": "String"}}}},
            "noTyping": {},
            "requiredParam": {
                "returns": {"$type": "String"},
                "parameters": {
                    "test": {"type": {"$type": "String"}},
                    "req": {"type": {"$type": "String"}, "required": true}
                }
            }
        },
        "tags": {"foo": {"attributes": {"bar": {"type": {"$type": "String"}}}}}
    }))
}

#[test]
fn parameters_with_a_missing_optional_parameter() {
    assert!(validate("{% foo bar=qux() /%}", &parameter_schema()).is_empty());
}

#[test]
fn parameters_with_a_missing_required_parameter() {
    let output = validate("{% foo bar=requiredParam() /%}", &parameter_schema());
    assert_errors(
        &output,
        &[(
            "parameter-missing-required",
            "Missing required parameter: 'req'",
        )],
    );
}

#[test]
fn parameters_accepts_a_keyed_parameter() {
    assert!(validate("{% foo bar=qux(test=\"example\") /%}", &parameter_schema()).is_empty());
}

#[test]
fn parameters_ignored_when_there_is_no_typing() {
    assert!(validate("{% foo bar=noTyping(foo=1) /%}", &parameter_schema()).is_empty());
}

#[test]
fn parameters_rejects_an_undeclared_keyed_parameter() {
    let output = validate("{% foo bar=baz(foo=1) /%}", &parameter_schema());
    assert_errors(
        &output,
        &[("parameter-undefined", "Invalid parameter: 'foo'")],
    );
    assert_eq!(output[0].node_type, NodeType::Tag);
}

#[test]
fn parameters_rejects_an_undeclared_positional_parameter() {
    let output = validate("{% foo bar=baz(1) /%}", &parameter_schema());
    assert_errors(
        &output,
        &[("parameter-undefined", "Invalid parameter: '0'")],
    );
    let output = validate("{% foo bar=baz(1, test=2) /%}", &parameter_schema());
    assert_errors(
        &output,
        &[("parameter-undefined", "Invalid parameter: '0'")],
    );
}

// inline rule

fn inline_config() -> Config {
    config(json!({"tags": {"foo": {"inline": true}, "bar": {"inline": false}, "baz": {}}}))
}

#[test]
fn inline_or_block_allowed_when_undefined() {
    assert!(validate("this is inline {% baz %}bar{% /baz %}", &inline_config()).is_empty());
    assert!(validate("\n{% baz %}\nbar\n{% /baz %}\n      ", &inline_config()).is_empty());
}

#[test]
fn validates_inline_tag() {
    assert!(validate("this is inline {% foo %}bar{% /foo %}", &inline_config()).is_empty());
    let wrong = validate("\n{% foo %}\nbar\n{% /foo %}\n      ", &inline_config());
    assert_eq!(wrong[0].error.id, "tag-placement-invalid");
    assert!(wrong[0].error.message.contains("should be inline"));
}

#[test]
fn validates_block_tag() {
    assert!(validate("\n{% bar %}\nbar\n{% /bar %}\n", &inline_config()).is_empty());
    let wrong = validate("this is inline {% bar %}bar{% /bar %}", &inline_config());
    assert_eq!(wrong[0].error.id, "tag-placement-invalid");
    assert!(wrong[0].error.message.contains("should be block"));
}

// attribute validation

#[test]
fn attribute_with_a_validate_function() {
    let validate_fn: crate::next::schema::AttributeValidateFn = Arc::new(|value, _cx, _key| {
        let n = match value {
            Scalar::Number(n) => *n,
            _ => f64::NAN,
        };
        Ok(if n > 10.0 {
            Vec::new()
        } else {
            vec![ValidationError {
                id: "attribute-should-be-greater-than-ten".into(),
                level: ValidationLevel::Error,
                message: "Attribute \"bar\" must have value greater than 10.".into(),
                location: None,
                raw: None,
                span: None,
            }]
        })
    });
    let mut schema =
        config(json!({"tags": {"foo": {"attributes": {"bar": {"type": {"$type": "Number"}}}}}}));
    schema.tags["foo"]
        .as_mut()
        .unwrap()
        .attributes
        .as_mut()
        .unwrap()["bar"]
        .validate = Some(validate_fn);
    assert!(validate("{% foo bar=20 /%}", &schema).is_empty());
    let output = validate("{% foo bar=5 /%}", &schema);
    assert_errors(
        &output,
        &[(
            "attribute-should-be-greater-than-ten",
            "Attribute \"bar\" must have value greater than 10.",
        )],
    );
    assert_eq!(output[0].node_type, NodeType::Tag);
}

#[test]
fn should_return_error_on_failure_to_match_array() {
    let schema = config(json!({"tags": {"foo": {"attributes": {"jawn": {
        "type": {"$type": "String"}, "matches": ["bar", "baz", "bat"]
    }}}}}));
    let output = validate("{% foo jawn=\"cat\" /%}", &schema);
    assert_errors(
        &output,
        &[(
            "attribute-value-invalid",
            "Attribute 'jawn' must match one of [\"bar\",\"baz\",\"bat\"]. Got 'cat' instead.",
        )],
    );
}

#[test]
fn elides_excess_values_in_matches_check() {
    let letters: Vec<String> = "foobarbazqux".chars().map(String::from).collect();
    let schema = config(json!({"tags": {"foo": {"attributes": {"jawn": {
        "type": {"$type": "String"}, "matches": letters
    }}}}}));
    let output = validate("{% foo jawn=\"cat\" /%}", &schema);
    assert_errors(
        &output,
        &[(
            "attribute-value-invalid",
            "Attribute 'jawn' must match one of [\"f\",\"o\",\"o\",\"b\",\"a\",\"r\",\"b\",\"a\", ... 4 more]. Got 'cat' instead.",
        )],
    );
}

#[test]
fn should_validate_partial_file_attributes() {
    let output = validate(
        "{% partial file=\"non-existent.md\" /%}",
        &Config::default(),
    );
    assert_errors(
        &output,
        &[(
            "attribute-value-invalid",
            "Partial `non-existent.md` not found. The 'file' attribute must be set in `config.partials`",
        )],
    );
    assert_eq!(output[0].error.level, ValidationLevel::Error);
}

#[test]
fn properly_validates_ids() {
    assert!(validate("# foo {% #bar %}", &Config::default()).is_empty());
    let number = validate("# foo {% #1bar %}", &Config::default());
    assert_eq!(number[0].error.id, "attribute-value-invalid");
    let hash = validate("# foo {% id=\"#bar\" %}", &Config::default());
    assert_eq!(hash[0].error.id, "attribute-value-invalid");
}

// custom type registration example

struct Link;
impl CustomType for Link {
    fn name(&self) -> &str {
        "Link"
    }
    fn validate(
        &self,
        value: &Scalar,
        _cx: &ValidationContext<'_>,
        _key: &str,
    ) -> Option<Result<Vec<ValidationError>>> {
        let ok = matches!(value, Scalar::String(s) if s.starts_with("http"));
        Some(Ok(if ok {
            Vec::new()
        } else {
            vec![ValidationError {
                id: "attribute-type-invalid".into(),
                level: ValidationLevel::Error,
                message: "Attribute 'href' must be type of 'Link'".into(),
                location: None,
                raw: None,
                span: None,
            }]
        }))
    }
}

fn link_config(self_closing: bool) -> Config {
    let mut attributes = IndexMap::new();
    attributes.insert(
        "href".to_string(),
        SchemaAttribute {
            attr_type: Some(TypeSpec::Custom(Arc::new(Link))),
            ..Default::default()
        },
    );
    let mut c = Config::default();
    c.tags.insert(
        "link".into(),
        Some(Schema {
            render: Some("a".into()),
            self_closing,
            attributes: Some(attributes),
            ..Default::default()
        }),
    );
    c
}

#[test]
fn custom_type_should_return_error_on_failure() {
    let output = validate("{% link href=\"/relative-link\"  /%}", &link_config(false));
    assert_errors(
        &output,
        &[(
            "attribute-type-invalid",
            "Attribute 'href' must be type of 'Link'",
        )],
    );
    assert_eq!(output[0].error.level, ValidationLevel::Error);
}

#[test]
fn custom_type_should_return_no_errors_when_valid() {
    assert!(
        validate(
            "{% link href=\"http://google.com\"  /%}",
            &link_config(true)
        )
        .is_empty()
    );
}

// variable validation

#[test]
fn variables_only_validated_if_the_config_has_them() {
    assert!(validate("{% $valid.variable %}", &Config::default()).is_empty());
}

#[test]
fn should_warn_against_missing_variables() {
    let output = validate(
        "{% $undefinedVariable %}",
        &config(json!({"variables": {}})),
    );
    assert_errors(
        &output,
        &[(
            "variable-undefined",
            "Undefined variable: 'undefinedVariable'",
        )],
    );
    assert_eq!(output[0].node_type, NodeType::Text);
    assert_eq!(output[0].error.level, ValidationLevel::Error);
}

#[test]
fn should_not_warn_if_variable_exists() {
    let c = config(json!({"variables": {"valid": {"variable": false}}}));
    assert!(validate("{% $valid.variable %}", &c).is_empty());
}

#[test]
fn should_not_error_for_missing_support_for_code_block() {
    let doc =
        "   # https://spec.commonmark.org/0.30/#indented-code-block\n    4-space indented code";
    assert!(validate(doc, &Config::default()).is_empty());
}

// attribute validation key

fn baz_below_five(value: &Scalar, name: &str) -> Vec<ValidationError> {
    let below = matches!(value, Scalar::Object(o) if matches!(o.get("baz"), Some(Scalar::Number(n)) if *n < 5.0));
    if below {
        vec![ValidationError {
            id: "invalid-foo-bar".into(),
            level: ValidationLevel::Error,
            message: format!("The value of '{name}.baz' must be less than five"),
            location: None,
            raw: None,
            span: None,
        }]
    } else {
        Vec::new()
    }
}

#[test]
fn attribute_validation_key_simple_case() {
    let validator: crate::next::schema::AttributeValidateFn =
        Arc::new(|value, _cx, name| Ok(baz_below_five(value, name)));
    let mut c = config(json!({"tags": {"foo": {"attributes": {
        "bar": {"type": {"$type": "Object"}},
        "blah": {"type": {"$type": "Object"}}
    }}}}));
    for (_, attr) in c.tags["foo"].as_mut().unwrap().attributes.as_mut().unwrap() {
        attr.validate = Some(validator.clone());
    }
    let errs = validate("{% foo bar={baz: 3} /%}", &c);
    assert_eq!(
        errs[0].error.message,
        "The value of 'bar.baz' must be less than five"
    );
}

struct CustomTypeBaz;
impl CustomType for CustomTypeBaz {
    fn name(&self) -> &str {
        "CustomType"
    }
    fn validate(
        &self,
        value: &Scalar,
        _cx: &ValidationContext<'_>,
        key: &str,
    ) -> Option<Result<Vec<ValidationError>>> {
        Some(Ok(baz_below_five(value, key)))
    }
}

#[test]
fn attribute_validation_key_custom_attribute_type() {
    let t: Arc<dyn CustomType> = Arc::new(CustomTypeBaz);
    let mut attributes = IndexMap::new();
    for name in ["bar", "blah"] {
        attributes.insert(
            name.to_string(),
            SchemaAttribute {
                attr_type: Some(TypeSpec::Custom(t.clone())),
                ..Default::default()
            },
        );
    }
    let mut c = Config::default();
    c.tags.insert(
        "foo".into(),
        Some(Schema {
            attributes: Some(attributes),
            ..Default::default()
        }),
    );
    let errs = validate("{% foo bar={baz: 3} /%}", &c);
    assert_eq!(
        errs[0].error.message,
        "The value of 'bar.baz' must be less than five"
    );
}

// parent validation

#[test]
fn parent_validation_for_deep_nesting() {
    let doc = "\n{% foo %}\n{% bar %}\n{% baz %}\n# testing\n{% /baz %}\n{% /bar %}\n{% /foo %}\n";
    let doc2 = "\n{% foo %}\n{% bar %}\n{% /bar %}\n{% /foo %}\n\n{% bar %}\n{% baz %}\n# testing\n{% /baz %}\n{% /bar %}\n";
    let validate_fn: ValidateFn = Arc::new(|_node, cx| {
        Ok(
            if cx.parents.iter().any(|p| p.tag.as_deref() == Some("foo")) {
                vec![ValidationError {
                    id: "heading-in-foo".into(),
                    level: ValidationLevel::Error,
                    message: "Can't nest a heading in tag 'foo'".into(),
                    location: None,
                    raw: None,
                    span: None,
                }]
            } else {
                Vec::new()
            },
        )
    });
    let mut c = config(json!({"tags": {"foo": {}, "bar": {}, "baz": {}}}));
    let mut heading = crate::next::schema::heading();
    heading.validate = Some(validate_fn);
    c.nodes.insert("heading".into(), Some(heading));

    let errors = validate(doc, &c);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].error.id, "heading-in-foo");
    assert_eq!(validate(doc2, &c).len(), 0);
}

#[test]
fn parent_validation_with_function_validation_enabled() {
    let seen: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let record = seen.clone();
    let validate_fn: ValidateFn = Arc::new(move |_node, cx| {
        record
            .lock()
            .unwrap()
            .push(cx.parents.iter().map(|p| p.node_type.to_string()).collect());
        Ok(Vec::new())
    });
    let mut c = config(json!({"validation": {"validateFunctions": true}, "tags": {"foo": {}}}));
    c.tags.insert(
        "bar".into(),
        Some(Schema {
            validate: Some(validate_fn),
            ..Default::default()
        }),
    );
    validate("{% foo %}{% bar %}this is a test{% /bar %}{% /foo %}", &c);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![vec!["document", "paragraph", "inline", "tag"]]
    );
}

// Beyond upstream's tests: the JS semantics the port relies on.

#[test]
fn a_tag_named_like_an_object_method_has_a_schema() {
    // `tags['toString']` is a function: no `tag-undefined`, and no attribute is defined.
    let doc = ast("{% foo bar=20 /%}");
    let mut renamed = doc.clone();
    renamed.children[0].tag = Some("toString".into());
    let output = index::validate(&renamed, &Config::default()).unwrap();
    assert_errors(
        &output,
        &[("attribute-undefined", "Invalid attribute: 'bar'")],
    );
    let mut unknown = doc;
    unknown.children[0].tag = Some("nope".into());
    let output = index::validate(&unknown, &Config::default()).unwrap();
    assert_errors(&output, &[("tag-undefined", "Undefined tag: 'nope'")]);
}

#[test]
fn a_global_regexp_keeps_its_last_index_from_node_to_node() {
    // node: /a/g tests 'a' true, then false (lastIndex 1), then true again.
    let c = config(
        json!({"tags": {"foo": {"attributes": {"bar": {"matches": {"$regex": "a", "flags": "g"}}}}}}),
    );
    let mut doc = ast("{% foo jawn=\"cat\" /%}");
    let mut node = doc.children[0].clone();
    node.attributes.clear();
    node.attributes
        .insert("bar".into(), Scalar::String("a".into()));
    doc.children = vec![node.clone(), node.clone(), node];
    let output = index::validate(&doc, &c).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(
        output[0].error.message,
        "Attribute 'bar' must match /a/g. Got 'a' instead."
    );
}

#[test]
fn a_variable_path_through_null_throws_when_variables_are_checked() {
    let c = config(json!({"variables": {"valid": null}}));
    assert!(index::validate(&ast("{% $valid.variable %}"), &c).is_err());
}

// Depth: upstream's recursive walk overflows V8's stack; the port throws its RangeError there
// (MAX_VALIDATE_DEPTH, node 22's measured limit) and validates on a stack of its own.

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

/// A document whose deepest node, a text node under nested `x` tags, is the `depth`th on its
/// path from the root.
fn chain(depth: usize) -> Node {
    let content = vec![("content".to_string(), Scalar::String("a".into()))];
    let mut node = Node::new(NodeType::Text, content, vec![], None);
    for _ in 0..depth - 2 {
        node = Node::new(
            NodeType::Tag,
            Vec::<(String, Scalar)>::new(),
            vec![node],
            Some("x".into()),
        );
    }
    Node::new(
        NodeType::Document,
        Vec::<(String, Scalar)>::new(),
        vec![node],
        None,
    )
}

#[test]
fn validate_throws_past_upstreams_depth() {
    // node 22: Markdoc.validate of a document with 4,043 nested tags between the document and
    // a text node (4,045 nodes on the path) returns; with 4,044 it throws RangeError.
    on_2_mib(|| {
        let config = config(json!({"tags": {"x": {}}}));
        let errors = index::validate(&chain(MAX_VALIDATE_DEPTH), &config).expect("validates");
        assert!(errors.is_empty());
        let err = index::validate(&chain(MAX_VALIDATE_DEPTH + 1), &config).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Validation error: RangeError: Maximum call stack size exceeded"
        );
    });
}

#[test]
fn validate_throws_before_it_validates_a_node() {
    // Upstream collects the whole walk first: a `matches` RegExp with `g` on the document's
    // first node is not tested (its lastIndex stays 0) when the tree is too deep.
    on_2_mib(|| {
        let config = config(json!({"tags": {"x": {}, "m": {"attributes": {
            "a": {"matches": {"$regex": "a", "flags": "g"}}}}}}));
        let mut doc = chain(MAX_VALIDATE_DEPTH + 1);
        let m = Node::new(
            NodeType::Tag,
            vec![("a".to_string(), Scalar::String("a".into()))],
            vec![],
            Some("m".into()),
        );
        doc.children.insert(0, m);
        let regexp = || match &config.tags["m"]
            .as_ref()
            .unwrap()
            .attributes
            .as_ref()
            .unwrap()["a"]
            .matches
        {
            Some(Matches::RegExp(r)) => r.clone(),
            _ => panic!("a RegExp"),
        };
        assert!(index::validate(&doc, &config).is_err());
        assert_eq!(regexp().last_index(), 0);
        doc.children.truncate(1);
        assert!(index::validate(&doc, &config).unwrap().is_empty());
        assert_eq!(regexp().last_index(), 1);
    });
}

#[test]
fn walk_with_parents_passes_the_path_down() {
    // The order upstream's generator yields: a node, then its slots', then its children's.
    let mut slot = Node::new(
        NodeType::Tag,
        Vec::<(String, Scalar)>::new(),
        vec![],
        Some("s".into()),
    );
    slot.children.push(Node::new(
        NodeType::Text,
        Vec::<(String, Scalar)>::new(),
        vec![],
        None,
    ));
    let mut tag = Node::new(
        NodeType::Tag,
        Vec::<(String, Scalar)>::new(),
        vec![Node::new(
            NodeType::Paragraph,
            Vec::<(String, Scalar)>::new(),
            vec![],
            None,
        )],
        Some("t".into()),
    );
    tag.slots.insert("a".into(), slot);
    let doc = Node::new(
        NodeType::Document,
        Vec::<(String, Scalar)>::new(),
        vec![tag],
        None,
    );
    let mut seen = Vec::new();
    walk_with_parents::<()>(&doc, |node, parents| {
        let name = |n: &Node| n.tag.clone().unwrap_or_else(|| n.node_type.to_string());
        seen.push(format!(
            "{} < {}",
            name(node),
            parents
                .iter()
                .map(|p| name(p))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        seen,
        [
            "document < ",
            "t < document",
            "s < document t",
            "text < document t s",
            "paragraph < document t",
        ]
    );
}

#[test]
fn a_type_named_after_an_object_method_is_not_a_constructor() {
    // node 22: validate throws "TypeError: type is not a constructor" (V8 names the callee by
    // its source text: `new type()` in validateType), for an attribute's type, a union member
    // it reaches and a function parameter's type alike.
    let doc = || {
        Node::new(
            NodeType::Document,
            Vec::<(String, Scalar)>::new(),
            vec![Node::new(
                NodeType::Tag,
                vec![("a".to_string(), Scalar::Number(1.0))],
                vec![],
                Some("foo".into()),
            )],
            None,
        )
    };
    for t in [
        json!("toString"),
        json!("hasOwnProperty"),
        json!("__lookupSetter__"),
        json!(["toString", "Number"]),
    ] {
        let config = config(json!({"tags": {"foo": {"attributes": {"a": {"type": t}}}}}));
        assert_eq!(
            index::validate(&doc(), &config).unwrap_err().to_string(),
            "Validation error: TypeError: type is not a constructor",
            "{t}"
        );
    }
    // A union whose first member validates never reaches the method.
    let config =
        config(json!({"tags": {"foo": {"attributes": {"a": {"type": ["Number", "toString"]}}}}}));
    assert!(index::validate(&doc(), &config).unwrap().is_empty());
}

// --- upstream's positions in validate()'s entries --------------------------------------------

/// `validate()`'s entries as `JSON.stringify` writes them: `{type, lines, location?, error}`.
fn entries_json(output: &[ValidateError]) -> String {
    use crate::next::compare::{ValidateRecord, errors_json, to_json_string};
    let records: Vec<ValidateRecord> = output
        .iter()
        .map(|e| ValidateRecord {
            node_type: e.node_type.to_string(),
            lines: e.lines.clone(),
            location: e.location.as_ref().map(|l| l.to_json()),
            error: e.error.clone(),
        })
        .collect();
    to_json_string(&errors_json(&records))
}

/// What node 22 writes for `JSON.stringify(Markdoc.validate(Markdoc.parse(tokens, {file,
/// location: true}), {}))` (work/positions/probes/validate_probe.cjs): an error is placed by
/// its node (`lines` the node's, which a closing grew; `location` the node's) unless its own
/// `location` has a numeric `line` at both ends (a tag in a fence, or a hand-made one), when
/// `lines` are those two and `location` is `{file: node.location?.file, ...error.location}`
/// (keys in a JS object's order); an error a hand-made token gave that is not an object is
/// kept as it is, and a `null` one throws.
#[test]
fn entries_carry_upstreams_lines_and_location() {
    use crate::next::compare::tokens_from_json;
    use crate::next::parser::ParserArgs;
    let file = |f: &str| ParserArgs {
        file: Some(f.into()),
        ..ParserArgs::default()
    };
    let from_source = |src: &str, args: &ParserArgs| {
        let doc = index::parse(src, args).expect("parses");
        entries_json(&index::validate(&doc, &Config::default()).expect("validates"))
    };
    assert_eq!(
        from_source(
            "# Hi {% .a %}\n\n{% callout type=\"x\" %}\nSome *em* text\n{% /callout %}\n",
            &file("f.md")
        ),
        r#"[{"type":"tag","lines":[2,3,4,5],"location":{"file":"f.md","start":{"line":2},"end":{"line":3}},"error":{"id":"tag-undefined","level":"critical","message":"Undefined tag: 'callout'"}}]"#
    );
    assert_eq!(
        from_source("```\nx {% a b=, %}\n```\n", &file("f.md")),
        r#"[{"type":"error","lines":[1,2],"location":{"file":"f.md","start":{"line":1,"character":7},"end":{"line":2,"character":8}},"error":{"id":"parse-error","level":"critical","message":"Expected \"[\", \"{\", boolean, identifier, null, number, string, or variable but \",\" found.","location":{"start":{"line":1,"character":7},"end":{"line":2,"character":8}}}}]"#
    );
    assert_eq!(
        from_source("{% a b=, %}\n", &ParserArgs::default()),
        r#"[{"type":"error","lines":[0,1],"location":{"start":{"line":0},"end":{"line":1}},"error":{"id":"parse-error","level":"critical","message":"Expected \"[\", \"{\", boolean, identifier, null, number, string, or variable but \",\" found.","location":{"start":{"offset":6},"end":{"offset":7}}}}]"#
    );

    let text = |extra: serde_json::Value| {
        let mut t = json!({"type": "text", "content": "x", "map": null});
        t.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        t
    };
    let hand_made = |tokens: serde_json::Value, args: &ParserArgs| {
        let tokens = tokens_from_json(&tokens).expect("tokens");
        let doc = index::parse_tokens(&tokens, "", args).expect("parses");
        index::validate(&doc, &Config::default()).map(|output| entries_json(&output))
    };
    let errors = json!([5, "s", [1, 2],
        {"id": "e", "level": "bogus", "message": "m"},
        {"id": "e", "level": "warning", "message": "m",
         "location": {"end": {"line": 4}, "start": {"line": 3.5, "character": 1}, "1": 0}},
        {"id": "e", "level": "error", "message": "m",
         "location": {"file": "g.md", "start": {"line": 1}, "end": {"line": 2}}},
        {"id": "e", "level": "error", "message": "m",
         "location": {"start": {"line": 1}, "end": {"line": 2}, "file": 5}}]);
    assert_eq!(
        hand_made(
            json!([text(json!({"map": [3, 4], "errors": errors}))]),
            &file("f.md")
        )
        .unwrap(),
        concat!(
            r#"[{"type":"document","lines":[],"error":{"id":"child-invalid","level":"warning","message":"Can't nest 'text' in 'document'"}},"#,
            r#"{"type":"text","lines":[3,4],"location":{"file":"f.md","start":{"line":3},"end":{"line":4}},"error":5},"#,
            r#"{"type":"text","lines":[3,4],"location":{"file":"f.md","start":{"line":3},"end":{"line":4}},"error":"s"},"#,
            r#"{"type":"text","lines":[3,4],"location":{"file":"f.md","start":{"line":3},"end":{"line":4}},"error":[1,2]},"#,
            r#"{"type":"text","lines":[3,4],"location":{"file":"f.md","start":{"line":3},"end":{"line":4}},"error":{"id":"e","level":"bogus","message":"m"}},"#,
            r#"{"type":"text","lines":[3.5,4],"location":{"1":0,"file":"f.md","end":{"line":4},"start":{"line":3.5,"character":1}},"error":{"id":"e","level":"warning","message":"m","location":{"1":0,"end":{"line":4},"start":{"line":3.5,"character":1}}}},"#,
            r#"{"type":"text","lines":[1,2],"location":{"file":"g.md","start":{"line":1},"end":{"line":2}},"error":{"id":"e","level":"error","message":"m","location":{"file":"g.md","start":{"line":1},"end":{"line":2}}}},"#,
            r#"{"type":"text","lines":[3,4],"location":{"file":"f.md","start":{"line":3},"end":{"line":4}},"error":{"id":"e","level":"error","message":"m","location":{"start":{"line":1},"end":{"line":2},"file":5}}}]"#
        )
    );
    // The typed fields of a kept value: what `String()` makes of its members.
    let tokens = tokens_from_json(&json!([text(json!({"errors": [
        {"id": 1, "level": "bogus", "message": {"a": 1}}, 5]}))]))
    .unwrap();
    let doc = index::parse_tokens(&tokens, "", &ParserArgs::default()).unwrap();
    let kept = &doc.children[0].errors;
    assert_eq!(
        (
            kept[0].id.as_str(),
            &kept[0].level,
            kept[0].message.as_str()
        ),
        ("1", &ValidationLevel::Error, "[object Object]")
    );
    assert!(
        kept[0].raw.is_some() && kept[1].raw.as_deref() == Some(&json!(5)) && kept[1].id.is_empty()
    );
    assert_eq!(
        hand_made(
            json!([text(json!({"errors": [null]}))]),
            &ParserArgs::default()
        )
        .unwrap_err()
        .to_string(),
        "Validation error: TypeError: Cannot read properties of null (reading 'location')"
    );
}
