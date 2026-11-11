//! The api stages' time over a set of documents, as
//! findings/conformance/work/api/bench/api_bench.cjs measures node's:
//! `api_bench INPUTS.jsonl [ROUNDS]`. Each document is parsed (`allowIndentation` and
//! `allowComments` on) and transformed once, untimed; then, timed, every AST is formatted,
//! every render tree written by `react_static` and rendered by `react` (a React that builds
//! plain values). Prints, per stage, the best of ROUNDS (default 7) rounds in ms.

use markdoc::next::formatter::{self, FormatOptions};
use markdoc::next::index;
use markdoc::next::parser::ParserArgs;
use markdoc::next::renderers::{react, react_static};
use markdoc::next::schema::Config;
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
use std::io::BufRead;
use std::time::Instant;

/// A React whose elements are counted (what a real one would build, cheaply).
struct Counter(usize);

impl react::React for Counter {
    type Element = usize;
    type Component = ();
    fn create_element(
        &mut self,
        _: react::ElementType<'_, ()>,
        _: Option<react::Prop<'_, usize>>,
        children: Vec<react::Rendered<'_, usize>>,
    ) -> usize {
        self.0 += 1;
        self.0 + children.len()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rounds: usize = args.get(2).map_or(7, |r| r.parse().unwrap());
    let file = std::fs::File::open(&args[1]).expect("inputs");
    let tokenizer = Tokenizer::new(TokenizerOptions {
        allow_indentation: true,
        allow_comments: true,
        ..TokenizerOptions::default()
    });
    let config = Config::default();
    let mut asts = Vec::new();
    let mut trees = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let doc: serde_json::Value = serde_json::from_str(&line).unwrap();
        let src = doc["code"].as_str().or(doc["src"].as_str()).unwrap_or("");
        let Ok(ast) = index::parse_with(&tokenizer, src, &ParserArgs::default()) else {
            continue;
        };
        let Ok(tree) = index::transform(&ast, &config) else {
            continue;
        };
        asts.push(ast);
        trees.push(tree);
    }
    let time = |label: &str, f: &dyn Fn() -> usize| {
        let mut best = f64::INFINITY;
        let mut sink = 0;
        for _ in 0..rounds {
            let t = Instant::now();
            sink += f();
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
        }
        println!(
            "{{\"stage\": \"{label}\", \"docs\": {}, \"best_ms\": {best:.2}, \"sink\": {sink}}}",
            asts.len()
        );
    };
    let options = FormatOptions::default();
    time("format", &|| {
        asts.iter()
            .map(|a| formatter::format(a, &options).map_or(1, |s| s.len()))
            .sum()
    });
    let static_options = react_static::StaticOptions::default();
    time("reactStatic", &|| {
        trees
            .iter()
            .map(|t| react_static::render(t, &static_options).map_or(1, |s| s.len()))
            .sum()
    });
    time("react", &|| {
        let mut counter = Counter(0);
        let options = react::RenderOptions::default();
        trees
            .iter()
            .map(|t| react::render(t, &mut counter, &options).map_or(1, |_| 1))
            .sum()
    });
}
