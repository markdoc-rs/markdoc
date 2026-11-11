//! The Rust half of the tag grammar's verification harness: reads the requests
//! tests/grammar/oracle_grammar.cjs reads (JSONL on stdin) and prints, for the port in
//! `markdoc::grammar`, the line the oracle prints for upstream's tag.js.
//!
//!   request: {id, content}
//!   output:  {id, result} or {id, error: {message, expected, found, location}},
//!            in JSON.stringify's text, or {id, crash} where the port returns its RangeError
//!            (values nested deeper than grammar::MAX_NESTING) or panics, as the oracle
//!            prints {id, crash} where tag.js throws something else than its SyntaxError
//!
//!   cargo run --release --example grammar_parse < requests.jsonl > results.jsonl
//!   cargo run --release --example grammar_parse -- --bench ROUNDS < requests.jsonl
//!
//! `--bench` parses every request's content ROUNDS times and reports the throughput
//! instead (no output lines).

use std::io::{BufRead, BufWriter, Write};
use std::panic;
use std::time::Instant;

use markdoc::grammar::{self, ParseError};

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

fn answer(id: &str, content: &str) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("{\"id\":");
    out.push_str(&json_string(id));
    match panic::catch_unwind(|| grammar::parse(content)) {
        Ok(Ok(result)) => {
            out.push_str(",\"result\":");
            result.write_json(&mut out);
        }
        Ok(Err(ParseError::Syntax(error))) => {
            out.push_str(",\"error\":{");
            error.write_fields_json(&mut out);
            out.push('}');
        }
        // where the JavaScript throws RangeError
        Ok(Err(ParseError::Range(error))) => {
            out.push_str(",\"crash\":");
            out.push_str(&json_string(&error.to_string()));
        }
        // where the JavaScript throws something else than its SyntaxError
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            out.push_str(",\"crash\":");
            out.push_str(&json_string(&message));
        }
    }
    out.push('}');
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bench = args.iter().position(|a| a == "--bench").map(|i| {
        args.get(i + 1)
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(10)
    });

    let stdin = std::io::stdin();
    let mut requests = Vec::new();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin is UTF-8");
        if line.trim().is_empty() {
            continue;
        }
        let request: serde_json::Value = serde_json::from_str(&line).expect("a JSON request");
        let id = request["id"]
            .as_str()
            .expect("request.id is a string")
            .to_string();
        let content = request["content"]
            .as_str()
            .expect("request.content is a string")
            .to_string();
        requests.push((id, content));
    }

    if let Some(rounds) = bench {
        let units: usize = requests.iter().map(|(_, c)| c.encode_utf16().count()).sum();
        let bytes: usize = requests.iter().map(|(_, c)| c.len()).sum();
        let (mut ok, mut errors) = (0usize, 0usize);
        let start = Instant::now();
        for _ in 0..rounds {
            for (_, content) in &requests {
                match grammar::parse(content) {
                    Ok(_) => ok += 1,
                    Err(_) => errors += 1,
                }
            }
        }
        let secs = start.elapsed().as_secs_f64();
        let n = (requests.len() * rounds) as f64;
        println!(
            "{} inputs x {rounds} rounds ({ok} results, {errors} errors) in {secs:.3}s: \
             {:.0} inputs/s, {:.1} MB/s, {:.1} M UTF-16 units/s",
            requests.len(),
            n / secs,
            (bytes * rounds) as f64 / secs / 1e6,
            (units * rounds) as f64 / secs / 1e6,
        );
        return;
    }

    panic::set_hook(Box::new(|_| {})); // a panic is reported as {id, crash}
    let mut out = BufWriter::new(std::io::stdout().lock());
    for (id, content) in &requests {
        writeln!(out, "{}", answer(id, content)).expect("stdout");
    }
}
