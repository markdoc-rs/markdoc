//! Upstream's `src/grammar/tag.test.ts`, ported (the `toDeepEqual` checks compare the results'
//! JSON, as the JavaScript objects compare), then cases for what those tests do not reach:
//! `buildMessage` for every expectation kind, numbers as JavaScript writes them, the object
//! semantics of hash literals and function parameters, and errors at astral characters.
//! Expected values that are not upstream's own were printed by node from upstream's tag.js.

use serde_json::json;

use super::json::number_to_string;
use super::tag::build_message;
use super::*;

fn parse_json(content: &str) -> serde_json::Value {
    parse(content)
        .unwrap_or_else(|e| panic!("{content:?} should parse: {e}"))
        .to_json()
}

fn attributes(content: &str) -> serde_json::Value {
    parse_json(content)["meta"]["attributes"].clone()
}

fn assert_syntax_error(content: &str) {
    assert!(
        matches!(parse(content), Err(ParseError::Syntax(_))),
        "{content:?} should throw a SyntaxError"
    );
}

/// The SyntaxError `parse(content)` throws.
fn syntax_error(content: &str) -> SyntaxError {
    match parse(content) {
        Err(ParseError::Syntax(error)) => error,
        other => panic!("{content:?} should throw a SyntaxError, not give {other:?}"),
    }
}

fn variable(path: serde_json::Value) -> serde_json::Value {
    json!({"$$mdtype": "Variable", "path": path})
}

// describe('Markdoc tag parser') / describe('tag parsing')

#[test]
fn with_a_simple_opening_tag() {
    assert_eq!(
        parse_json("foo"),
        json!({"type": "tag_open", "nesting": 1, "meta": {"tag": "foo", "attributes": null}})
    );
}

#[test]
fn with_an_opening_tag_that_has_attributes() {
    assert_eq!(
        parse_json("foo foo=1 bar=true"),
        json!({
            "type": "tag_open",
            "nesting": 1,
            "meta": {
                "tag": "foo",
                "attributes": [
                    {"type": "attribute", "name": "foo", "value": 1},
                    {"type": "attribute", "name": "bar", "value": true},
                ],
            },
        })
    );
}

#[test]
fn with_a_self_closing_tag() {
    assert_eq!(
        parse_json("foo /"),
        json!({"type": "tag", "nesting": 0, "meta": {"tag": "foo", "attributes": null}})
    );
}

#[test]
fn with_a_self_closing_tag_that_has_attributes() {
    assert_eq!(
        parse_json("foo foo=1 bar=true /"),
        json!({
            "type": "tag",
            "nesting": 0,
            "meta": {
                "tag": "foo",
                "attributes": [
                    {"type": "attribute", "name": "foo", "value": 1},
                    {"type": "attribute", "name": "bar", "value": true},
                ],
            },
        })
    );
}

#[test]
fn with_a_closing_tag() {
    assert_eq!(
        parse_json("/foo"),
        json!({"type": "tag_close", "nesting": -1, "meta": {"tag": "foo"}})
    );
}

#[test]
fn with_an_invalid_closing_tag() {
    assert_syntax_error("/foo/");
}

#[test]
fn with_an_invalid_closing_tag_that_has_attributes() {
    assert_syntax_error("/foo test=1");
}

// describe('variable parsing')

#[test]
fn variable_with_a_simple_variable() {
    assert_eq!(
        parse_json("$foo"),
        json!({"type": "variable", "meta": {"variable": variable(json!(["foo"]))}})
    );
}

#[test]
fn variable_with_multiple_levels_of_depth() {
    assert_eq!(
        parse_json("$foo.bar.baz"),
        json!({"type": "variable", "meta": {"variable": variable(json!(["foo", "bar", "baz"]))}})
    );
}

#[test]
fn variable_with_an_array_index() {
    assert_eq!(
        parse_json("$foo[1]"),
        json!({"type": "variable", "meta": {"variable": variable(json!(["foo", 1]))}})
    );
}

#[test]
fn variable_with_multiple_array_indexes() {
    assert_eq!(
        parse_json("$foo[1][2]"),
        json!({"type": "variable", "meta": {"variable": variable(json!(["foo", 1, 2]))}})
    );
}

#[test]
fn variable_with_array_indexes_and_properties() {
    assert_eq!(
        parse_json("$foo[1].bar.baz[2].test"),
        json!({
            "type": "variable",
            "meta": {"variable": variable(json!(["foo", 1, "bar", "baz", 2, "test"]))},
        })
    );
}

#[test]
fn variable_with_an_invalid_array_index() {
    assert_syntax_error("$foo[asdf]");
}

#[test]
fn variable_with_an_invalid_namespace() {
    assert_syntax_error("$.foo:bar.baz");
}

// describe('parsing attributes')

#[test]
fn parsing_annotation_with_a_single_attribute() {
    assert_eq!(
        attributes("test=1"),
        json!([{"type": "attribute", "name": "test", "value": 1}])
    );
}

#[test]
fn with_an_id() {
    assert_eq!(
        attributes("#test"),
        json!([{"type": "attribute", "name": "id", "value": "test"}])
    );
}

#[test]
fn with_hyphens() {
    assert_eq!(
        attributes("#test-1 .foo-bar"),
        json!([
            {"type": "attribute", "name": "id", "value": "test-1"},
            {"type": "class", "name": "foo-bar", "value": true},
        ])
    );
}

#[test]
fn with_chained_classes() {
    assert_eq!(
        attributes(".foo .bar"),
        json!([
            {"type": "class", "name": "foo", "value": true},
            {"type": "class", "name": "bar", "value": true},
        ])
    );
}

#[test]
fn with_chained_id_and_classes() {
    assert_eq!(
        attributes("#test-1 .foo .bar"),
        json!([
            {"type": "attribute", "name": "id", "value": "test-1"},
            {"type": "class", "name": "foo", "value": true},
            {"type": "class", "name": "bar", "value": true},
        ])
    );
}

#[test]
fn with_an_invalid_id() {
    assert_syntax_error("#foo@bar.baz@test");
}

#[test]
fn with_key_value_pairs() {
    assert_eq!(
        attributes("foo=\"bar\" baz=3 test=true"),
        json!([
            {"type": "attribute", "name": "foo", "value": "bar"},
            {"type": "attribute", "name": "baz", "value": 3},
            {"type": "attribute", "name": "test", "value": true},
        ])
    );
}

#[test]
fn with_shortcuts_and_key_value_pairs() {
    assert_eq!(
        attributes("#foo .bar test=\"asdf\""),
        json!([
            {"type": "attribute", "name": "id", "value": "foo"},
            {"type": "class", "name": "bar", "value": true},
            {"type": "attribute", "name": "test", "value": "asdf"},
        ])
    );
}

#[test]
fn with_boolean_key_value_pairs() {
    assert_eq!(
        attributes("test=true foo=false bar=true"),
        json!([
            {"type": "attribute", "name": "test", "value": true},
            {"type": "attribute", "name": "foo", "value": false},
            {"type": "attribute", "name": "bar", "value": true},
        ])
    );
}

#[test]
fn with_null_key_value_pair() {
    assert_eq!(
        attributes("foo=null"),
        json!([{"type": "attribute", "name": "foo", "value": null}])
    );
}

// describe('with variables as values')

#[test]
fn value_with_a_simple_variable() {
    assert_eq!(
        attributes("test=$foo"),
        json!([{"type": "attribute", "name": "test", "value": variable(json!(["foo"]))}])
    );
}

#[test]
fn value_with_multiple_levels_of_depth() {
    assert_eq!(
        attributes("test=$foo.bar.baz"),
        json!([{"type": "attribute", "name": "test", "value": variable(json!(["foo", "bar", "baz"]))}])
    );
}

#[test]
fn value_with_an_array_index() {
    assert_eq!(
        attributes("test=$foo[1]"),
        json!([{"type": "attribute", "name": "test", "value": variable(json!(["foo", 1]))}])
    );
}

#[test]
fn value_with_multiple_array_indexes() {
    assert_eq!(
        attributes("test=$foo[1][2]"),
        json!([{"type": "attribute", "name": "test", "value": variable(json!(["foo", 1, 2]))}])
    );
}

#[test]
fn value_with_array_indexes_and_properties() {
    assert_eq!(
        attributes("test=$foo[1].bar.baz[2].test"),
        json!([{
            "type": "attribute",
            "name": "test",
            "value": variable(json!(["foo", 1, "bar", "baz", 2, "test"])),
        }])
    );
}

#[test]
fn value_with_an_invalid_array_index() {
    assert_syntax_error("test=$foo[asdf]");
}

// describe('with complex values')

#[test]
fn with_a_simple_hash_literal_value() {
    assert_eq!(
        attributes("foo={bar: true}"),
        json!([{"type": "attribute", "name": "foo", "value": {"bar": true}}])
    );
}

#[test]
fn with_a_nested_hash_literal_value() {
    let expected = json!([{
        "type": "attribute",
        "name": "foo",
        "value": {"bar": true, "baz": {"test": "this is a test"}},
    }]);
    assert_eq!(
        attributes("foo={bar: true, baz: {test: \"this is a test\"}}"),
        expected
    );
    assert_eq!(
        attributes("foo={bar:true,baz:{test:\"this is a test\"}}"),
        expected
    );
}

#[test]
fn with_a_hash_literal_that_has_string_keys() {
    assert_eq!(
        attributes("foo={bar: true, \"baz\": 1}"),
        json!([{"type": "attribute", "name": "foo", "value": {"bar": true, "baz": 1}}])
    );
}

#[test]
fn with_multiple_hash_literal_values() {
    assert_eq!(
        attributes("foo={bar: true} baz={test: \"testing\"}"),
        json!([
            {"type": "attribute", "name": "foo", "value": {"bar": true}},
            {"type": "attribute", "name": "baz", "value": {"test": "testing"}},
        ])
    );
}

#[test]
fn with_an_array_literal_value() {
    let expected = json!([{"type": "attribute", "name": "foo", "value": [1, 2, 3]}]);
    assert_eq!(attributes("foo=[1, 2, 3]"), expected);
    assert_eq!(attributes("foo=[1,2,3]"), expected);
}

#[test]
fn with_nested_array_literal_values() {
    let expected =
        json!([{"type": "attribute", "name": "foo", "value": [1, 2, ["test", true, null]]}]);
    assert_eq!(attributes("foo=[1, 2, [\"test\", true, null]]"), expected);
    assert_eq!(attributes("foo=[1,2,[\"test\",true,null]]"), expected);
}

#[test]
fn with_multiple_nested_array_literal_values() {
    assert_eq!(
        attributes("foo=[1, 2, [\"test\", true, null]] bar=[\"baz\"]"),
        json!([
            {"type": "attribute", "name": "foo", "value": [1, 2, ["test", true, null]]},
            {"type": "attribute", "name": "bar", "value": ["baz"]},
        ])
    );
}

#[test]
fn with_array_and_object_literals() {
    assert_eq!(
        attributes("foo=[1, 2, {bar: \"baz\", test: [1, 2, 3]}]"),
        json!([{
            "type": "attribute",
            "name": "foo",
            "value": [1, 2, {"bar": "baz", "test": [1, 2, 3]}],
        }])
    );
}

#[test]
fn with_an_invalid_value() {
    for example in ["foo=bar", "foo=a1", "foo=1a"] {
        assert_syntax_error(example);
    }
}

// --- beyond tag.test.ts ---------------------------------------------------------------------

/// `JSON.stringify(parse(content))`, or of the error's four fields, as the oracle prints them.
fn line(content: &str) -> String {
    match parse(content) {
        Ok(result) => result.to_json_string(),
        Err(ParseError::Syntax(error)) => {
            let mut out = String::from("{");
            error.write_fields_json(&mut out);
            out.push('}');
            out
        }
        Err(ParseError::Range(error)) => panic!("{content:?}: {error}"),
    }
}

#[test]
fn errors_carry_message_expected_found_and_location() {
    assert_eq!(
        line("foo bar"),
        r#"{"message":"Expected \"(\" or \"=\" but end of input found.","expected":[{"type":"literal","text":"(","ignoreCase":false},{"type":"literal","text":"=","ignoreCase":false}],"found":null,"location":{"start":{"offset":7,"line":1,"column":8},"end":{"offset":7,"line":1,"column":8}}}"#
    );
    assert_eq!(
        line("/foo/"),
        r#"{"message":"Expected end of input but \"/\" found.","expected":[{"type":"end"}],"found":"/","location":{"start":{"offset":4,"line":1,"column":5},"end":{"offset":5,"line":1,"column":6}}}"#
    );
    assert_eq!(
        line(""),
        r#"{"message":"Expected \"/\", class, id, identifier, tag name, or variable but end of input found.","expected":[{"type":"other","description":"variable"},{"type":"other","description":"identifier"},{"type":"other","description":"id"},{"type":"other","description":"class"},{"type":"other","description":"identifier"},{"type":"other","description":"tag name"},{"type":"literal","text":"/","ignoreCase":false}],"found":null,"location":{"start":{"offset":0,"line":1,"column":1},"end":{"offset":0,"line":1,"column":1}}}"#
    );
    // Lines start after "\n" only; columns count UTF-16 code units.
    let error = syntax_error("foo\n\n$");
    assert_eq!(
        error.location.start,
        Position {
            offset: 5,
            line: 3,
            column: 1
        }
    );
    assert_eq!(
        error.location.end,
        Position {
            offset: 6,
            line: 3,
            column: 2
        }
    );
    assert_eq!(error.found, Some(JsString::from("$")));
}

#[test]
fn found_is_one_code_unit_even_of_an_astral_character() {
    let error = syntax_error("foo 😀");
    assert_eq!(error.found, Some(JsString(vec![0xD83D])));
    assert_eq!(error.location.start.offset, 4);
    assert_eq!(error.location.end.offset, 5);
    assert!(
        line("foo 😀").contains(r#"or whitespace but \"\ud83d\" found.","#),
        "JSON.stringify escapes the lone surrogate"
    );
    let json = error.to_json();
    assert_eq!(json["found"], json!("\u{FFFD}"));
    assert_eq!(json["name"], json!("SyntaxError"));
    assert!(
        error
            .to_json_string()
            .ends_with(r#","name":"SyntaxError"}"#)
    );
}

#[test]
fn a_falsy_primary_value_is_dropped() {
    for content in ["foo 0", "foo -0", "foo \"\"", "foo false", "foo null"] {
        assert_eq!(
            line(content),
            r#"{"type":"tag_open","nesting":1,"meta":{"tag":"foo","attributes":null}}"#,
            "{content}"
        );
    }
    assert_eq!(
        line("foo 1 x=2"),
        r#"{"type":"tag_open","nesting":1,"meta":{"tag":"foo","attributes":[{"type":"attribute","name":"primary","value":1},{"type":"attribute","name":"x","value":2}]}}"#
    );
}

#[test]
fn at_variables_are_plain_arrays() {
    assert_eq!(
        line(r#"@x.y[0]["z"]"#),
        r#"{"type":"variable","meta":{"variable":["x","y",0,"z"]}}"#
    );
}

#[test]
fn hash_keys_follow_javascript_property_order() {
    let cases = [
        (
            "foo x={b: 1, 2: 2, a: 3, 1: 4}",
            r#"{"1":4,"2":2,"b":1,"a":3}"#,
        ),
        (
            r#"foo x={4294967294: 1, 4294967295: 2, a: 3, 01: 4, "-0": 5, 0: 6}"#,
            r#"{"0":6,"4294967294":1,"4294967295":2,"a":3,"01":4,"-0":5}"#,
        ),
        (r#"foo x={"$$mdtype": 1, a: 2}"#, r#"{"a":2}"#),
        (r#"foo x={"": 1}"#, r#"{"":1}"#),
        ("foo x={a: 1, a: 2, b: 3}", r#"{"a":2,"b":3}"#),
    ];
    for (content, expected) in cases {
        let parsed = parse(content).unwrap();
        let value = &parsed.meta.attributes.as_ref().unwrap().as_ref().unwrap()[0].value;
        assert_eq!(value.to_json_string(), expected, "{content}");
    }
}

#[test]
fn proto_keys_behave_as_in_javascript() {
    let cases = [
        // the head item's computed key is an own property; later ones go through [[Set]]
        ("foo x={a: 1, __proto__: 2}", r#"{"a":1}"#),
        ("foo x={__proto__: 1, a: 2}", r#"{"__proto__":1,"a":2}"#),
        (
            "foo x={__proto__: 1, a: 2, __proto__: 3}",
            r#"{"__proto__":3,"a":2}"#,
        ),
        (
            "foo x={a: 1, __proto__: null, __proto__: 5}",
            r#"{"a":1,"__proto__":5}"#,
        ),
        (
            "foo x={x: 1, __proto__: {a: 1, __proto__: null}, __proto__: 3}",
            r#"{"x":1,"__proto__":3}"#,
        ),
        (
            "foo x={x: 1, __proto__: {__proto__: 1}, __proto__: 3}",
            r#"{"x":1,"__proto__":3}"#,
        ),
        (
            "foo x={a: 1, __proto__: [1,2], length: 3}",
            r#"{"a":1,"length":3}"#,
        ),
        (
            "foo x=f(__proto__=1)",
            r#"{"$$mdtype":"Function","name":"f","parameters":{}}"#,
        ),
        (
            "foo x=f(__proto__=null, __proto__=1)",
            r#"{"$$mdtype":"Function","name":"f","parameters":{"__proto__":1}}"#,
        ),
        (
            "foo x=f(a=1, 2)",
            r#"{"$$mdtype":"Function","name":"f","parameters":{"1":2,"a":1}}"#,
        ),
        (
            "foo x=f(1, 0=5)",
            r#"{"$$mdtype":"Function","name":"f","parameters":{"0":5}}"#,
        ),
        (
            "foo x=f(, 1)",
            r#"{"$$mdtype":"Function","name":"f","parameters":{}}"#,
        ),
    ];
    for (content, expected) in cases {
        let parsed = parse(content).unwrap();
        let value = &parsed.meta.attributes.as_ref().unwrap().as_ref().unwrap()[0].value;
        assert_eq!(value.to_json_string(), expected, "{content}");
    }
    let parsed = parse("foo x={a: 1, __proto__: [1,2]}").unwrap();
    let Value::Object(object) = &parsed.meta.attributes.unwrap().unwrap()[0].value else {
        panic!("a hash literal is an object");
    };
    assert_eq!(
        object.prototype(),
        &Prototype::Value(Box::new(Value::Array(vec![
            Value::Number(1.0),
            Value::Number(2.0)
        ])))
    );
}

// The halfway cases are written as the doubles' exact values on purpose: the shorter spelling
// clippy suggests is what Rust's `{:e}` prints for them, the very rounding under test.
#[allow(clippy::excessive_precision)]
#[test]
fn numbers_are_written_as_javascript_writes_them() {
    let cases: [(f64, &str); 35] = [
        (0.0, "0"),
        (-0.0, "0"),
        (1.0, "1"),
        (-1.0, "-1"),
        (0.1, "0.1"),
        (0.5, "0.5"),
        (100.0, "100"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (1.5e-7, "1.5e-7"),
        (123456789012345680000.0, "123456789012345680000"),
        (2f64.powi(60), "1152921504606847000"),
        (2f64.powi(53), "9007199254740992"),
        (2f64.powi(53) + 2.0, "9007199254740994"),
        (5e-324, "5e-324"),
        (f64::MAX, "1.7976931348623157e+308"),
        // exactly halfway between two shortest candidates: the even one
        (9041039839299.8125, "9041039839299.812"),
        (-88644776111884.125, "-88644776111884.12"),
        (2158187080458500.25, "2158187080458500.2"),
        (0.000001, "0.000001"),
        (0.0000015, "0.0000015"),
        (1e20, "100000000000000000000"),
        (1.25e20, "125000000000000000000"),
        (4.35, "4.35"),
        (0.1 + 0.2, "0.30000000000000004"),
        (1.0 / 3.0, "0.3333333333333333"),
        (123.456, "123.456"),
        (1e16, "10000000000000000"),
        (12345678901234567890.0, "12345678901234567000"),
        (1.2e-300, "1.2e-300"),
        (9.5, "9.5"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (f64::NAN, "NaN"),
        (1e-6, "0.000001"),
    ];
    for (x, expected) in cases {
        assert_eq!(number_to_string(x), expected, "{x:e}");
    }
    // JSON.stringify writes null for what is not finite (an overlong literal is Infinity)
    let content = format!("foo x=1{}", "0".repeat(400));
    assert!(line(&content).contains(r#""value":null"#));
}

#[test]
fn a_tie_takes_the_even_digit_only_if_it_reads_back() {
    // 2^-24 is exactly halfway between 5.960464477539062e-8 and ...063, but the double below
    // a power of two is half as far away as the one above, so ...062 reads back as
    // 0x1.fffffffffffffp-25 and only ...063 is a candidate. node: String(2 ** -24)
    let x = 2f64.powi(-24);
    assert_eq!(number_to_string(x), "5.960464477539063e-8");
    assert_eq!(number_to_string(-x), "-5.960464477539063e-8");
    // node: JSON.stringify(parse(content, {Variable, Function})) of upstream's tag.js
    for (content, value) in [
        ("tag a=0.000000059604644775390625", "5.960464477539063e-8"),
        ("tag a=-0.000000059604644775390625", "-5.960464477539063e-8"),
        ("tag a=0.00000005960464477539063", "5.960464477539063e-8"),
        ("tag a=0.000000059604644775390624", "5.960464477539063e-8"),
        ("tag a=9041039839299.8125", "9041039839299.812"),
    ] {
        assert_eq!(
            line(content),
            format!(
                r#"{{"type":"tag_open","nesting":1,"meta":{{"tag":"tag","attributes":[{{"type":"attribute","name":"a","value":{value}}}]}}}}"#
            ),
            "{content}"
        );
    }
    assert_eq!(
        line("tag 0.000000059604644775390625"),
        r#"{"type":"tag_open","nesting":1,"meta":{"tag":"tag","attributes":[{"type":"attribute","name":"primary","value":5.960464477539063e-8}]}}"#
    );
    assert_eq!(
        line("tag x=$a[0.000000059604644775390625]"),
        r#"{"type":"tag_open","nesting":1,"meta":{"tag":"tag","attributes":[{"type":"attribute","name":"x","value":{"$$mdtype":"Variable","path":["a",5.960464477539063e-8]}}]}}"#
    );
    // What V8 writes always reads back as the number: check that for every power of two,
    // the one family where a tie's even candidate can fall outside the rounding interval.
    for e in -1074i64..=1023 {
        // 2^e built from its bits (powi is not exact for subnormals)
        let p = if e >= -1022 {
            f64::from_bits(((e + 1023) as u64) << 52)
        } else {
            f64::from_bits(1u64 << (e + 1074))
        };
        for x in [p, -p] {
            let text = number_to_string(x);
            assert_eq!(text.parse::<f64>(), Ok(x), "2^{e}: {text}");
        }
    }
}

#[test]
fn to_json_is_json_parse_of_the_json_text() {
    for content in [
        "foo x=1e5",
        "if $a.b[0][\"c d\"] .x #y z=fn(1, a=$b, \"q\") /",
        "foo x=[1152921504606846976, 100000000000000000000, 0.000001, 1000000000000000000000, -0]",
        "foo x={b: 1, 2: 2, a: {__proto__: 1, c: [null, true, \"\\n\\t\\\"\"]}}",
        "#id .class key=\"é😀\"",
        "/foo",
        "@a.b",
        "",
    ] {
        let (text, json) = match parse(content) {
            Ok(r) => (r.to_json_string(), r.to_json()),
            Err(e) => {
                let e = e.as_syntax().expect("a SyntaxError");
                (e.to_json_string(), e.to_json())
            }
        };
        let reparsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json, reparsed, "{content}");
    }
}

#[allow(clippy::excessive_precision)]
#[test]
fn json_text_reads_back_as_the_same_numbers() {
    // serde_json reads a number's shortest text back as that number (its `float_roundtrip`
    // feature, which this crate enables); without it 2^-24 and about 30% of random doubles
    // came back one unit in the last place off.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut doubles = vec![2f64.powi(-24), -(2f64.powi(-24)), 9041039839299.8125];
    while doubles.len() < 20_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let x = f64::from_bits(state);
        if x.is_finite() {
            doubles.push(x);
        }
    }
    for x in doubles {
        let text = number_to_string(x);
        let back: f64 = serde_json::from_str(&text).unwrap();
        assert_eq!(back.to_bits(), x.to_bits(), "{text}");
    }
}

#[test]
fn build_message_describes_every_expectation_kind() {
    static PARTS: [ClassPart; 14] = [
        ClassPart::Char("a"),
        ClassPart::Range("0", "9"),
        ClassPart::Char("-"),
        ClassPart::Char("]"),
        ClassPart::Char("^"),
        ClassPart::Char("\\"),
        ClassPart::Char("\0"),
        ClassPart::Char("\x1f"),
        ClassPart::Char("\x7f"),
        ClassPart::Char("\t"),
        ClassPart::Char("\n"),
        ClassPart::Char("\r"),
        ClassPart::Char("\x0b"),
        ClassPart::Char("\u{9f}"),
    ];
    static AZ: [ClassPart; 1] = [ClassPart::Range("a", "z")];
    let message = |expected: &[Expectation], found: Option<JsString>| {
        build_message(expected, found.as_ref()).to_string_lossy()
    };
    assert_eq!(
        message(
            &[Expectation::Class {
                parts: &PARTS,
                inverted: true,
                ignore_case: false
            }],
            Some(JsString::from("\0"))
        ),
        r#"Expected [^a,0-9,\-,\],\^,\\,\0,\x1F,\x7F,\t,\n,\r,\x0B,\x9F] but "\0" found."#
    );
    assert_eq!(
        message(
            &[
                Expectation::Class {
                    parts: &AZ,
                    inverted: false,
                    ignore_case: false
                },
                Expectation::Any,
                Expectation::Not(Box::new(Expectation::Literal {
                    text: "x",
                    ignore_case: false
                })),
                Expectation::End,
            ],
            None
        ),
        r#"Expected [a-z], any character, end of input, or not "x" but end of input found."#
    );
    assert_eq!(
        message(
            &[Expectation::Literal {
                text: "\"\\\0\t\n\r\x01\x0f\x10\x1f\x7f\u{80}\u{9f}\u{a0}",
                ignore_case: false
            }],
            Some(JsString::from("\x7f"))
        ),
        "Expected \"\\\"\\\\\\0\\t\\n\\r\\x01\\x0F\\x10\\x1F\\x7F\\x80\\x9F\u{a0}\" but \"\\x7F\" found."
    );
    // No expectations at all: `descriptions[-1]` is undefined.
    assert_eq!(
        message(&[], Some(JsString::from("a"))),
        r#"Expected , or undefined but "a" found."#
    );
    // Sorted and deduplicated, code unit by code unit (U+FFFF sorts after a surrogate pair).
    assert_eq!(
        message(
            &[
                Expectation::Other { description: "z" },
                Expectation::Other { description: "é" },
                Expectation::Other {
                    description: "\u{ffff}"
                },
                Expectation::Other {
                    description: "😀"
                },
                Expectation::Other { description: "z" },
            ],
            Some(JsString(Vec::new()))
        ),
        "Expected z, é, 😀, or \u{ffff} but end of input found."
    );
}

#[test]
fn large_objects_keep_javascript_semantics() {
    // Past INDEXED_FROM properties the object keeps an index; [[Set]] must still overwrite
    // in place, keep creation order, and put array indices first.
    let mut items: Vec<String> = (0..2000).map(|i| format!("k{i}: {i}")).collect();
    items.push("k5: \"again\"".to_string());
    items.push("7: 7".to_string());
    items.push("__proto__: null".to_string());
    items.push("__proto__: 1".to_string());
    let content = format!("foo x={{{}}}", items.join(", "));
    let parsed = parse(&content).unwrap();
    let Value::Object(object) = &parsed.meta.attributes.unwrap().unwrap()[0].value else {
        panic!("a hash literal is an object");
    };
    let keys: Vec<&str> = object.entries().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys.len(), 2002);
    assert_eq!(&keys[..3], ["7", "k0", "k1"]);
    assert_eq!(keys.last(), Some(&"__proto__"));
    assert_eq!(object.get("k5"), Some(&Value::String("again".to_string())));
    assert_eq!(object.get("__proto__"), Some(&Value::Number(1.0)));
    assert_eq!(object.prototype(), &Prototype::Null);

    let params: Vec<String> = (0..2000).map(|i| format!("p{i}={i}")).collect();
    let content = format!("foo x=f({}, p3=\"last\", 0)", params.join(", "));
    let parsed = parse(&content).unwrap();
    let Value::Function(function) = &parsed.meta.attributes.unwrap().unwrap()[0].value else {
        panic!("a call is a function");
    };
    assert_eq!(function.parameters.len(), 2001);
    assert_eq!(
        function.parameters.entries()[0],
        ("2001", &Value::Number(0.0))
    );
    assert_eq!(
        function.parameters.get("p3"),
        Some(&Value::String("last".to_string()))
    );
}

/// `tag x=` with values nested `levels` deep: `open` around `core` and as many `close`, the
/// core `core_levels` values deep itself (the attribute's value is one level).
fn nested(open: &str, core: &str, close: &str, core_levels: usize, levels: usize) -> String {
    let d = levels - core_levels;
    format!("tag x={}{core}{} /", open.repeat(d), close.repeat(d))
}

/// Every construct of tag.pegjs that nests values, as `(open, core, close, core_levels)`; the
/// last ones fail at the innermost level, so the parser backtracks through every level.
const NESTING: [(&str, &str, &str, usize); 13] = [
    ("[", "1", "]", 1),
    ("[1, ", "1", ", 2]", 1),
    ("{a: ", "1", "}", 1),
    ("{a: 1, b: ", "1", ", c: 2}", 1),
    ("{\"k\": ", "1", "}", 1),
    ("f(", "1", ")", 1),
    ("f(a=", "1", ")", 1),
    ("f(1, ", "1", ")", 1),
    ("f(1, a=", "1", ")", 1),
    ("{a: 1, __proto__: ", "{b: 1}", "}", 2),
    ("[", "x", "]", 1),
    ("f(1, ", "x", ")", 1),
    ("{a: 1, b: ", "x", ", c: 2}", 1),
];

/// What a level of `open` costs on the depth budget, and what the core adds.
fn level_costs(open: &str, core: &str) -> (u64, u64) {
    use super::tag::cost::*;
    let level = match open {
        "[" => ARRAY,
        "[1, " => ARRAY + ARRAY_TAIL,
        "{a: " | "{\"k\": " => HASH,
        "{a: 1, b: " | "{a: 1, __proto__: " => HASH + HASH_TAIL,
        "f(" | "f(a=" => CALL,
        "f(1, " | "f(1, a=" => CALL + PARAMETER_TAIL,
        other => panic!("no cost for {other}"),
    };
    (level, if core == "{b: 1}" { HASH } else { 0 })
}

#[test]
fn nesting_deeper_than_the_limit_is_a_range_error() {
    // On a thread with Rust's default stack, 2 MiB, which is what `cargo test` gives each
    // test. Where the stack grows, each construct nests as deep as node's stack takes (the
    // levels' costs on the depth budget, `tag::cost`); where it cannot, MAX_NESTING levels
    // fit a debug build's frames.
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            for (open, core, close, core_levels) in NESTING {
                let levels = if crate::depth::can_grow() {
                    let (level, core_cost) = level_costs(open, core);
                    usize::try_from((crate::depth::STACK - core_cost) / level).unwrap()
                        + core_levels
                } else {
                    MAX_NESTING
                };
                let at_limit = parse(&nested(open, core, close, core_levels, levels));
                if core == "x" {
                    assert!(
                        matches!(at_limit, Err(ParseError::Syntax(_))),
                        "{open}: {at_limit:?}"
                    );
                } else {
                    assert!(at_limit.is_ok(), "{open} {levels}: {at_limit:?}");
                }
                // One level more is what V8 reports, past its own stack, as
                // `RangeError: Maximum call stack size exceeded`.
                let deeper = parse(&nested(open, core, close, core_levels, levels + 1));
                assert_eq!(
                    deeper,
                    Err(ParseError::Range(RangeError(
                        "Maximum call stack size exceeded".to_owned()
                    ))),
                    "{open} {}",
                    levels + 1
                );
            }
            // Far past the limit, unclosed: the error comes at once, not after backtracking
            // through every level (node 22 throws RangeError here too).
            let unclosed = format!("x={}", "[".repeat(100_000));
            assert!(matches!(parse(&unclosed), Err(ParseError::Range(_))));
            let mixed = format!("tag a={}", "f(1, [{a: ".repeat(50_000));
            assert!(matches!(parse(&mixed), Err(ParseError::Range(_))));
        })
        .unwrap()
        .join();
    assert!(outcome.is_ok(), "the parser overflowed or panicked");
}

#[test]
fn a_failure_far_into_a_long_input_keeps_two_positions() {
    // peg$posDetailsCache is sparse: a SyntaxError at the end of a long content computes its
    // line and column from the start without keeping a slot per code unit before it.
    // node: tag.parse('tag' + '\n a=1'.repeat(500000) + '\n ?') throws at offset 2500005,
    // line 500002, column 2.
    let content = format!("tag{}\n ?", "\n a=1".repeat(500_000));
    let error = syntax_error(&content);
    let at = |p: Position| (p.offset, p.line, p.column);
    assert_eq!(at(error.location.start), (2_500_005, 500_002, 2));
    assert_eq!(at(error.location.end), (2_500_006, 500_002, 3));
    assert_eq!(error.found, Some(JsString::from("?")));
}
