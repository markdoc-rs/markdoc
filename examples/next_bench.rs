//! Times `markdoc::next`'s stages over input sets: each stage over every document of a file
//! at once, a number of rounds, the fastest round per stage (to compare builds, and with node:
//! `findings/conformance/work/depth/node_bench.cjs`).
//!
//! ```text
//! next_bench ROUNDS FILE.jsonl...
//! ```
//!
//! Each line of a file is `{"src": ...}` (or `"code"`); the tokenizer has `allowIndentation`
//! and `allowComments` on (the harness's default) and the config is the default one. Prints
//! one JSON line per file: `{"file", "docs", "tokenize", "parse", "validate", "transform",
//! "html"}`, each `[wall ms, cpu ms]`, the CPU time this thread spent (from
//! `/proc/thread-self/schedstat`, where there is one: on a busy machine it is what the work
//! took, the wall time is what the scheduler gave it). A document whose stage throws is left
//! out of the stages after it.

use markdoc::next::index;
use markdoc::next::parser::ParserArgs;
use markdoc::next::renderers::html;
use markdoc::next::schema::Config;
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
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
    let cpu = (cpu_ns() - cpu) as f64 / 1e6;
    (out, [wall.elapsed().as_secs_f64() * 1e3, cpu])
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rounds: usize = args[0].parse().expect("ROUNDS");
    let tokenizer = Tokenizer::new(TokenizerOptions {
        allow_indentation: true,
        allow_comments: true,
        ..TokenizerOptions::default()
    });
    let config = Config::default();
    let parser_args = ParserArgs::default();
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
        let mut best = [[f64::MAX; 2]; 5];
        // NEXT_BENCH_ONLY=tokenize times the tokenizer alone (each document's tokens dropped
        // as they are made).
        let only_tokenize = std::env::var("NEXT_BENCH_ONLY").as_deref() == Ok("tokenize");
        for _ in 0..rounds {
            if only_tokenize {
                let (n, t0) = timed(|| {
                    docs.iter()
                        .map(|d| tokenizer.tokenize(d).map(|t| t.len()).unwrap_or(0))
                        .sum::<usize>()
                });
                std::hint::black_box(n);
                best[0][0] = best[0][0].min(t0[0]);
                best[0][1] = best[0][1].min(t0[1]);
                continue;
            }
            let (tokens, t0) = timed(|| {
                docs.iter()
                    .map(|d| tokenizer.tokenize(d).ok())
                    .collect::<Vec<_>>()
            });
            let (asts, t1) = timed(|| {
                tokens
                    .iter()
                    .zip(&docs)
                    .map(|(t, d)| {
                        t.as_ref()
                            .and_then(|t| index::parse_tokens(t, d, &parser_args).ok())
                    })
                    .collect::<Vec<_>>()
            });
            drop(tokens);
            let (errors, t2) = timed(|| {
                asts.iter()
                    .flatten()
                    .map(|a| index::validate(a, &config).map(|e| e.len()).unwrap_or(0))
                    .sum::<usize>()
            });
            let (trees, t3) = timed(|| {
                asts.iter()
                    .flatten()
                    .filter_map(|a| index::transform(a, &config).ok())
                    .collect::<Vec<_>>()
            });
            let (bytes, t4) = timed(|| {
                trees
                    .iter()
                    .map(|t| html::render(t).map(|h| h.len()).unwrap_or(0))
                    .sum::<usize>()
            });
            std::hint::black_box((errors, bytes));
            drop((asts, trees));
            for (b, t) in best.iter_mut().zip([t0, t1, t2, t3, t4]) {
                b[0] = b[0].min(t[0]);
                b[1] = b[1].min(t[1]);
            }
        }
        let ms = |t: [f64; 2]| [(t[0] * 1e3).round() / 1e3, (t[1] * 1e3).round() / 1e3];
        println!(
            "{}",
            serde_json::json!({
                "file": file.rsplit('/').next().unwrap_or(file),
                "docs": docs.len(),
                "tokenize": ms(best[0]),
                "parse": ms(best[1]),
                "validate": ms(best[2]),
                "transform": ms(best[3]),
                "html": ms(best[4]),
            })
        );
    }
}
