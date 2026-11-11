//! Every stage's time over input sets, as `findings/conformance/work/final/node_bench.cjs`
//! measures node's: `final_bench [--spans] ROUNDS FILE.jsonl...`. Each stage runs over every
//! document of a file at once, ROUNDS rounds in one process; per file and stage it prints the
//! first round (a fresh process) and the best (warmed up), each `[wall ms, cpu ms]` (the CPU
//! time of this thread, from `/proc/thread-self/schedstat`: on a busy machine what the work
//! took).
//!
//! The tokenizer has `allowIndentation` and `allowComments` on and the config is the default
//! one (the harness's defaults); `react` renders with a React that counts what it is asked to
//! create. A document whose stage fails is left out of the stages after it. Stages: tokenize,
//! parse (`parse_tokens`, from the tokens), validate, transform, html, format, reactStatic,
//! react. The parse is upstream's (`ParserArgs::default()`, no spans); with `--spans` it also
//! finds this crate's own positions (`ParserArgs::spans`), as a linter's parse does.

use markdoc::formatter::{self, FormatOptions};
use markdoc::parser::ParserArgs;
use markdoc::renderers::{html, react, react_static};
use markdoc::schema::Config;
use markdoc::tokenizer::{Tokenizer, TokenizerOptions};
use std::time::Instant;

/// The CPU time this thread has run, in nanoseconds (0 where the kernel does not say).
fn cpu_ns() -> u64 {
    std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

/// Runs `f`, and how long it took: (wall, CPU) in milliseconds.
fn timed<T>(f: impl FnOnce() -> T) -> (T, [f64; 2]) {
    let (wall, cpu) = (Instant::now(), cpu_ns());
    let out = f();
    let cpu = cpu_ns().saturating_sub(cpu) as f64 / 1e6;
    (out, [wall.elapsed().as_secs_f64() * 1e3, cpu])
}

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

const STAGES: [&str; 8] = [
    "tokenize",
    "parse",
    "validate",
    "transform",
    "html",
    "format",
    "reactStatic",
    "react",
];

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let spans = args.first().is_some_and(|a| a == "--spans");
    if spans {
        args.remove(0);
    }
    let rounds: usize = args[0].parse().expect("ROUNDS");
    let tokenizer = Tokenizer::new(TokenizerOptions {
        allow_indentation: true,
        allow_comments: true,
        ..TokenizerOptions::default()
    });
    let config = Config::default();
    let parser_args = ParserArgs {
        spans,
        ..ParserArgs::default()
    };
    let format_options = FormatOptions::default();
    let static_options = react_static::StaticOptions::default();
    for file in &args[1..] {
        let text = std::fs::read_to_string(file).expect("the file");
        let docs: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("JSON");
                v.get("src")
                    .or_else(|| v.get("code"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_owned()
            })
            .collect();
        let mut first: [Option<[f64; 2]>; 8] = [None; 8];
        let mut best = [[f64::MAX; 2]; 8];
        let mut sink = 0usize;
        for _ in 0..rounds {
            let mut times = [[0.0; 2]; 8];
            let (tokens, t) = timed(|| {
                docs.iter()
                    .map(|d| tokenizer.tokenize(d).ok())
                    .collect::<Vec<_>>()
            });
            times[0] = t;
            let (asts, t) = timed(|| {
                tokens
                    .iter()
                    .zip(&docs)
                    .map(|(t, d)| {
                        t.as_ref()
                            .and_then(|t| markdoc::parse_tokens(t, d, &parser_args).ok())
                    })
                    .collect::<Vec<_>>()
            });
            times[1] = t;
            drop(tokens);
            let (n, t) = timed(|| {
                asts.iter()
                    .flatten()
                    .map(|a| markdoc::validate(a, &config).map_or(0, |e| e.len()))
                    .sum::<usize>()
            });
            times[2] = t;
            sink += n;
            let (trees, t) = timed(|| {
                asts.iter()
                    .flatten()
                    .filter_map(|a| markdoc::transform(a, &config).ok())
                    .collect::<Vec<_>>()
            });
            times[3] = t;
            let (n, t) = timed(|| {
                trees
                    .iter()
                    .map(|t| html::render(t).map_or(0, |h| h.len()))
                    .sum::<usize>()
            });
            times[4] = t;
            sink += n;
            let (n, t) = timed(|| {
                asts.iter()
                    .flatten()
                    .map(|a| formatter::format(a, &format_options).map_or(0, |s| s.len()))
                    .sum::<usize>()
            });
            times[5] = t;
            sink += n;
            let (n, t) = timed(|| {
                trees
                    .iter()
                    .map(|t| react_static::render(t, &static_options).map_or(0, |s| s.len()))
                    .sum::<usize>()
            });
            times[6] = t;
            sink += n;
            let (n, t) = timed(|| {
                let mut counter = Counter(0);
                let options = react::RenderOptions::default();
                trees
                    .iter()
                    .map(|t| react::render(t, &mut counter, &options).map_or(0, |_| 1))
                    .sum::<usize>()
            });
            times[7] = t;
            sink += n;
            drop((asts, trees));
            for i in 0..8 {
                first[i].get_or_insert(times[i]);
                best[i][0] = best[i][0].min(times[i][0]);
                best[i][1] = best[i][1].min(times[i][1]);
            }
        }
        std::hint::black_box(sink);
        let ms = |t: [f64; 2]| [(t[0] * 1e3).round() / 1e3, (t[1] * 1e3).round() / 1e3];
        let mut out = serde_json::Map::new();
        out.insert("side".into(), "port".into());
        out.insert(
            "file".into(),
            file.rsplit('/').next().unwrap_or(file).into(),
        );
        out.insert("docs".into(), docs.len().into());
        for (i, stage) in STAGES.iter().enumerate() {
            out.insert(
                (*stage).into(),
                serde_json::json!({"first": ms(first[i].unwrap_or([0.0; 2])), "best": ms(best[i])}),
            );
        }
        println!("{}", serde_json::Value::Object(out));
    }
}
