//! The tokenizer port (`markdoc::next::tokenizer`) against upstream's own token streams,
//! without node.
//!
//! - `tests/next/tokenizer/golden.jsonl`: the regression set `tests/next/tokenizer/golden.py`
//!   writes from upstream's Tokenizer: a probe of each quirk of upstream's plugins, under the
//!   configurations where it shows, and the documents the fuzzer's differences were minimised
//!   to (`tests/next/tokenizer/minimised.jsonl`). Each line is `{id, code, tokenizer, tokens}`
//!   or `{id, code, tokenizer, threw: {name, message}}`.
//! - `tests/next/golden/conformance.jsonl`: upstream's tokens for the 105 conformance cases.
//!
//! The port's tokens are compared as upstream's JSON of them (`tokens_json`), whole, keys in
//! order; where upstream throws, the port must return the same error (name and message), not
//! panic.
//!
//! ```text
//! cargo test --test next_tokenizer
//! python3 -I tests/next/tokenizer/golden.py --markdoc DIST/index.js \
//!     --minimised tests/next/tokenizer/minimised.jsonl
//! ```

use markdoc::next::compare::{first_difference, tokens_json};
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
use serde_json::{Map, Value};

fn lines(path: &str) -> Vec<Value> {
    let path = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// The case's difference from upstream, if any.
fn check(case: &Value) -> Option<String> {
    let id = case["id"].as_str().unwrap_or("?");
    let code = case["code"].as_str().expect("code");
    let config = case
        .get("tokenizer")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    let tokenizer = Tokenizer::new(TokenizerOptions::from_json(&config).expect("options"));
    let result = std::panic::catch_unwind(|| tokenizer.tokenize(code));
    match (result, case.get("tokens"), case.get("threw")) {
        (Err(_), _, _) => Some(format!("{id}: the port panicked")),
        (Ok(Ok(tokens)), Some(expected), _) => first_difference(expected, &tokens_json(&tokens))
            .map(|(path, a, b)| format!("{id}: {path}: upstream {a} | port {b}")),
        (Ok(Err(error)), _, Some(threw)) => {
            let expected = (threw["name"].as_str(), threw["message"].as_str());
            (expected != (Some(error.name.as_str()), Some(error.message.as_str())))
                .then(|| format!("{id}: upstream threw {threw}, the port {error}"))
        }
        (Ok(Ok(_)), None, Some(threw)) => Some(format!("{id}: upstream threw {threw}")),
        (Ok(Err(error)), Some(_), _) => Some(format!("{id}: the port threw {error}")),
        _ => Some(format!("{id}: a golden line has tokens or threw")),
    }
}

fn run(cases: &[Value]) -> Vec<String> {
    cases.iter().filter_map(check).collect()
}

#[test]
fn the_regression_set_tokenizes_as_upstream() {
    let cases = lines("tests/next/tokenizer/golden.jsonl");
    assert!(cases.len() >= 90, "golden.jsonl has {} cases", cases.len());
    let throws = cases.iter().filter(|c| c.get("threw").is_some()).count();
    assert!(throws >= 14, "{throws} cases where upstream throws");
    let failures = run(&cases);
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn the_conformance_cases_tokenize_as_upstream() {
    let cases = lines("tests/next/golden/conformance.jsonl");
    assert_eq!(cases.len(), 105);
    let failures = run(&cases);
    assert!(
        failures.is_empty(),
        "{} of 105 cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
