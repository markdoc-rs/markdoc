//! The realm against node 22.22.1: every object of the table as node converts it
//! (`table::EXPECTED`, generated with the table), the contract the object model calls
//! ([`get_inherited`], [`prototype_of`]), and every built-in function as upstream's
//! conversions call it (`tests/next/realm/conversions.jsonl`, written by
//! `tools/gen_realm_conversions.cjs` from node).

use super::*;
use crate::next::schema::value::{JsString, json_stringify, own_entries};
use crate::types::JsObject;
use serde_json::Value as Json;

/// `Name: message` of a result, as node's `String(error)`.
fn text(result: Result<JsString, String>) -> Result<String, String> {
    result.map(|s| s.to_string_lossy().into_owned())
}

#[test]
fn every_object_converts_as_in_node() {
    assert_eq!(Builtin::all().count(), table::EXPECTED.len());
    for (b, (type_of, string, json, keys)) in Builtin::all().zip(table::EXPECTED.iter()) {
        let v = Value::Builtin(b);
        assert_eq!(call::type_of(&v), *type_of, "typeof {b:?}");
        assert_eq!(
            text(v.to_js_string()),
            string.map(str::to_string).map_err(str::to_string),
            "String({b:?})"
        );
        assert_eq!(
            json_stringify(&v, ""),
            json.map(|j| j.map(str::to_string)).map_err(str::to_string),
            "JSON.stringify({b:?})"
        );
        let own: Vec<String> = own_entries(&v).into_iter().map(|(k, _)| k).collect();
        assert_eq!(own, *keys, "Object.keys({b:?})");
        assert!(v.to_boolean(), "every object is truthy");
    }
}

#[test]
fn the_object_prototype_lists_agree_with_the_table() {
    let table: Vec<&str> = Builtin::OBJECT_PROTOTYPE.own_keys().collect();
    assert_eq!(table, crate::next::schema::value::OBJECT_PROTOTYPE);
    assert_eq!(table, crate::markdown_it::mdurl::OBJECT_PROTOTYPE_KEYS);
}

#[test]
fn get_inherited_walks_a_built_in_chain() {
    let s = Value::from("abc");
    // node: "abc".trim === String.prototype.trim; ("abc").hasOwnProperty is
    // Object.prototype's; an absent name is undefined.
    assert!(matches!(
        get_inherited(Builtin::STRING_PROTOTYPE, "trim", &s),
        Ok(Value::Builtin(Builtin::STRING_PROTOTYPE_TRIM))
    ));
    assert!(matches!(
        get_inherited(Builtin::STRING_PROTOTYPE, "hasOwnProperty", &s),
        Ok(Value::Builtin(Builtin::OBJECT_PROTOTYPE_HAS_OWN_PROPERTY))
    ));
    assert!(matches!(
        get_inherited(Builtin::STRING_PROTOTYPE, "nothing", &s),
        Ok(Value::Undefined)
    ));
    // The `__proto__` getter answers for the receiver, not for the object it is found on:
    // ("abc").__proto__ is String.prototype, found on Object.prototype.
    assert!(matches!(
        get_inherited(Builtin::OBJECT_PROTOTYPE, "__proto__", &s),
        Ok(Value::Builtin(Builtin::STRING_PROTOTYPE))
    ));
    assert!(matches!(
        get_inherited(
            Builtin::OBJECT_PROTOTYPE,
            "__proto__",
            &Value::array(vec![])
        ),
        Ok(Value::Builtin(Builtin::ARRAY_PROTOTYPE))
    ));
    // node: String.prototype.trim.caller throws; .length is 0, .name "trim".
    let trim = Value::Builtin(Builtin::STRING_PROTOTYPE_TRIM);
    assert_eq!(
        get_inherited(Builtin::FUNCTION_PROTOTYPE, "caller", &trim).err(),
        Some(RESTRICTED.to_string())
    );
    assert!(matches!(
        crate::next::schema::value::get_property(&trim, "length"),
        Ok(Value::Number(n)) if n == 0.0
    ));
    assert!(matches!(
        crate::next::schema::value::get_property(&trim, "name"),
        Ok(Value::String(ref n)) if n == "trim"
    ));
    // node: Iterator.prototype.constructor is a getter that gives Iterator.
    assert!(matches!(
        get_inherited(
            Builtin::ITERATOR_PROTOTYPE,
            "constructor",
            &Value::empty_object()
        ),
        Ok(Value::Builtin(Builtin::ITERATOR))
    ));
    // node: RegExp.prototype.global is undefined for RegExp.prototype, a TypeError for any
    // other object, and RegExp.prototype.flags is ''.
    let proto = Value::Builtin(Builtin::REGEXP_PROTOTYPE);
    assert!(matches!(
        get_inherited(Builtin::REGEXP_PROTOTYPE, "global", &proto),
        Ok(Value::Undefined)
    ));
    assert_eq!(
        get_inherited(Builtin::REGEXP_PROTOTYPE, "global", &Value::empty_object()).err(),
        Some("TypeError: RegExp.prototype.global getter called on non-RegExp object".to_string())
    );
    assert!(matches!(
        get_inherited(Builtin::REGEXP_PROTOTYPE, "flags", &proto),
        Ok(Value::String(ref f)) if f.is_empty()
    ));
}

#[test]
fn intrinsic_names_the_built_in_a_chain_holds() {
    assert_eq!(
        intrinsic(Builtin::ARRAY_PROTOTYPE, "toString"),
        Some(Builtin::ARRAY_PROTOTYPE_TO_STRING)
    );
    // Inherited from Object.prototype.
    assert_eq!(
        intrinsic(Builtin::ARRAY_PROTOTYPE, "valueOf"),
        Some(Builtin::OBJECT_PROTOTYPE_VALUE_OF)
    );
    // A primitive, an accessor, nothing.
    assert_eq!(intrinsic(Builtin::FUNCTION_PROTOTYPE, "length"), None);
    assert_eq!(intrinsic(Builtin::OBJECT_PROTOTYPE, "__proto__"), None);
    assert_eq!(intrinsic(Builtin::OBJECT_PROTOTYPE, "nothing"), None);
}

#[test]
fn every_value_has_its_default_prototype() {
    let cases = [
        (Value::from("s"), Value::Builtin(Builtin::STRING_PROTOTYPE)),
        (
            Value::Number(1.0),
            Value::Builtin(Builtin::NUMBER_PROTOTYPE),
        ),
        (
            Value::Boolean(true),
            Value::Builtin(Builtin::BOOLEAN_PROTOTYPE),
        ),
        (
            Value::array(vec![]),
            Value::Builtin(Builtin::ARRAY_PROTOTYPE),
        ),
        (
            Value::empty_object(),
            Value::Builtin(Builtin::OBJECT_PROTOTYPE),
        ),
        (
            Value::from(crate::next::schema::value::TagValue::new(
                "a",
                JsObject::new(),
                vec![],
            )),
            Value::Builtin(Builtin::TAG_PROTOTYPE),
        ),
        (
            Value::Builtin(Builtin::STRING_PROTOTYPE_TRIM),
            Value::Builtin(Builtin::FUNCTION_PROTOTYPE),
        ),
        (Value::Builtin(Builtin::OBJECT_PROTOTYPE), Value::Null),
    ];
    for (v, proto) in cases {
        assert!(
            crate::next::schema::value::strict_equals(&prototype_of(&v), &proto),
            "{v:?}"
        );
    }
}

fn decode(d: &Json) -> Value {
    match d {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().expect("a number")),
        Json::String(s) => Value::from(s.as_str()),
        Json::Object(o) => {
            if o.contains_key("u") {
                Value::Undefined
            } else if let Some(Json::String(n)) = o.get("n") {
                Value::Number(match n.as_str() {
                    "-0" => -0.0,
                    "NaN" => f64::NAN,
                    "Infinity" => f64::INFINITY,
                    _ => f64::NEG_INFINITY,
                })
            } else if let Some(Json::String(name)) = o.get("b") {
                Value::Builtin(
                    Builtin::all()
                        .find(|b| b.name() == name)
                        .expect("a built-in of the table"),
                )
            } else if let Some(Json::Array(items)) = o.get("a") {
                Value::array(items.iter().map(decode).collect())
            } else if let Some(Json::Array(entries)) = o.get("o") {
                let mut object = JsObject::new();
                for e in entries {
                    object.insert(e[0].as_str().expect("a key").to_string(), decode(&e[1]));
                }
                Value::object(object)
            } else {
                panic!("a desc: {d}")
            }
        }
        Json::Array(_) => panic!("a desc: {d}"),
    }
}

#[test]
fn every_function_called_as_upstream_calls_it() {
    let cases = include_str!("../../../tests/next/realm/conversions.jsonl");
    let mut failures = Vec::new();
    for line in cases.lines() {
        let case: Json = serde_json::from_str(line).expect("a case");
        let value = decode(&case["value"]);
        let got = match case["kind"].as_str() {
            Some("string") => match value.to_js_string() {
                Ok(s) => Json::String(s.to_string_lossy().into_owned()),
                Err(e) => serde_json::json!({ "t": e }),
            },
            _ => {
                let holder = match case.get("key").and_then(Json::as_str) {
                    Some(key) => {
                        let mut o = JsObject::new();
                        o.insert(key.to_string(), value);
                        Value::object(o)
                    }
                    None => value,
                };
                match json_stringify(&holder, "") {
                    Ok(Some(s)) => Json::String(s),
                    Ok(None) => serde_json::json!({ "u": 1 }),
                    Err(e) => serde_json::json!({ "t": e }),
                }
            }
        };
        if got != case["out"] {
            failures.push(format!("{line}\n  port: {got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases.lines().count(),
        failures.join("\n")
    );
}

/// The divergences the module documents, pinned.
#[test]
fn documented_divergences() {
    let holder = |key: &str, f: Builtin| {
        let mut inner = JsObject::new();
        inner.insert("toJSON", Value::Builtin(f));
        let mut o = JsObject::new();
        o.insert(key.to_string(), Value::object(inner));
        Value::object(o)
    };
    // node: JSON.stringify({"a b": {toJSON: Function}}) throws `SyntaxError: Unexpected
    // identifier 'b'` (V8 compiles the key as a body); here every body is a function.
    assert_eq!(
        json_stringify(&holder("a b", Builtin::FUNCTION), "").unwrap(),
        Some("{}".into())
    );
    // node: (1234.5).toLocaleString('de') is "1.234,5"; here every valid locale formats as
    // node's default locale (en-US) does. An invalid one throws in both.
    let mut array_like = JsObject::new();
    array_like.insert("0", Value::Number(1234.5));
    array_like.insert("length", Value::Number(1.0));
    array_like.insert(
        "toJSON",
        Value::Builtin(Builtin::ARRAY_PROTOTYPE_TO_LOCALE_STRING),
    );
    let mut o = JsObject::new();
    o.insert("de", Value::object(array_like.clone()));
    assert_eq!(
        json_stringify(&Value::object(o), "").unwrap(),
        Some(r#"{"de":"1,234.5"}"#.into())
    );
    let mut o = JsObject::new();
    o.insert("a b", Value::object(array_like));
    assert_eq!(
        json_stringify(&Value::object(o), ""),
        Err("RangeError: Incorrect locale information provided".into())
    );
}

/// How much stack a level of nested calls takes, for every function that converts its `this`
/// again (`{toString: f}` recurses until the limit): measured on a big stack, printed with
/// `--nocapture`; [`call::MAX_CALL_DEPTH`] levels of the costliest must fit a 2 MiB stack with
/// a quarter spare.
#[test]
fn the_stack_a_call_costs() {
    let handle = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| {
            let mut worst = (0usize, String::new());
            for f in Builtin::all().filter(|b| b.is_callable()) {
                for key in ["toString", "valueOf", "toJSON"] {
                    let mut o = JsObject::new();
                    o.insert(key, Value::Builtin(f));
                    let holder = Value::object(o);
                    call::STACK_PROBE.with(|p| p.set((0, usize::MAX, 0)));
                    let _ = if key == "toJSON" {
                        json_stringify(&holder, "").map(|_| ())
                    } else {
                        holder.to_js_string().map(|_| ())
                    };
                    let (top, low, deepest) = call::STACK_PROBE.with(|p| p.get());
                    if deepest > 2 {
                        let per_level = (top - low) / (deepest - 1);
                        if per_level > worst.0 {
                            worst = (per_level, format!("{key}: {f:?}"));
                        }
                    }
                }
                // Nested data, each level converting the next (an array-like whose item
                // is the next holder): 60 levels.
                for key in ["toString", "toJSON"] {
                    let mut holder = Value::from("x");
                    for _ in 0..60 {
                        let mut o = JsObject::new();
                        o.insert(key, Value::Builtin(f));
                        o.insert("length", Value::Number(1.0));
                        o.insert("0", holder);
                        holder = Value::object(o);
                    }
                    call::STACK_PROBE.with(|p| p.set((0, usize::MAX, 0)));
                    let _ = if key == "toJSON" {
                        json_stringify(&holder, "").map(|_| ())
                    } else {
                        holder.to_js_string().map(|_| ())
                    };
                    let (top, low, deepest) = call::STACK_PROBE.with(|p| p.get());
                    if deepest > 2 {
                        let per_level = (top - low) / (deepest - 1);
                        if per_level > worst.0 {
                            worst = (per_level, format!("nested {key}: {f:?}"));
                        }
                    }
                }
            }
            worst
        })
        .expect("a thread");
    let (per_level, which) = handle.join().expect("no panic");
    println!("costliest level: {per_level} bytes ({which})");
    assert!(
        per_level * call::MAX_CALL_DEPTH <= 3 * (2 << 20) / 4,
        "{} levels of {per_level} bytes ({which}) do not fit 1.5 MiB",
        call::MAX_CALL_DEPTH
    );
}

/// Runs `f` on a thread with a 2 MiB stack (the robustness policy's).
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .expect("a thread")
        .join()
        .expect("no panic")
}

/// node: each throws `RangeError: Maximum call stack size exceeded`; so does the port, past
/// [`call::MAX_CALL_DEPTH`] nested calls, before a 2 MiB stack runs out (debug and release).
#[test]
fn recursion_through_calls_throws_before_the_stack_runs_out() {
    let range = Err(crate::next::schema::value::RANGE_ERROR.to_string());
    let results = on_small_stack(|| {
        let holder = |entries: &[(&str, Builtin)]| {
            let mut o = JsObject::new();
            for (k, f) in entries {
                o.insert(*k, Value::Builtin(*f));
            }
            Value::object(o)
        };
        vec![
            text(holder(&[("toString", Builtin::STRING_PROTOTYPE_TRIM)]).to_js_string()),
            text(holder(&[("toString", Builtin::STRING_PROTOTYPE_ANCHOR)]).to_js_string()),
            text(
                holder(&[("toString", Builtin::OBJECT_PROTOTYPE_TO_LOCALE_STRING)]).to_js_string(),
            ),
            json_stringify(
                &holder(&[
                    ("toJSON", Builtin::STRING_PROTOTYPE_TRIM),
                    ("toString", Builtin::STRING_PROTOTYPE_TRIM),
                ]),
                "",
            )
            .map(|s| s.unwrap_or_default()),
        ]
    });
    for r in results {
        assert_eq!(r, range);
    }
    // Nested objects whose `toString` is `Array.prototype.join` are joined as nested arrays
    // are (the object model's join, a loop to node's 3,301 levels): no call nests, and node
    // converts them too.
    let joined = on_small_stack(|| {
        let mut nested = Value::from("x");
        for _ in 0..(call::MAX_CALL_DEPTH + 50) {
            let mut o = JsObject::new();
            o.insert("toString", Value::Builtin(Builtin::ARRAY_PROTOTYPE_JOIN));
            o.insert("length", Value::Number(1.0));
            o.insert("0", nested);
            nested = Value::object(o);
        }
        text(nested.to_js_string())
    });
    assert_eq!(joined, Ok("x".to_string()));
}

/// A conversion that recurses deep in a transform (a `debug()` at the bottom of 500 nested
/// partials, a transform 1,000 levels deep) throws `RangeError` before the stack the
/// transform and the calls share runs out; node throws the same.
#[test]
fn calls_deep_in_a_transform_fit_the_stack() {
    let result = on_small_stack(|| {
        use crate::next::schema::{Config, Partial};
        use crate::next::{index, parser::ParserArgs};
        let parse = |src: &str| index::parse(src, &ParserArgs::default()).expect("parses");
        let mut config =
            Config::from_json(&serde_json::json!({"variables": {"s": "abc"}})).expect("a config");
        let depth = 500;
        let mut partials = indexmap::IndexMap::new();
        for i in 0..depth {
            let src = if i + 1 < depth {
                format!("{{% partial file=\"p{}\" /%}}\n", i + 1)
            } else {
                "{% debug({toJSON: $s.trim, toString: $s.trim}) %}\n".to_string()
            };
            partials.insert(
                format!("p{i}"),
                Partial::Node(std::sync::Arc::new(parse(&src))),
            );
        }
        config.partials = Some(partials);
        let ast = parse("{% partial file=\"p0\" /%}\n");
        index::transform(&ast, &config)
            .map(|_| ())
            .map_err(|e| index::thrown(&e).to_owned())
    });
    assert_eq!(
        result,
        Err(crate::next::schema::value::RANGE_ERROR.to_string())
    );
}
