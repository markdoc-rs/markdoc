use super::*;
use crate::types::js_insert;

fn obj(entries: &[(&str, Value)]) -> Value {
    let mut m = IndexMap::new();
    for (k, v) in entries {
        js_insert(&mut m, *k, v.clone());
    }
    Value::object(m)
}

fn stringify(value: &Value, gap: &str) -> Option<String> {
    json_stringify(value, gap).expect("not too deep")
}

#[test]
fn to_string_is_js_string() {
    // node: String([1, [2, null], undefined, {}, 'x', true, 1e21, -0])
    let v = Value::array(vec![
        Value::Number(1.0),
        Value::array(vec![Value::Number(2.0), Value::Null]),
        Value::Undefined,
        Value::empty_object(),
        Value::from("x"),
        Value::Boolean(true),
        Value::Number(1e21),
        Value::Number(-0.0),
    ]);
    assert_eq!(
        v.to_js_string().unwrap(),
        "1,2,,,[object Object],x,true,1e+21,0"
    );
    assert_eq!(Value::Undefined.to_js_string().unwrap(), "undefined");
    // node: String({toString: 1}) throws a TypeError, String({valueOf: 1}) does not.
    assert!(
        obj(&[("toString", Value::Number(1.0))])
            .to_js_string()
            .is_err()
    );
    assert!(
        Value::array(vec![obj(&[("toString", Value::Null)])])
            .to_js_string()
            .is_err()
    );
    assert_eq!(
        obj(&[("valueOf", Value::Number(1.0))])
            .to_js_string()
            .unwrap(),
        "[object Object]"
    );
}

#[test]
fn json_stringify_matches_js() {
    // node: JSON.stringify({b: [1, undefined, {}], a: undefined, c: 'q"\n'}, null, 2)
    let v = obj(&[
        (
            "b",
            Value::array(vec![
                Value::Number(1.0),
                Value::Undefined,
                Value::empty_object(),
            ]),
        ),
        ("a", Value::Undefined),
        ("c", Value::from("q\"\n")),
    ]);
    assert_eq!(
        stringify(&v, "  ").unwrap(),
        "{\n  \"b\": [\n    1,\n    null,\n    {}\n  ],\n  \"c\": \"q\\\"\\n\"\n}"
    );
    assert_eq!(
        stringify(&v, "").unwrap(),
        r#"{"b":[1,null,{}],"c":"q\"\n"}"#
    );
    assert_eq!(stringify(&Value::Undefined, "  "), None);
    assert_eq!(stringify(&Value::array(vec![]), "  ").unwrap(), "[]");
    assert_eq!(
        stringify(&obj(&[("x", Value::Undefined)]), "  ").unwrap(),
        "{}"
    );
    assert_eq!(stringify(&Value::Number(f64::NAN), "").unwrap(), "null");
    // node: JSON.stringify({a: undefined, b: 1, c: undefined, d: [[], {}, [undefined]]}, null, 1)
    let v = obj(&[
        ("a", Value::Undefined),
        ("b", Value::Number(1.0)),
        ("c", Value::Undefined),
        (
            "d",
            Value::array(vec![
                Value::array(vec![]),
                Value::empty_object(),
                Value::array(vec![Value::Undefined]),
            ]),
        ),
    ]);
    assert_eq!(
        stringify(&v, " ").unwrap(),
        "{\n \"b\": 1,\n \"d\": [\n  [],\n  {},\n  [\n   null\n  ]\n ]\n}"
    );
    assert_eq!(stringify(&v, "").unwrap(), r#"{"b":1,"d":[[],{},[null]]}"#);
    // node: JSON.stringify(new Markdoc.Tag('a', {x: undefined, y: 1}, ['t']))
    let tag: Value = TagValue::new(
        "a",
        [("x", Value::Undefined), ("y", Value::Number(1.0))],
        vec![Value::from("t")],
    )
    .into();
    assert_eq!(
        stringify(&tag, "").unwrap(),
        r#"{"$$mdtype":"Tag","name":"a","attributes":{"y":1},"children":["t"]}"#
    );
}

#[test]
fn identity_and_equality() {
    let a = Value::array(vec![]);
    let b = a.clone();
    assert!(strict_equals(&a, &b));
    assert!(!strict_equals(&a, &Value::array(vec![])));
    assert!(!strict_equals(
        &Value::Number(f64::NAN),
        &Value::Number(f64::NAN)
    ));
    assert!(strict_equals(&Value::Number(0.0), &Value::Number(-0.0)));
    assert!(!strict_equals(&Value::Null, &Value::Undefined));
}

#[test]
fn property_reads() {
    let s = Value::from("ab");
    assert!(matches!(get_property(&s, "length"), Ok(Value::Number(n)) if n == 2.0));
    assert!(matches!(get_property(&s, "1"), Ok(Value::String(ref c)) if c == "b"));
    assert!(matches!(get_property(&s, "01"), Ok(Value::Undefined)));
    assert!(matches!(get_property(&s, "2"), Ok(Value::Undefined)));
    // node: "ab".trim is String.prototype.trim; ({}).constructor is Object.
    use crate::next::realm::Builtin;
    assert!(matches!(
        get_property(&s, "trim"),
        Ok(Value::Builtin(Builtin::STRING_PROTOTYPE_TRIM))
    ));
    assert!(get_property(&Value::Null, "x").is_err());
    let o = obj(&[("toString", Value::Number(1.0))]);
    assert!(matches!(get_property(&o, "toString"), Ok(Value::Number(_))));
    assert!(matches!(
        get_property(&Value::empty_object(), "constructor"),
        Ok(Value::Builtin(Builtin::OBJECT))
    ));
    assert!(matches!(
        get_property(&Value::Number(1.0), "x"),
        Ok(Value::Undefined)
    ));
    assert_eq!(has_own_property(&s, "length"), Ok(true));
    assert_eq!(has_own_property(&s, "1"), Ok(true));
    assert_eq!(has_own_property(&s, "2"), Ok(false));
    assert_eq!(has_own_property(&Value::Number(1.0), "x"), Ok(false));
    assert!(has_own_property(&Value::Null, "x").is_err());
}

// tag.test.ts
#[test]
fn is_tag_detects_tags() {
    for not_a_tag in [
        Value::Undefined,
        Value::Null,
        Value::from(""),
        Value::Number(8.0),
        Value::Boolean(true),
        Value::array(vec![]),
        obj(&[("my", Value::from("object"))]),
    ] {
        assert!(!TagValue::is_tag(&not_a_tag), "{not_a_tag:?}");
    }
    assert!(TagValue::is_tag(
        &TagValue::new("tag", IndexMap::new(), vec![]).into()
    ));
}

#[test]
fn assigning_proto_is_the_prototype_setter() {
    // node: const o = {}; o.__proto__ = 1; o.a = 2; o.__proto__ = {x: 1};
    //   o.__proto__ = null; o.__proto__ = 3; o.b = 4; o[1] = 5; JSON.stringify(o)
    //   === '{"1":5,"a":2,"__proto__":3,"b":4}'
    let mut o = PlainObject::new();
    o.assign("__proto__", Value::Number(1.0));
    o.assign("a", Value::Number(2.0));
    o.assign("__proto__", obj(&[("x", Value::Number(1.0))]));
    o.assign("__proto__", Value::Null);
    o.assign("__proto__", Value::Number(3.0));
    o.assign("b", Value::Number(4.0));
    o.assign("1", Value::Number(5.0));
    let entries = o.into_entries();
    assert_eq!(
        stringify(&Value::object(entries), ""),
        Some(r#"{"1":5,"a":2,"__proto__":3,"b":4}"#.into())
    );
    // node: const p = {}; p.__proto__ = []; p.__proto__ = 'x'; Object.keys(p).length === 0
    let mut p = PlainObject::new();
    p.assign("__proto__", Value::array(vec![]));
    p.assign("__proto__", Value::from("x"));
    assert!(p.into_entries().is_empty());
}

#[test]
fn truthiness() {
    assert!(truthy(&Value::Number(0.0)) && truthy(&Value::from("")));
    assert!(!truthy(&Value::Null) && !truthy(&Value::Undefined));
    assert!(!Value::Number(0.0).to_boolean() && !Value::from("").to_boolean());
    assert!(Value::empty_object().to_boolean());
}

// ---------------------------------------------------------------------------------------
// Lone surrogates (each against node 22, `s = '😀'`)
// ---------------------------------------------------------------------------------------

/// `'😀'[i]`, read as a variable path reads it.
fn half(i: &str) -> Value {
    get_property(&Value::from("\u{1F600}"), i).expect("an index")
}

#[test]
fn a_variable_path_into_a_string_is_one_code_unit() {
    let (lead, trail) = (half("0"), half("1"));
    // s[0].length === 1, s.length === 2, s[2] === undefined
    assert!(matches!(get_property(&lead, "length"), Ok(Value::Number(n)) if n == 1.0));
    assert!(matches!(
        get_property(&Value::from("\u{1F600}"), "length"),
        Ok(Value::Number(n)) if n == 2.0
    ));
    assert!(matches!(half("2"), Value::Undefined));
    let Value::String(s) = &lead else {
        panic!("a string")
    };
    assert_eq!(s.code_units().collect::<Vec<_>>(), [0xD83D]);
    // typeof s[0] === 'string', !!s[0] === true, truthy(s[0])
    assert!(lead.as_js_str().is_some() && lead.as_str().is_none());
    assert!(lead.to_boolean() && truthy(&lead));
    // s[0][0] === s[0]
    assert!(strict_equals(&get_property(&lead, "0").unwrap(), &lead));
    // Object.entries(s): [['0', '\ud83d'], ['1', '\ude00']]
    let entries = own_entries(&Value::from("\u{1F600}"));
    assert_eq!(entries.len(), 2);
    assert!(strict_equals(&entries[0].1, &lead) && strict_equals(&entries[1].1, &trail));
    assert_eq!((entries[0].0.as_str(), entries[1].0.as_str()), ("0", "1"));
}

#[test]
fn halves_compare_by_code_unit() {
    let (lead, trail) = (half("0"), half("1"));
    // s[0] === s[0]; s[0] !== s[1]; s[0] !== '\ufffd'; s[0] + s[1] === s (below)
    assert!(strict_equals(&lead, &half("0")));
    assert!(!strict_equals(&lead, &trail));
    assert!(!strict_equals(&lead, &Value::from("\u{FFFD}")));
    assert!(!strict_equals(&Value::from("\u{FFFD}"), &trail));
}

#[test]
fn json_stringify_escapes_a_lone_surrogate() {
    let (lead, trail) = (half("0"), half("1"));
    // JSON.stringify(s[0], null, 2) === '"\\ud83d"' (eight characters)
    assert_eq!(stringify(&lead, "  ").unwrap(), "\"\\ud83d\"");
    assert_eq!(stringify(&trail, "").unwrap(), "\"\\ude00\"");
    // JSON.stringify([s[0], s[1], s, 'a' + s[1] + s[0]]) ===
    //   '["\\ud83d","\\ude00","😀","a\\ude00\\ud83d"]'
    let mixed = Value::String(JsString::from("a").concat(&{
        let Value::String(t) = &trail else {
            unreachable!()
        };
        let Value::String(l) = &lead else {
            unreachable!()
        };
        t.concat(l)
    }));
    let list = Value::array(vec![
        lead.clone(),
        trail.clone(),
        Value::from("\u{1F600}"),
        mixed,
    ]);
    assert_eq!(
        stringify(&list, "").unwrap(),
        "[\"\\ud83d\",\"\\ude00\",\"\u{1F600}\",\"a\\ude00\\ud83d\"]"
    );
}

#[test]
fn to_string_keeps_a_lone_surrogate() {
    let (lead, trail) = (half("0"), half("1"));
    // String([s[0], s[1]]) === '\ud83d,\ude00' (two lone surrogates)
    let joined = Value::array(vec![lead.clone(), trail.clone()])
        .to_js_string()
        .unwrap();
    assert_eq!(
        joined.code_units().collect::<Vec<_>>(),
        [0xD83D, 0x2C, 0xDE00]
    );
    // String([s[0]]) === s[0]
    assert!(strict_equals(
        &Value::String(Value::array(vec![lead.clone()]).to_js_string().unwrap()),
        &lead
    ));
    // ... and leaves as U+FFFD only as Rust text.
    assert_eq!(joined.to_string_lossy(), "\u{FFFD},\u{FFFD}");
}

#[test]
fn the_json_render_tree_holds_u_fffd() {
    let tree = Value::array(vec![half("0"), half("1")]);
    assert_eq!(
        tree.to_render().unwrap(),
        RenderableTreeNode::Scalar(Scalar::Array(vec![
            Scalar::String("\u{FFFD}".into()),
            Scalar::String("\u{FFFD}".into()),
        ]))
    );
}

// ---------------------------------------------------------------------------------------
// Depth: a loop at any depth, node's RangeError past its limits, on a 2 MiB stack
// ---------------------------------------------------------------------------------------

/// Runs `f` on a thread with the robustness policy's 2 MiB stack.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .expect("a thread")
        .join()
        .expect("no panic")
}

/// `depth` arrays around `inner`.
fn nested_arrays(depth: usize, inner: Value) -> Value {
    (0..depth).fold(inner, |v, _| Value::array(vec![v]))
}

fn nested_objects(depth: usize, inner: Value) -> Value {
    (0..depth).fold(inner, |v, _| Value::object([("a", v)]))
}

fn nested_tags(depth: usize, inner: Value) -> Value {
    (0..depth).fold(inner, |v, _| {
        TagValue::new("b", JsObject::new(), vec![v]).into()
    })
}

fn is_range_error<T: fmt::Debug>(result: Result<T, String>) -> bool {
    matches!(result, Err(ref e) if e == RANGE_ERROR)
}

#[test]
fn to_js_string_joins_as_deep_as_node_and_throws_past_it() {
    on_small_stack(|| {
        // node: String() of 3,301 nested arrays around 'x' is 'x'; of 3,302 (or 3,301 around
        // []) a RangeError.
        let at = nested_arrays(MAX_JOIN_DEPTH, Value::from("x"));
        assert_eq!(at.to_js_string().unwrap(), "x");
        assert!(is_range_error(
            nested_arrays(MAX_JOIN_DEPTH + 1, Value::from("x")).to_js_string()
        ));
        assert!(is_range_error(
            nested_arrays(MAX_JOIN_DEPTH, Value::array(vec![])).to_js_string()
        ));
        // An object ends the join ("[object Object]"), whatever it holds.
        let o = nested_arrays(MAX_JOIN_DEPTH, nested_objects(100_000, Value::Null));
        assert_eq!(o.to_js_string().unwrap(), "[object Object]");
        drop((at, o));
    });
}

#[test]
fn json_stringify_writes_as_deep_as_node_and_throws_past_it() {
    on_small_stack(|| {
        for gap in ["", "  "] {
            let at = nested_arrays(MAX_JSON_DEPTH, Value::from("x"));
            let text = stringify(&at, gap).unwrap();
            assert!(text.starts_with('[') && text.contains("\"x\""));
            assert!(is_range_error(json_stringify(
                &nested_arrays(MAX_JSON_DEPTH + 1, Value::from("x")),
                gap
            )));
            assert!(stringify(&nested_objects(MAX_JSON_DEPTH, Value::Null), gap).is_some());
            // An empty array or object at the bottom is a level too.
            assert!(is_range_error(json_stringify(
                &nested_objects(MAX_JSON_DEPTH, Value::empty_object()),
                gap
            )));
            // A tag is an object around its children array: two levels each.
            let tags = MAX_JSON_DEPTH / 2;
            assert!(stringify(&nested_tags(tags, Value::from("x")), gap).is_some());
            assert!(is_range_error(json_stringify(
                &nested_tags(tags + 1, Value::from("x")),
                gap
            )));
        }
    });
}

#[test]
fn to_render_converts_up_to_its_limit() {
    // Where the stack grows, as deep as `JSON.stringify` writes; else MAX_RENDER_TREE_DEPTH.
    let limit = if crate::depth::can_grow() {
        MAX_JSON_DEPTH
    } else {
        MAX_RENDER_TREE_DEPTH
    };
    on_small_stack(move || {
        let at = nested_arrays(limit, Value::from("x"));
        let tree = at.to_render().unwrap();
        // The tree it returns is safe to clone, compare and drop on this stack.
        let copy = tree.clone();
        assert_eq!(copy, tree);
        assert!(format!("{tree:?}").len() < 100_000);
        drop((copy, tree));
        assert!(is_range_error(
            nested_arrays(limit + 1, Value::from("x")).to_render()
        ));
        let objects = nested_objects(limit, Value::Null).to_render().unwrap();
        assert_eq!(objects.clone(), objects);
        // ... and so is its JSON, to drop; serde_json's own clone recurses, which a debug
        // build has stack for at the fixed limit only.
        let json = objects.to_json();
        if limit == MAX_RENDER_TREE_DEPTH {
            assert_eq!(json.clone(), json);
        }
        drop((json, objects));
        assert!(is_range_error(
            nested_objects(limit + 1, Value::Null).to_render()
        ));
        let tags = limit / 2;
        let tree = nested_tags(tags, Value::from("x")).to_render().unwrap();
        assert_eq!(tree.clone(), tree);
        assert!(serde_json::to_string(&tree).is_ok());
        assert!(is_range_error(
            nested_tags(tags + 1, Value::from("x")).to_render()
        ));
    });
}

#[test]
fn deep_values_build_convert_and_drop_in_a_loop() {
    on_small_stack(|| {
        const DEEP: usize = 300_000;
        // Dropping: arrays, objects, tags and the three mixed, 300,000 deep.
        drop(nested_arrays(DEEP, Value::from("x")));
        drop(nested_objects(DEEP, Value::Null));
        drop(nested_tags(DEEP, Value::Null));
        let mixed = (0..DEEP).fold(Value::Null, |v, i| match i % 3 {
            0 => Value::array(vec![v]),
            1 => Value::object([("k", v)]),
            _ => TagValue::new("t", [("a", v)], vec![]).into(),
        });
        // A clone shares the contents: dropping one leaves the other whole.
        let kept = mixed.clone();
        drop(mixed);
        assert!(matches!(
            kept,
            Value::Tag(_) | Value::Object(_) | Value::Array(_)
        ));
        // Debug prints the top and an ellipsis below.
        assert!(format!("{kept:?}").contains('…'));
        drop(kept);
        // From JSON, from an AST value and from a render tree: any depth.
        let mut json = serde_json::Value::Null;
        for _ in 0..100_000 {
            json = serde_json::Value::Array(vec![json]);
        }
        let value = Value::from_json(&json);
        dismantle_json(json);
        let mut depth = 0;
        let mut at = &value;
        while let Value::Array(items) = at {
            depth += 1;
            at = items.get(0).expect("an item");
        }
        assert_eq!(depth, 100_000);
        drop(value);
    });
}

/// Drops a deep `serde_json::Value` without recursing (serde_json's own drop recurses).
fn dismantle_json(json: serde_json::Value) {
    let mut pending = vec![json];
    while let Some(v) = pending.pop() {
        match v {
            serde_json::Value::Array(items) => pending.extend(items),
            serde_json::Value::Object(o) => pending.extend(o.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

#[test]
fn values_round_trip_through_json_and_render_trees() {
    let json =
        serde_json::json!({"b": [1, {"2": null, "1": true}], "0": "z", "a": {"$$mdtype": "Tag"}});
    let value = Value::from_json(&json);
    // node: JSON.stringify(JSON.parse(text)) puts "0" first, and "1" before "2".
    assert_eq!(
        stringify(&value, "").unwrap(),
        r#"{"0":"z","b":[1,{"1":true,"2":null}],"a":{"$$mdtype":"Tag"}}"#
    );
    let tag: Value = TagValue::new(
        "p",
        [("x", Value::from("1")), ("u", Value::Undefined)],
        vec![
            Value::from("t"),
            TagValue::new("b", JsObject::new(), vec![Value::Number(2.0)]).into(),
        ],
    )
    .into();
    let tree = tag.to_render().unwrap();
    let back = Value::from_render(&tree);
    assert_eq!(stringify(&back, "").unwrap(), stringify(&tag, "").unwrap());
    let Value::Tag(t) = &back else {
        panic!("a tag")
    };
    assert_eq!(t.name.as_str(), Some("p"));
    assert!(t.attributes.get("u").is_none());
}

#[test]
fn a_regexp_sees_the_code_unit() {
    use crate::next::schema::JsRegExp;
    let (lead, trail) = (half("0"), half("1"));
    let (Value::String(lead), Value::String(trail)) = (&lead, &trail) else {
        unreachable!("strings")
    };
    // node: /^\uD83D$/.test('😀'[0]) === true, /^\uD83D$/.test('�') === false,
    //   /^.$/u.test('😀'[1]) === true, /^\uDE00$/.test('😀'[0] + '😀'[1]) === false
    let lead_only = JsRegExp::new("^\\uD83D$", "").unwrap();
    assert!(lead_only.test_js(lead).unwrap());
    assert!(!lead_only.test_js(&JsString::from("\u{FFFD}")).unwrap());
    assert!(!lead_only.test_js(trail).unwrap());
    assert!(JsRegExp::new("^.$", "u").unwrap().test_js(trail).unwrap());
    assert!(
        !JsRegExp::new("^\\uDE00$", "")
            .unwrap()
            .test_js(&lead.concat(trail))
            .unwrap()
    );
    assert!(lead_only.test_utf16(&[0xD83D]).unwrap());
}

// ---------------------------------------------------------------------------------------
// The object model: prototypes, holes, extra properties (node 22.22.1's answers)
// ---------------------------------------------------------------------------------------

fn with_proto(entries: &[(&str, Value)], proto: Proto) -> Value {
    let mut m = JsObject::new();
    for (k, v) in entries {
        m.insert(*k, v.clone());
    }
    Value::object_with_proto(m, proto)
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

#[test]
fn to_string_reads_through_prototypes() {
    // String(Object.setPrototypeOf({length: 3}, [1, [2, 3]])) is "1,2,3,".
    let proto = Value::array(vec![n(1.0), Value::array(vec![n(2.0), n(3.0)])]);
    let v = with_proto(&[("length", n(3.0))], Proto::Value(proto));
    assert_eq!(v.to_js_string().unwrap(), "1,2,3,");
    // String(Object.setPrototypeOf({}, Object.assign([1, 2], {length: 5}))) is "1,2,,,".
    let proto = Value::sparse_array(5, [(0, n(1.0)), (1, n(2.0))], IndexMap::new());
    assert_eq!(
        with_proto(&[], Proto::Value(proto)).to_js_string().unwrap(),
        "1,2,,,"
    );
    // A null prototype, or a `toString` that is no function over an array's: TypeError.
    assert_eq!(
        with_proto(&[], Proto::Null).to_js_string().unwrap_err(),
        NO_PRIMITIVE
    );
    let proto = Value::array(vec![n(9.0)]);
    assert_eq!(
        with_proto(&[("toString", n(5.0))], Proto::Value(proto))
            .to_js_string()
            .unwrap_err(),
        NO_PRIMITIVE
    );
    // String([1, , 'b']) is "1,,b".
    let holes = Value::sparse_array(3, [(0, n(1.0)), (2, Value::from("b"))], IndexMap::new());
    assert_eq!(holes.to_js_string().unwrap(), "1,,b");
}

#[test]
fn json_and_entries_see_own_properties_and_holes() {
    // JSON.stringify(Object.setPrototypeOf({x: 1}, [1, 2])) is {"x":1}.
    let v = with_proto(
        &[("x", n(1.0))],
        Proto::Value(Value::array(vec![n(1.0), n(2.0)])),
    );
    assert_eq!(stringify(&v, "").as_deref(), Some(r#"{"x":1}"#));
    // Object.assign([1, , 3], {b: 2}): JSON [1,null,3]; entries 0, 2, b.
    let mut named = IndexMap::new();
    named.insert("b".to_string(), n(2.0));
    let a = Value::sparse_array(3, [(0, n(1.0)), (2, n(3.0))], named);
    assert_eq!(stringify(&a, "").as_deref(), Some("[1,null,3]"));
    assert_eq!(
        stringify(&a, "  ").as_deref(),
        Some("[\n  1,\n  null,\n  3\n]")
    );
    let keys: Vec<String> = own_entries(&a).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["0", "2", "b"]);
    // A length of 2^32 - 1 costs nothing until written: V8's string limit.
    let huge = Value::sparse_array(u32::MAX, [(u32::MAX - 1, n(1.0))], IndexMap::new());
    assert_eq!(json_stringify(&huge, ""), Err(INVALID_STRING_LENGTH.into()));
    assert_eq!(huge.to_js_string(), Err(INVALID_STRING_LENGTH.into()));
    assert_eq!(own_entries(&huge).len(), 1);
}

#[test]
fn reads_and_sets_go_through_the_chain() {
    // `in` and [[Get]] through a document prototype, then Object.prototype.
    let proto = obj(&[("a", n(1.0))]);
    let v = with_proto(&[("b", n(2.0))], Proto::Value(proto));
    assert!(matches!(get(&v, "a"), Ok(Value::Number(x)) if x == 1.0));
    assert!(has_property(&v, "a").unwrap());
    assert!(has_property(&v, "toString").unwrap());
    assert!(!has_property(&v, "c").unwrap());
    assert!(!has_own_property(&v, "a").unwrap());
    // `output.__proto__ = [1]` is the setter: a prototype, no own key.
    let mut out = PlainObject::new();
    out.assign("__proto__", Value::array(vec![n(1.0)]));
    out.assign("x", n(2.0));
    let o = Value::Object(std::sync::Arc::new(out.into_object()));
    assert_eq!(stringify(&o, "").as_deref(), Some(r#"{"x":2}"#));
    assert_eq!(o.to_js_string().unwrap(), "1");
    // With a null prototype there is no setter: `__proto__` is an own key.
    let mut out = PlainObject::with_proto(Proto::Null);
    out.assign("__proto__", n(1.0));
    let o = Value::Object(std::sync::Arc::new(out.into_object()));
    assert_eq!(stringify(&o, "").as_deref(), Some(r#"{"__proto__":1}"#));
}

#[test]
fn array_methods_check_what_upstream_calls() {
    // Object.assign([1], {map: true}).map(...): TypeError; {constructor: 5}: species.
    let mut named = IndexMap::new();
    named.insert("map".to_string(), Value::Boolean(true));
    let a = Value::sparse_array(1, [(0, n(1.0))], named);
    assert_eq!(
        array_method(&a, "map", Method::ArrayMap, "value"),
        Err("TypeError: value.map is not a function".into())
    );
    let mut named = IndexMap::new();
    named.insert("constructor".to_string(), n(5.0));
    let a = Value::sparse_array(1, [(0, n(1.0))], named);
    assert_eq!(
        species_check(&a),
        Err("TypeError: object.constructor[Symbol.species] is not a constructor".into())
    );
    // {constructor: {}} is an object: its Symbol.species is undefined, a plain array.
    let mut named = IndexMap::new();
    named.insert("constructor".to_string(), Value::empty_object());
    let a = Value::sparse_array(1, [(0, n(1.0))], named);
    assert_eq!(species_check(&a), Ok(()));
}

#[test]
fn tags_and_ast_objects_through_the_chain() {
    // Tag.isTag(Object.setPrototypeOf({}, new Tag('p'))) is true; an object inheriting a
    // variable is an AST value.
    let tag: Value = TagValue::new("p", Object::default(), vec![]).into();
    let v = with_proto(&[], Proto::Value(tag));
    assert!(TagValue::is_tag(&v));
    let var = Value::variable(Value::array(vec![Value::from("v")]));
    let v = with_proto(&[("a", n(1.0))], Proto::Value(var));
    assert!(is_ast(&v) && mdtype_is(&v, "Variable"));
    // A variable is an object: String() is [object Object].
    assert_eq!(
        Value::variable(Value::Boolean(true))
            .to_js_string()
            .unwrap(),
        "[object Object]"
    );
}
