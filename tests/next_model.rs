//! The JavaScript object model of `markdoc::next`, pinned to node.
//!
//! `tests/next/golden/model.jsonl` is upstream Markdoc 0.5.11's output (node 22.22.1) for the
//! cases in `findings/conformance/work/model/model-cases.jsonl` (written by
//! `work/model/make_golden.sh`, `compare.py --golden`): `__proto__` attributes and hash keys
//! and parameters (prototypes the AST holds and every stage reads through), class shorthands
//! on every kind of class value (an array's elements, holes and `length`, a variable's or a
//! call's own properties, `.map` and `.constructor` making upstream's array methods throw),
//! slots filed through `__proto__`, JSON configs read as `JSON.parse` makes them (own
//! `__proto__` keys, a `render` that is no string, schemas that are `null` or a string, error
//! levels and types of any value, a `validate` or `transform` that is no function), and
//! hand-made token streams with annotations that are not attribute objects.
//!
//! Each case runs end to end through the port (its tokenizer, or the case's own tokens;
//! parser; `validate`; `transform`; the HTML renderer) and every output must be node's: the
//! AST (as `compare.py` compares it), `validate()`'s errors (type, id, level, message), the
//! render tree (`JSON.stringify`), the HTML, and what each stage threw.

use markdoc::markdown_it::Token;
use markdoc::next::compare::{
    first_difference, node_json, request_tokens_json_revived, tokens_from_json,
};
use markdoc::next::index;
use markdoc::next::parser::ParserArgs;
use markdoc::next::renderers::html;
use markdoc::next::schema::value::json_stringify;
use markdoc::next::schema::{Config, Partial};
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
use serde_json::{Map, Value};
use std::sync::Arc;

fn golden() -> Vec<Value> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/next/golden/model.jsonl");
    std::fs::read_to_string(path)
        .expect("tests/next/golden/model.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// What a stage threw, as the oracle writes it (`Name: message`), if it threw.
fn upstream_threw(case: &Value, stage: &str) -> Option<String> {
    let failed = case.get("failed")?.get(stage)?;
    Some(format!(
        "{}: {}",
        failed["name"].as_str().unwrap_or_default(),
        failed["message"].as_str().unwrap_or_default()
    ))
}

fn tokenizer(case: &Value) -> Tokenizer {
    let options: Map<String, Value> = case
        .get("tokenizer")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Tokenizer::new(TokenizerOptions::from_json(&options).expect("tokenizer options"))
}

fn parse_args(case: &Value, file: Option<&str>) -> ParserArgs {
    ParserArgs {
        file: file.map(str::to_owned),
        slots: case.get("slots").and_then(Value::as_bool).unwrap_or(false),
        ..ParserArgs::default()
    }
}

/// The stage's output and the port's, or what each threw.
#[derive(Debug, PartialEq)]
enum Out {
    Json(Value),
    Text(String),
    Threw(String),
}

/// Records a difference between upstream's output and the port's.
fn differ(failures: &mut Vec<String>, id: &str, stage: &str, upstream: Out, port: Out) {
    let same = match (&upstream, &port) {
        (Out::Json(a), Out::Json(b)) => first_difference(a, b).is_none(),
        _ => upstream == port,
    };
    if !same {
        let detail = match (&upstream, &port) {
            (Out::Json(a), Out::Json(b)) => format!("{:?}", first_difference(a, b)),
            _ => format!("upstream {upstream:?} | port {port:?}"),
        };
        failures.push(format!("{id}: {stage}: {detail}"));
    }
}

#[test]
fn every_case_is_upstreams() {
    let cases = golden();
    assert!(cases.len() >= 100, "the golden has the model cases");
    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");

        // tokens
        let tokens: Result<Vec<Token>, String> = match case.get("requestTokens") {
            Some(tokens) => {
                markdoc::next::compare::revive_check(tokens).and_then(|()| tokens_from_json(tokens))
            }
            None => tokenizer(case)
                .tokenize(case["code"].as_str().unwrap_or_default())
                .map_err(|e| e.to_string()),
        };
        let tokens = match (tokens, upstream_threw(case, "tokens")) {
            (Ok(tokens), None) => {
                if case.get("requestTokens").is_some() {
                    differ(
                        &mut failures,
                        id,
                        "tokens",
                        Out::Json(case["tokens"].clone()),
                        Out::Json(request_tokens_json_revived(&case["requestTokens"])),
                    );
                }
                tokens
            }
            (Err(port), Some(upstream)) => {
                differ(
                    &mut failures,
                    id,
                    "tokens",
                    Out::Threw(upstream),
                    Out::Threw(port),
                );
                continue;
            }
            (port, upstream) => {
                failures.push(format!(
                    "{id}: tokens: upstream {upstream:?} | port {port:?}"
                ));
                continue;
            }
        };

        // ast
        let code = case["code"].as_str().unwrap_or_default();
        let file = case.get("file").and_then(Value::as_str);
        let ast = index::parse_tokens(&tokens, code, &parse_args(case, file))
            .map_err(|e| index::thrown(&e).to_owned());
        let ast = match (ast, upstream_threw(case, "ast")) {
            (Ok(ast), None) => {
                differ(
                    &mut failures,
                    id,
                    "ast",
                    // The whole JSON, upstream's lines and location included (the strict harness).
                    Out::Json(case["ast"].clone()),
                    Out::Json(node_json(&ast)),
                );
                ast
            }
            (Err(port), Some(upstream)) => {
                differ(
                    &mut failures,
                    id,
                    "ast",
                    Out::Threw(upstream),
                    Out::Threw(port),
                );
                continue;
            }
            (port, upstream) => {
                failures.push(format!(
                    "{id}: ast: upstream {upstream:?} | port {:?}",
                    port.map(|_| "an AST")
                ));
                continue;
            }
        };

        let config_json = case.get("config").cloned().unwrap_or(Value::Null);
        let config = Config::from_json(&config_json).expect("the config reads");

        // errors: type, id, level and message (lines are positions, compared apart)
        let errors = index::validate(&ast, &config).map_err(|e| index::thrown(&e).to_owned());
        match (errors, upstream_threw(case, "errors")) {
            (Ok(errors), None) => {
                let port: Vec<Value> = errors
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "type": e.node_type.to_string(),
                            "id": e.error.id,
                            "level": e.error.level.to_json(),
                            "message": e.error.message,
                        })
                    })
                    .collect();
                let upstream: Vec<Value> = case["errors"]
                    .as_array()
                    .expect("errors")
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "type": e["type"], "id": e["id"], "level": e["level"],
                            "message": e["message"],
                        })
                    })
                    .collect();
                differ(
                    &mut failures,
                    id,
                    "errors",
                    Out::Json(upstream.into()),
                    Out::Json(port.into()),
                );
            }
            (Err(port), Some(upstream)) => differ(
                &mut failures,
                id,
                "errors",
                Out::Threw(upstream),
                Out::Threw(port),
            ),
            (port, upstream) => failures.push(format!(
                "{id}: errors: upstream {upstream:?} | port {:?}",
                port.map(|e| e.len())
            )),
        }

        // transform, with the partials parsed as the oracle parses them
        let mut config = config;
        // `Object.entries(req.partials || config.partials || {})`, as the oracle parses them.
        let given = match case.get("requestPartials") {
            Some(partials) => partials.clone(),
            None => config_json.get("partials").cloned().unwrap_or(Value::Null),
        };
        let mut parsed = indexmap::IndexMap::new();
        let sources: Vec<(String, Value)> = match &given {
            Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Value::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect(),
            _ => Vec::new(),
        };
        for (name, src) in &sources {
            let src = src.as_str().unwrap_or_default();
            let tokens = tokenizer(case).tokenize(src).expect("a partial tokenizes");
            let node = index::parse_tokens(&tokens, src, &parse_args(case, Some(name)))
                .expect("a partial parses");
            parsed.insert(name.clone(), Partial::Node(Arc::new(node)));
        }
        config.partials = Some(parsed);
        config.partials_null = false;
        let tree = index::transform(&ast, &config).map_err(|e| index::thrown(&e).to_owned());
        let tree = match (tree, upstream_threw(case, "transformed")) {
            (Ok(tree), None) => {
                let port: Value = serde_json::from_str(
                    &json_stringify(&tree, "")
                        .expect("writable")
                        .unwrap_or_else(|| "null".into()),
                )
                .expect("JSON");
                differ(
                    &mut failures,
                    id,
                    "transform",
                    Out::Json(case["transformed"].clone()),
                    Out::Json(port),
                );
                tree
            }
            (Err(port), Some(upstream)) => {
                differ(
                    &mut failures,
                    id,
                    "transform",
                    Out::Threw(upstream),
                    Out::Threw(port),
                );
                continue;
            }
            (port, upstream) => {
                failures.push(format!(
                    "{id}: transform: upstream {upstream:?} | port {:?}",
                    port.map(|_| "a tree")
                ));
                continue;
            }
        };

        // html
        let rendered = html::render(&tree).map_err(|e| index::thrown(&e).to_owned());
        match (rendered, upstream_threw(case, "html")) {
            (Ok(port), None) => differ(
                &mut failures,
                id,
                "html",
                Out::Text(case["html"].as_str().unwrap_or_default().to_owned()),
                Out::Text(port),
            ),
            (Err(port), Some(upstream)) => differ(
                &mut failures,
                id,
                "html",
                Out::Threw(upstream),
                Out::Threw(port),
            ),
            (port, upstream) => {
                failures.push(format!("{id}: html: upstream {upstream:?} | port {port:?}"))
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
