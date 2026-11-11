//! The next layer's stages timed over an input set, without the harness's JSON: each stage of
//! every document (tokenize, parse, validate, transform, render), the best of N runs.
//!
//! ```text
//! cargo run --release --example stage_bench -- INPUT.jsonl [RUNS]
//! ```
//!
//! INPUT is compare.py's JSONL (`{id, code or src, config?, slots?, ...}`); the tokenizer has
//! `allowIndentation` and `allowComments` on, as compare.py's runs. A document whose config
//! the port does not read, or whose stage throws, is timed as far as it goes.

use markdoc::markdown_it::Token;
use markdoc::next::index;
use markdoc::next::parser::ParserArgs;
use markdoc::next::renderers::html;
use markdoc::next::schema::Config;
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("an input file");
    let runs: usize = args
        .next()
        .map_or(5, |r| r.parse().expect("a number of runs"));
    let text = std::fs::read_to_string(&path).expect("the input");
    let docs: Vec<serde_json::Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect();
    let mut options = serde_json::Map::new();
    options.insert("allowIndentation".into(), true.into());
    options.insert("allowComments".into(), true.into());
    let tokenizer = Tokenizer::new(TokenizerOptions::from_json(&options).expect("options"));
    let codes: Vec<&str> = docs
        .iter()
        .map(|d| {
            d.get("code")
                .or(d.get("src"))
                .and_then(|c| c.as_str())
                .unwrap_or_default()
        })
        .collect();
    let configs: Vec<Option<Config>> = docs
        .iter()
        .map(|d| {
            let empty = serde_json::Value::Object(serde_json::Map::new());
            Config::from_json(d.get("config").unwrap_or(&empty)).ok()
        })
        .collect();
    let args: Vec<ParserArgs> = docs
        .iter()
        .map(|d| ParserArgs {
            slots: d.get("slots").and_then(|s| s.as_bool()).unwrap_or(false),
            ..ParserArgs::default()
        })
        .collect();

    let mut best = [Duration::MAX; 5];
    for _ in 0..runs {
        let t = Instant::now();
        let tokens: Vec<Option<Vec<Token>>> =
            codes.iter().map(|c| tokenizer.tokenize(c).ok()).collect();
        let d0 = t.elapsed();

        let t = Instant::now();
        let asts: Vec<_> = tokens
            .iter()
            .zip(&codes)
            .zip(&args)
            .map(|((t, c), a)| t.as_ref().and_then(|t| index::parse_tokens(t, c, a).ok()))
            .collect();
        let d1 = t.elapsed();

        let t = Instant::now();
        let mut errors = 0usize;
        for (ast, config) in asts.iter().zip(&configs) {
            if let (Some(ast), Some(config)) = (ast, config) {
                errors += index::validate(ast, config).map(|e| e.len()).unwrap_or(0);
            }
        }
        let d2 = t.elapsed();

        let t = Instant::now();
        let trees: Vec<_> = asts
            .iter()
            .zip(&configs)
            .map(|(ast, config)| match (ast, config) {
                (Some(ast), Some(config)) => index::transform(ast, config).ok(),
                _ => None,
            })
            .collect();
        let d3 = t.elapsed();

        let t = Instant::now();
        let mut bytes = 0usize;
        for tree in trees.iter().flatten() {
            bytes += html::render(tree).map(|h| h.len()).unwrap_or(0);
        }
        let d4 = t.elapsed();
        std::hint::black_box((errors, bytes));
        for (b, d) in best.iter_mut().zip([d0, d1, d2, d3, d4]) {
            *b = (*b).min(d);
        }
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{}: {} documents: tokenize {:.1} ms, parse {:.1} ms, validate {:.1} ms, transform {:.1} ms, render {:.1} ms",
        path.rsplit('/').next().unwrap_or(&path),
        docs.len(),
        ms(best[0]),
        ms(best[1]),
        ms(best[2]),
        ms(best[3]),
        ms(best[4])
    );
}
