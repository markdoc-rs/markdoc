//! The next layer's comparison harness, checked without node.
//!
//! `tests/next/golden/conformance.jsonl` is upstream Markdoc 0.5.11's own output for the 105
//! conformance cases (tokens, AST, partials' ASTs, errors, render tree, HTML), written by
//!
//! ```text
//! python3 -I tests/next/compare.py --golden tests/next/golden/conformance.jsonl \
//!     --conformance --markdoc DIST/index.js
//! ```
//!
//! Each of upstream's values goes through `markdoc::next::compare`'s reader and writer and must
//! come back as it was, whole and in upstream's key order, and every AST through the crate's
//! own serde, which writes `Variable`, `Function` and `Tag` values as upstream does, must come
//! back to the same JSON. Run the same check over any input set, against node, with
//! `compare.py --roundtrip`.

use markdoc::ast::Node;
use markdoc::next::compare::{
    first_difference, node_from_json, node_json, render_from_json, render_json, to_json_string,
    tokens_from_json, tokens_json,
};
use markdoc::types::Scalar;
use serde_json::Value;

fn golden() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/next/golden/conformance.jsonl"
    );
    std::fs::read_to_string(path)
        .expect("tests/next/golden/conformance.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

fn check(id: &str, what: &str, expected: &Value, got: &Value, failures: &mut Vec<String>) {
    if let Some((path, a, b)) = first_difference(expected, got) {
        failures.push(format!("{id}: {what}{path}: upstream {a} | back {b}"));
    }
}

/// Every attribute and annotation value of a node and its descendants.
fn values(node: &Node, out: &mut Vec<Scalar>) {
    out.extend(node.attributes.values().cloned());
    out.extend(node.annotations.iter().map(|a| a.value.clone()));
    for child in node.children.iter().chain(node.slots.values()) {
        values(child, out);
    }
}

#[test]
fn upstream_output_round_trips_through_the_converters() {
    let cases = golden();
    assert_eq!(cases.len(), 105, "the conformance corpus has 105 cases");
    let mut failures = Vec::new();
    let (mut asts, mut trees, mut tokens, mut scalars) = (0, 0, 0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        if let Some(t) = case.get("tokens") {
            let back = tokens_json(&tokens_from_json(t).expect("tokens read"));
            check(id, "tokens", t, &back, &mut failures);
            tokens += 1;
        }
        let mut nodes = Vec::new();
        if let Some(ast) = case.get("ast") {
            nodes.push(("ast".to_string(), ast));
        }
        for (name, ast) in case
            .get("partials")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            nodes.push((format!("partials.{name}"), ast));
        }
        for (what, ast) in nodes {
            let node = node_from_json(ast).expect("a Node read");
            let expected = ast;
            check(id, &what, expected, &node_json(&node), &mut failures);
            // The crate's serde: write, read back, same upstream JSON.
            let text = serde_json::to_string(&node).expect("a Node serializes");
            let again: Node = serde_json::from_str(&text).expect("and deserializes");
            check(
                id,
                &format!("{what} (serde)"),
                expected,
                &node_json(&again),
                &mut failures,
            );
            let mut all = Vec::new();
            values(&node, &mut all);
            for value in all {
                let json = serde_json::to_value(&value).expect("a Scalar serializes");
                let back: Scalar = serde_json::from_value(json).expect("and deserializes");
                if back != value
                    || to_json_string(&back.to_json()) != to_json_string(&value.to_json())
                {
                    failures.push(format!(
                        "{id}: {what}: Scalar serde changed {value:?} to {back:?}"
                    ));
                }
                scalars += 1;
            }
            asts += 1;
        }
        if let Some(tree) = case.get("transformed") {
            let back = render_json(&render_from_json(tree).expect("a render tree read"));
            check(id, "transformed", tree, &back, &mut failures);
            trees += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "{} difference(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!((tokens, trees), (105, 105));
    assert!(
        asts >= 105 && scalars > 100,
        "{asts} ASTs, {scalars} values"
    );
}
