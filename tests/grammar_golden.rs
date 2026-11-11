//! The tag grammar port against upstream's own parser, without node.
//!
//! `tests/grammar/golden.jsonl` holds requests and, for each, the exact line
//! `tests/grammar/oracle_grammar.cjs` prints for it from upstream Markdoc 0.5.11's
//! `src/grammar/tag.js`: `{id, result}` or `{id, error: {message, expected, found, location}}`
//! in `JSON.stringify`'s text. The requests are upstream's tag.test.ts cases, every tag in
//! tests/marktest/tests.yaml, probes of the grammar's corners, and generated cases covering
//! each shape of result and each set of expectations the fuzzer reached
//! (`tests/grammar/golden.py` writes the file). This test prints the same line from
//! `markdoc::grammar` and requires it to be identical.
//!
//! ```text
//! cargo test --test grammar_golden
//! python3 -I tests/grammar/golden.py --tagjs TAG_JS --markdoc DIST/index.js --tag-test TAG_TEST_TS
//! ```

use std::path::PathBuf;

use markdoc::grammar;

/// The line `examples/grammar_parse.rs` prints.
fn line(id: &str, content: &str) -> String {
    let mut out = String::from("{\"id\":");
    out.push_str(&serde_json::to_string(id).unwrap());
    match grammar::parse(content) {
        Ok(result) => {
            out.push_str(",\"result\":");
            result.write_json(&mut out);
        }
        Err(grammar::ParseError::Syntax(error)) => {
            out.push_str(",\"error\":{");
            error.write_fields_json(&mut out);
            out.push('}');
        }
        // what the oracle prints where tag.js throws something else than its SyntaxError
        Err(grammar::ParseError::Range(error)) => {
            out.push_str(",\"crash\":");
            out.push_str(&serde_json::to_string(&error.to_string()).unwrap());
        }
    }
    out.push('}');
    out
}

#[test]
fn every_golden_line_is_reproduced() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/grammar/golden.jsonl");
    let text = std::fs::read_to_string(&path).expect("tests/grammar/golden.jsonl");
    let mut cases = 0;
    let mut failures = Vec::new();
    for row in text.lines().filter(|l| !l.trim().is_empty()) {
        let case: serde_json::Value = serde_json::from_str(row).expect("a JSON line");
        let id = case["id"].as_str().unwrap();
        let content = case["content"].as_str().unwrap();
        let expected = case["line"].as_str().unwrap();
        cases += 1;
        let actual = line(id, content);
        if actual != expected {
            failures.push(format!(
                "{id} {content:?}\n  node: {expected}\n  rust: {actual}"
            ));
        }
    }
    assert!(cases > 700, "golden.jsonl has {cases} cases");
    assert!(
        failures.is_empty(),
        "{} of {cases} cases differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
