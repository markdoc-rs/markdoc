//! Upstream Markdoc's own conformance corpus, held as a ratchet.
//!
//! `marktest/tests.yaml` is `spec/marktest/tests.yaml` from Stripe's Markdoc,
//! the JavaScript original this crate ports, vendored unmodified at the
//! revision in `marktest/UPSTREAM.md`. Every case is run through this crate
//! and graded by the rule upstream's own runner applies: `marktest/index.ts`
//! is that runner, vendored beside the corpus, and the line numbers cited
//! below are its.
//!
//! The crate's pipeline is graded through its root paths (upstream's port:
//! its tokenizer, parser, validate, transform and HTML renderer, the
//! runner's steps one for one), against `conformance-baseline.txt`. (The
//! old pipeline's own baseline went with it at the switch-over.)
//!
//! The verdicts are compared with the baseline, and the test fails when the
//! two disagree in either direction:
//!
//! - a case the baseline records as passing that now fails is a regression;
//! - a case the baseline records as failing that now passes is an
//!   improvement nobody has recorded — so the number at the top of the
//!   baseline is always the real one.
//!
//! Every failing case carries its reason in the baseline. A difference from
//! upstream is declared, never discovered.
//!
//! ```text
//! cargo test --test conformance                                  # check
//! MARKDOC_CONFORMANCE_BLESS=1 cargo test --test conformance      # record improvements
//! MARKDOC_CONFORMANCE_BLESS=rebuild cargo test --test conformance # after a corpus refresh
//! ```
//!
//! A regression is never blessed. When one is deliberate, change its line in
//! the baseline to `fail` by hand and write the reason underneath, so the
//! review sees both the lost case and the argument for losing it.

// An integration test is a crate root, so its modules are found beside it,
// not in a directory named after it; these say where they live.
#[path = "conformance/corpus.rs"]
mod corpus;
#[path = "conformance/react.rs"]
mod react;
#[path = "conformance/value.rs"]
mod value;

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};
use value::{V, eq, first_difference};

/// The upstream revision the corpus is vendored from.
const REVISION: &str = "9440a1d45670d367090d66446c5e387028ea9236";
/// `marktest/tests.yaml`'s sha256, pinned here as well as in
/// `marktest/UPSTREAM.md`. This test checks every vendored file against
/// UPSTREAM.md's table with no network; `marktest/verify-vendored.sh`, which
/// CI runs, checks the table against upstream.
const CORPUS_SHA256: &str = "549762cc080c9723c0d2495290de60ede053a0cfe7f46686b385bd1c1cd70b7b";
/// What `rebuild` writes as the reason for a failing case. The check refuses
/// it, so a rebuilt baseline is not done until every failure is explained.
const UNDECLARED: &str = "UNDECLARED: say why this case fails";

fn path(rel: &str) -> String {
    format!("{}/tests/{rel}", env!("CARGO_MANIFEST_DIR"))
}

/// The crate's root paths, graded against `conformance-baseline.txt`.
#[test]
fn upstream_conformance_holds_its_baseline() {
    holds_its_baseline("conformance-baseline.txt", grade_next);
}

fn holds_its_baseline(baseline_file: &str, grade: fn(&corpus::Case) -> Graded) {
    let corpus = check_vendored().unwrap_or_else(|e| panic!("{e}"));
    let cases = corpus::load(std::str::from_utf8(&corpus).expect("UTF-8 corpus"));
    let results: Vec<Graded> = cases.iter().map(grade).collect();

    let file = path(baseline_file);
    let existing = std::fs::read_to_string(&file).ok();
    let bless = std::env::var("MARKDOC_CONFORMANCE_BLESS").unwrap_or_default();
    match bless.as_str() {
        "" | "1" => {}
        "rebuild" => rebuild(existing.as_deref(), &results, &file),
        other => panic!("MARKDOC_CONFORMANCE_BLESS={other:?}: the values are `1` and `rebuild`"),
    }

    let text = existing.unwrap_or_else(|| {
        panic!(
            "{file} does not exist. Create it with \
             MARKDOC_CONFORMANCE_BLESS=rebuild cargo test --test conformance"
        )
    });
    let baseline = Baseline::parse(&text).unwrap_or_else(|e| panic!("{file}: {e}"));

    // The baseline is about this corpus, case for case.
    assert!(
        baseline.revision == REVISION && baseline.corpus_sha256 == CORPUS_SHA256,
        "the baseline records verdicts for corpus {} {}, not {REVISION} {CORPUS_SHA256}; \
         see the refresh steps in marktest/UPSTREAM.md",
        baseline.revision,
        baseline.corpus_sha256
    );
    let listed: Vec<(usize, &str)> = baseline
        .records
        .iter()
        .map(|r| (r.position, r.name.as_str()))
        .collect();
    let corpus_cases: Vec<(usize, &str)> = results
        .iter()
        .map(|g| (g.position, g.name.as_str()))
        .collect();
    assert_eq!(
        listed, corpus_cases,
        "the baseline does not list the corpus's cases, in order"
    );

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    for (record, now) in baseline.records.iter().zip(&results) {
        match (record.pass, now.pass) {
            (true, false) => regressions.push(now),
            (false, true) => improvements.push(now),
            _ => {}
        }
    }

    if bless == "1" {
        assert!(
            regressions.is_empty(),
            "refusing to bless {} regression(s); a regression is written into the baseline by \
             hand, with its reason:\n{}",
            regressions.len(),
            describe(&regressions)
        );
        let records = baseline
            .records
            .iter()
            .zip(&results)
            .map(|(r, now)| Record {
                pass: now.pass,
                reason: if now.pass { None } else { r.reason.clone() },
                ..r.clone()
            })
            .collect::<Vec<_>>();
        // Never write a baseline the next plain run would reject.
        let problems = record_problems(&records, totals_of(&records));
        assert!(problems.is_empty(), "nothing blessed:\n{problems}");
        std::fs::write(&file, render_baseline(&records)).expect("write baseline");
        eprintln!("recorded {} improvement(s) in {file}", improvements.len());
        return;
    }

    let mut problems = String::new();
    if !regressions.is_empty() {
        let _ = writeln!(
            problems,
            "{} case(s) the baseline records as passing now fail:\n{}",
            regressions.len(),
            describe(&regressions)
        );
    }
    if !improvements.is_empty() {
        let _ = writeln!(
            problems,
            "{} case(s) the baseline records as failing now pass. Record them with \
             MARKDOC_CONFORMANCE_BLESS=1 cargo test --test conformance\n{}",
            improvements.len(),
            describe(&improvements)
        );
    }
    problems.push_str(&record_problems(&baseline.records, baseline.totals));
    assert!(problems.is_empty(), "\n{problems}");
}

/// Every vendored file is what UPSTREAM.md's table says it is, the table
/// lists exactly the files that are here, and its row for the corpus carries
/// the hash this test pins. Returns the corpus. No network: whether the
/// table's hashes are upstream's is `verify-vendored.sh`'s question, in CI.
fn check_vendored() -> Result<Vec<u8>, String> {
    let dir = path("marktest");
    let doc = std::fs::read_to_string(path("marktest/UPSTREAM.md"))
        .map_err(|e| format!("marktest/UPSTREAM.md: {e}"))?;
    let revision_line = format!("revision: {REVISION} ");
    if !doc.lines().any(|l| l.starts_with(&revision_line)) {
        return Err(format!(
            "marktest/UPSTREAM.md has no `revision: {REVISION}` line"
        ));
    }
    let row = regex::Regex::new(r"^\| `([^`]+)` \| `([^`]+)` \| `([0-9a-f]{64})` \|$")
        .expect("valid regex");
    let mut listed = std::collections::BTreeMap::new();
    for (n, line) in doc.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if !line.starts_with("| `") {
            continue;
        }
        let m = row.captures(line).ok_or_else(|| {
            format!(
                "marktest/UPSTREAM.md:{}: a file row must read \
                 | `here` | `upstream path` | `sha256` |",
                n + 1
            )
        })?;
        if listed.insert(m[1].to_string(), m[3].to_string()).is_some() {
            return Err(format!("marktest/UPSTREAM.md lists {} twice", &m[1]));
        }
    }
    let mut present = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("{dir}: {e}"))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let name = name.to_string_lossy().into_owned();
        if name != "UPSTREAM.md" && name != "verify-vendored.sh" {
            present.insert(name);
        }
    }
    let listed_names: std::collections::BTreeSet<String> = listed.keys().cloned().collect();
    if listed_names != present {
        return Err(format!(
            "marktest/UPSTREAM.md lists {listed_names:?}, and marktest/ holds {present:?}; \
             every vendored file is listed, and only those"
        ));
    }
    let mut corpus = None;
    for (name, recorded) in &listed {
        let bytes = std::fs::read(format!("{dir}/{name}")).map_err(|e| format!("{name}: {e}"))?;
        let actual = hex(&Sha256::digest(&bytes));
        if &actual != recorded {
            return Err(format!(
                "marktest/{name} is not the vendored file any more (sha256 {actual}, \
                 UPSTREAM.md records {recorded}). Vendored files are never edited: see \
                 marktest/UPSTREAM.md, which also says how to refresh them."
            ));
        }
        if name == "tests.yaml" {
            if actual != CORPUS_SHA256 {
                return Err(format!(
                    "UPSTREAM.md and marktest/tests.yaml agree on {actual}, but this test pins \
                     {CORPUS_SHA256}. Changing the corpus is a refresh: see UPSTREAM.md."
                ));
            }
            corpus = Some(bytes);
        }
    }
    corpus.ok_or_else(|| "marktest/UPSTREAM.md lists no tests.yaml".to_string())
}

/// Write a fresh baseline for a refreshed corpus. A failing case keeps its
/// reason when the same case — same position, same name — failed before
/// too; any other failure is written `UNDECLARED`, so what the refresh
/// changed is what has to be explained. Always ends the run as a failure:
/// a rebuilt baseline is reviewed before it is trusted.
fn rebuild(existing: Option<&str>, results: &[Graded], file: &str) -> ! {
    if let Some(text) = existing {
        // Read the recorded corpus however the rest of the file looks, so
        // a changed revision line alone cannot unlock a rebuild.
        let recorded = text.lines().find_map(|l| {
            l.trim_end()
                .strip_prefix("corpus ")
                .and_then(|r| r.split_whitespace().nth(1).map(str::to_string))
        });
        assert!(
            recorded.as_deref() != Some(CORPUS_SHA256),
            "`rebuild` is for a refreshed corpus, and the baseline already describes this one. \
             Record improvements with MARKDOC_CONFORMANCE_BLESS=1; a deliberate regression is \
             written into the baseline by hand, with its reason."
        );
    }
    let before: Vec<Record> = existing
        .and_then(|t| Baseline::parse(t).ok())
        .map(|b| b.records)
        .unwrap_or_default();
    let (mut carried, mut undeclared) = (0, 0);
    let records = results
        .iter()
        .map(|g| {
            let kept = before
                .iter()
                .find(|r| r.position == g.position && r.name == g.name && !r.pass)
                .and_then(|r| r.reason.clone());
            let reason = match (g.pass, kept) {
                (true, _) => None,
                (false, Some(reason)) => {
                    carried += 1;
                    Some(reason)
                }
                (false, None) => {
                    undeclared += 1;
                    Some(UNDECLARED.to_string())
                }
            };
            Record {
                pass: g.pass,
                position: g.position,
                name: g.name.clone(),
                reason,
            }
        })
        .collect::<Vec<_>>();
    std::fs::write(file, render_baseline(&records)).expect("write baseline");
    panic!(
        "wrote a fresh baseline to {file}: {carried} failing case(s) kept the reason they had \
         at the same position and name, and {undeclared} are `{UNDECLARED}`. Write each of \
         those, check the kept ones still describe the new revision's cases, then run the \
         test again."
    );
}

/// What makes a set of records unfit to be a baseline, whatever the parser
/// does: a failure without a reason in the agreed shape, a pass that still
/// carries one, or totals that are not the records' totals.
fn record_problems(records: &[Record], totals: (usize, usize, usize)) -> String {
    let shape = regex::Regex::new(r"^(divergence|gap): .+ \[[a-z0-9-]+(, [a-z0-9-]+)*\]$")
        .expect("valid regex");
    let mut problems = String::new();
    for r in records {
        let case = format!("case {} {:?}", r.position, r.name);
        match (&r.reason, r.pass) {
            (Some(_), true) => {
                let _ = writeln!(problems, "{case} passes but still carries a reason");
            }
            (None, false) => {
                let _ = writeln!(problems, "{case} fails with no reason given");
            }
            (Some(why), false) if why.to_lowercase().starts_with("undeclared") => {
                let _ = writeln!(problems, "{case} fails and its reason is still `{why}`");
            }
            (Some(why), false) if !shape.is_match(why) => {
                let _ = writeln!(
                    problems,
                    "{case}: a reason reads `divergence: …` or `gap: …` and ends with the \
                     [root-cause] it shares with other cases, not {why:?}"
                );
            }
            _ => {}
        }
    }
    if totals != totals_of(records) {
        let (green, fail, total) = totals_of(records);
        let _ = writeln!(
            problems,
            "the totals line says green {} fail {} total {}, but the records say green {green} \
             fail {fail} total {total}",
            totals.0, totals.1, totals.2
        );
    }
    problems
}

fn totals_of(records: &[Record]) -> (usize, usize, usize) {
    let green = records.iter().filter(|r| r.pass).count();
    (green, records.len() - green, records.len())
}

// ---------------------------------------------------------------------------
// Grading — index.ts's rule, applied to this crate.
// ---------------------------------------------------------------------------

/// index.ts:21-24: the one tokenizer every case shares.
fn next_tokenizer() -> &'static markdoc::tokenizer::Tokenizer {
    use markdoc::tokenizer::{Tokenizer, TokenizerOptions};
    static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();
    TOKENIZER.get_or_init(|| {
        Tokenizer::new(TokenizerOptions {
            allow_indentation: true,
            allow_comments: true,
            ..TokenizerOptions::default()
        })
    })
}

/// index.ts:26-29 `parse(content, slots, file)`.
fn next_parse(
    content: &str,
    slots: bool,
    file: Option<&str>,
) -> Result<markdoc::ast::Node, String> {
    use markdoc::parser::ParserArgs;
    let args = ParserArgs {
        file: file.map(str::to_owned),
        slots,
        ..ParserArgs::default()
    };
    markdoc::parse_with(next_tokenizer(), content, &args)
        .map_err(|e| markdoc::thrown(&e).to_owned())
}

fn grade_next(case: &corpus::Case) -> Graded {
    let (pass, detail) = verdict_next(case);
    Graded {
        position: case.idx + 1,
        line: case.line,
        name: case.name.clone(),
        pass,
        detail,
    }
}

/// index.ts's steps through the crate's root paths. Anything upstream's
/// runner would see thrown fails the case.
fn verdict_next(case: &corpus::Case) -> (bool, String) {
    use markdoc::renderers::html as next_html;
    use markdoc::schema::{Config, Partial};

    // index.ts:108.
    let code = match next_parse(&case.code, case.slots, None) {
        Ok(code) => code,
        Err(e) => return (false, format!("parse threw: {e}")),
    };
    // `test.config`: the YAML as upstream's loader reads it, as JSON.
    let config_json = match &case.config {
        Some(c) => serde_json::to_value(c).expect("a JSON-shaped config"),
        None => serde_json::json!({}),
    };
    let config = match Config::from_json(&config_json) {
        Ok(config) => config,
        Err(e) => return (false, format!("config: {e}")),
    };

    // index.ts:113.
    let validation = match markdoc::validate(&code, &config) {
        Ok(v) => v,
        Err(e) => return (false, format!("validate threw: {}", markdoc::thrown(&e))),
    };

    // index.ts:116-123.
    if let Some(expected) = &case.expected_error {
        let got = validation
            .iter()
            .map(|e| e.error.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let (expected, got) = (expected.trim(), got.trim());
        return (
            expected == got,
            format!("validation: expected {expected:?}, got {got:?}"),
        );
    }

    if case.html {
        // index.ts:54-58: the config as given (partials unparsed), the HTML
        // renderer, both sides trimmed.
        let output = markdoc::transform(&code, &config)
            .and_then(|tree| next_html::render(&tree))
            .map_err(|e| markdoc::thrown(&e).to_owned());
        let got = match output {
            Ok(html) => html,
            Err(e) => return (false, format!("transform or html threw: {e}")),
        };
        let expected = case
            .expected
            .as_ref()
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .trim();
        let got = got.trim();
        return (
            expected == got,
            format!(
                "html: expected {:?}, got {:?}",
                value::elide(expected),
                value::elide(got)
            ),
        );
    }

    // index.ts:37-44: the partials parsed, transform, the React renderer and
    // shim; then index.ts:61-63 `diffJson(expected || {}, output.children || [])`.
    let mut with_partials = config.clone();
    let mut partials = indexmap::IndexMap::new();
    if let Some(serde_json::Value::Object(sources)) = config_json.get("partials") {
        for (file, src) in sources {
            match next_parse(src.as_str().unwrap_or_default(), false, Some(file)) {
                Ok(node) => {
                    partials.insert(file.clone(), Partial::Node(Arc::new(node)));
                }
                Err(e) => return (false, format!("partial {file:?} threw: {e}")),
            }
        }
    }
    with_partials.partials = Some(partials);
    let tree = match markdoc::transform(&code, &with_partials).map(|tree| tree.to_render()) {
        Ok(Ok(tree)) => tree,
        Ok(Err(e)) => return (false, format!("the render tree's JSON threw: {e}")),
        Err(e) => return (false, format!("transform threw: {}", markdoc::thrown(&e))),
    };
    let expected = match &case.expected {
        None | Some(serde_yaml::Value::Null) => V::Obj(Vec::new()),
        Some(e) => V::from_yaml(e),
    };
    let got = react::article_children(&tree);
    let pass = eq(&expected, &got);
    let detail = first_difference("", &expected, &got).unwrap_or_default();
    (pass, detail)
}

struct Graded {
    /// 1-based position in the corpus.
    position: usize,
    /// 1-based line of the case in tests.yaml.
    line: usize,
    name: String,
    pass: bool,
    /// For a failure: where it differs, for the report.
    detail: String,
}

fn describe(cases: &[&Graded]) -> String {
    cases
        .iter()
        .map(|g| {
            format!(
                "  case {} {:?} (marktest/tests.yaml:{}): {}",
                g.position, g.name, g.line, g.detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// The baseline file.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Record {
    pass: bool,
    position: usize,
    name: String,
    reason: Option<String>,
}

struct Baseline {
    revision: String,
    corpus_sha256: String,
    /// green, fail, total — as the totals line states them.
    totals: (usize, usize, usize),
    records: Vec<Record>,
}

impl Baseline {
    /// The file's structure. Whether it describes this corpus, and whether
    /// its records are fit to be a baseline, are the callers' checks.
    fn parse(text: &str) -> Result<Self, String> {
        let mut corpus: Option<(String, String)> = None;
        let mut totals = None;
        let mut records: Vec<Record> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let n = n + 1;
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("corpus ") {
                let fields: Vec<&str> = rest.split_whitespace().collect();
                let [revision, sha] = fields[..] else {
                    return Err(format!("line {n}: `corpus <revision> <sha256>`"));
                };
                if corpus.replace((revision.into(), sha.into())).is_some() {
                    return Err(format!("line {n}: a second `corpus` line"));
                }
            } else if let Some(rest) = line.strip_prefix("totals ") {
                let words: Vec<&str> = rest.split_whitespace().collect();
                let ["green", green, "fail", fail, "total", total] = words[..] else {
                    return Err(format!("line {n}: `totals green N fail N total N`"));
                };
                let number = |w: &str| {
                    w.parse::<usize>()
                        .map_err(|_| format!("line {n}: {w:?} is not a count"))
                };
                if totals
                    .replace((number(green)?, number(fail)?, number(total)?))
                    .is_some()
                {
                    return Err(format!("line {n}: a second `totals` line"));
                }
            } else if line.starts_with(char::is_whitespace) {
                let Some(last) = records.last_mut().filter(|r| !r.pass) else {
                    return Err(format!(
                        "line {n}: a reason line must follow a `fail` record"
                    ));
                };
                let reason = last.reason.get_or_insert_with(String::new);
                if !reason.is_empty() {
                    reason.push(' ');
                }
                reason.push_str(line.trim());
            } else {
                // `pass   7  Name of the case`: the number is padded, so split on
                // runs of whitespace, and keep the name's own spacing.
                let (verdict, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
                let rest = rest.trim_start();
                let (position, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let name = name.trim();
                let pass = match verdict {
                    "pass" => true,
                    "fail" => false,
                    _ => return Err(format!("line {n}: expected `pass` or `fail`: {line:?}")),
                };
                let position = position
                    .parse()
                    .map_err(|_| format!("line {n}: no case number: {line:?}"))?;
                records.push(Record {
                    pass,
                    position,
                    name: name.to_string(),
                    reason: None,
                });
            }
        }
        let (revision, corpus_sha256) = corpus.ok_or("no `corpus` line")?;
        Ok(Self {
            revision,
            corpus_sha256,
            totals: totals.ok_or("no `totals` line")?,
            records,
        })
    }
}

fn render_baseline(records: &[Record]) -> String {
    let green = records.iter().filter(|r| r.pass).count();
    let mut out = String::from(
        "# Upstream Markdoc's conformance corpus, case by case: what this crate passes,
# and for every case it fails, why.
#
# The corpus is spec/marktest/tests.yaml from markdoc/markdoc, vendored
# unmodified in marktest/ (see marktest/UPSTREAM.md), graded by the rule
# upstream's own runner applies (marktest/index.ts). tests/conformance.rs
# fails when this file and the parser disagree, in either direction: a `pass`
# that now fails is a regression, a `fail` that now passes is an improvement
# to record with
#
#     MARKDOC_CONFORMANCE_BLESS=1 cargo test --test conformance
#
# A regression is never blessed. If one is deliberate, change its line to
# `fail` by hand and write the reason under it.
#
# A reason starts `divergence:` when the difference is a decision this crate
# made, and `gap:` when nobody decided it. The bracketed word at the end
# names the root cause the case shares with others.

",
    );
    let _ = writeln!(out, "corpus {REVISION} {CORPUS_SHA256}");
    let _ = writeln!(
        out,
        "totals green {green} fail {} total {}\n",
        records.len() - green,
        records.len()
    );
    for r in records {
        let _ = writeln!(
            out,
            "{} {:>3}  {}",
            if r.pass { "pass" } else { "fail" },
            r.position,
            r.name
        );
        if let Some(reason) = &r.reason {
            let _ = writeln!(out, "          {reason}");
        }
    }
    out
}
