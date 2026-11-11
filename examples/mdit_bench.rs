//! Throughput of the markdown-it port: parses every document of a JSONL file (`{id, src}`
//! per line, as the harness inputs) a number of rounds and prints MB/s and documents/s.
//!
//!   cargo run --release --example mdit_bench -- INPUTS.jsonl [ROUNDS] [CONFIG]
//!
//! CONFIG is `markdoc` (the default: Markdoc's tokenizer, i.e. the default preset with
//! `allowIndentation` and lheading and code disabled), `markdoc-noai` (the same with
//! `allowIndentation` false), `default` or `commonmark`.

use std::hint::black_box;
use std::time::Instant;

use markdoc::markdown_it::{Env, MarkdownIt};
use serde_json::{Map, Value, json};

fn parser(config: &str) -> MarkdownIt {
    let (preset, options, markdoc) = match config {
        "markdoc" => ("default", json!({"allowIndentation": true}), true),
        "markdoc-noai" => ("default", json!({"allowIndentation": false}), true),
        "default" => ("default", json!({}), false),
        "commonmark" => ("commonmark", json!({}), false),
        other => panic!("unknown config {other}"),
    };
    let options: Map<String, Value> = options.as_object().cloned().unwrap_or_default();
    let mut md = MarkdownIt::with_options(preset, &options).expect("preset");
    if markdoc {
        md.disable(&["lheading", "code"], false).expect("rules");
    }
    md
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: mdit_bench INPUTS.jsonl [ROUNDS] [CONFIG]");
    let rounds: usize = args.get(2).map_or(5, |s| s.parse().expect("ROUNDS"));
    let config = args.get(3).map_or("markdoc", String::as_str);
    let docs: Vec<String> = std::fs::read_to_string(path)
        .expect("inputs")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("a JSON object per line");
            v["src"].as_str().expect("src").to_owned()
        })
        .collect();
    let md = parser(config);
    let bytes: usize = docs.iter().map(String::len).sum();

    // one untimed round, which also counts the tokens
    let mut tokens = 0;
    for src in &docs {
        let out = md.parse(src, &mut Env::default());
        tokens += out.len()
            + out
                .iter()
                .filter_map(|t| t.children.as_ref())
                .map(Vec::len)
                .sum::<usize>();
    }
    let start = Instant::now();
    for _ in 0..rounds {
        for src in &docs {
            black_box(md.parse(black_box(src), &mut Env::default()));
        }
    }
    let pass = start.elapsed().as_secs_f64() / rounds as f64;
    let mb = bytes as f64 / 1e6;
    println!(
        "{config}: {} documents, {mb:.2} MB, {tokens} tokens; {:.1} ms per pass ({rounds} rounds): {:.1} MB/s, {:.0} documents/s",
        docs.len(),
        pass * 1e3,
        mb / pass,
        docs.len() as f64 / pass
    );
}
