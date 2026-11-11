//! The Rust half of the next layer's comparison harness (tests/next/compare.py runs it):
//! requests on stdin, one per line, through the port; one JSON answer per line on stdout,
//! written as `JSON.stringify` writes it.
//!
//! ```text
//! cargo run --release --example next_compare < requests.jsonl
//! cargo run --release --example next_compare -- --conformance > conformance.jsonl
//! ```
//!
//! `--conformance` prints upstream's conformance corpus (tests/marktest/tests.yaml) as oracle
//! requests: `{id: "NNN name", code, config?, slots?}`.
//!
//! A request: `{id, stage or stages, code, file?, slots?, tokenizer: {...}, parse: {...},
//! config?, partials?: {name: source}}`. `stage` is one stage, `stages` a list of them, all
//! answered from one run of the pipeline. End to end is the default: the source goes through
//! the port's tokenizer, then its parser, then validate, transform and the HTML renderer
//! (partials parsed the same way, with their own name as `file`), as tests/next/oracle.cjs
//! runs upstream. In from-oracle mode the request also carries upstream's output for the
//! stage before, which then stands in for the port's: `tokens` (upstream's tokens, parsed
//! instead of tokenizing), `ast` and `partialAsts` (upstream Node JSON, transformed or
//! validated instead of parsing), `tree` (upstream's render tree, rendered instead of the
//! port's own values).
//!
//! Every answer is upstream's JSON of the value, whole, keys in upstream's order:
//!
//! | stage | answer key | from |
//! | --- | --- | --- |
//! | `tokens` | `tokens` (`compare::tokens_json`) | `plugins::tokenize` |
//! | `ast` | `ast` (`compare::node_json`, `lines` and `location` included) | `parser::parse` over the tokens |
//! | `errors` | `errors` (`compare::errors_json`: `[{type, lines, location, error}]`) | `semantics::validate` over the AST |
//! | `transform` | `transformed` (`JSON.stringify` of the values: `value::json_stringify`, written as it is) | `semantics::transform` over the AST and the partials' ASTs |
//! | `html` | `html` | `semantics::html` over the transform's values (or `semantics::html_tree` over `tree`, or `semantics::html` over `renderTree`) |
//! | `roundtrip` | `{tokens?, ast?, transformed?, partials?}` | none: each of upstream's outputs given, read and written back |
//! | `format` | `formatted` | `api::format` over the AST, with the request's `formatOptions` |
//! | `reactStatic` | `reactStatic` | `api::react_static` over the transform's values (or `tree`, read as `JSON.parse` reads it) |
//! | `react` | `react` (`{calls, result}`, JSON text) | `api::react` over the same, with the request's `components` |
//!
//! Where a step throws, the stage that threw is in `failed`, `{stage: "Name: message"}`,
//! with upstream's stage names (`tokens`, `ast`, `partials`, `transformed`, `errors`,
//! `html`); a stage that needs it is left out, as the oracle leaves it out. `threw` is the
//! first of them. The message is what upstream's would be, `String(error)` in JS: the
//! tokenizer's `TokenizeError` as `Name: message`, the rest as `index::thrown` gives it;
//! `panic: ...` for a panic (never right) and `harness: ...` for a converter's failure.

mod api;
mod parser;
mod plugins;
mod semantics;

use indexmap::IndexMap;
use markdoc::ast::Node;
use markdoc::markdown_it::Token;
use markdoc::next::compare as cmp;
use markdoc::next::schema::value::{Value as JsValue, json_stringify};
use markdoc::types::RenderableTreeNode;
use serde_json::{Map, Value};
use std::cell::OnceCell;
use std::io::{BufRead, BufWriter, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

/// Why a step has no result: the stage that threw, and what (`Name: message`).
#[derive(Clone)]
struct Fail {
    stage: &'static str,
    message: String,
}

type Step<T> = Result<T, Fail>;

fn at<T>(stage: &'static str, result: Result<T, String>) -> Step<T> {
    result.map_err(|message| Fail { stage, message })
}

fn convert<T>(stage: &'static str, result: Result<T, String>) -> Step<T> {
    at(stage, result.map_err(|e| format!("harness: {e}")))
}

/// A stage's output: JSON, or JSON text written into the answer as it is (`None`: the key is
/// left out, as `JSON.stringify` leaves out `undefined`), or the error `JSON.stringify` threw
/// writing a value the stage made (the oracle's `unwritable`: the stage did not throw).
enum Output {
    Json(Value),
    Text(Option<String>),
    Unwritable(String),
}

/// What the html stage renders: the transform's own values, or upstream's tree as JSON, or
/// the request's `renderTree` (as `JSON.parse` reads it).
enum Tree<'a> {
    Values(&'a JsValue),
    Json(RenderableTreeNode),
    Given(JsValue),
}

/// One request, each stage run at most once.
struct Request<'a> {
    req: &'a Map<String, Value>,
    tokens: OnceCell<Step<Vec<Token>>>,
    ast: OnceCell<Step<Node>>,
    partials: OnceCell<Step<IndexMap<String, Node>>>,
    values: OnceCell<Step<JsValue>>,
}

impl<'a> Request<'a> {
    fn new(req: &'a Map<String, Value>) -> Self {
        Request {
            req,
            tokens: OnceCell::new(),
            ast: OnceCell::new(),
            partials: OnceCell::new(),
            values: OnceCell::new(),
        }
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.req.get(key).and_then(Value::as_str)
    }

    fn object(&self, key: &str) -> Map<String, Value> {
        self.req
            .get(key)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    fn config(&self) -> Value {
        self.req
            .get("config")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()))
    }

    /// `{file, slots: !!slots, location: true, ...parse}`.
    fn parse_args(&self, file: Option<&str>) -> Map<String, Value> {
        let mut args = Map::new();
        if let Some(file) = file {
            args.insert("file".into(), file.into());
        }
        let slots = self
            .req
            .get("slots")
            .is_some_and(|s| s.as_bool() != Some(false) && !s.is_null());
        args.insert("slots".into(), slots.into());
        args.insert("location".into(), true.into());
        for (k, v) in self.object("parse") {
            args.insert(k, v);
        }
        args
    }

    fn tokens(&self) -> Result<&Vec<Token>, Fail> {
        self.tokens
            .get_or_init(|| match self.req.get("tokens") {
                // The oracle revives a request's tokens with `Ast.fromJSON` (tests/next/
                // oracle.cjs), which throws where a `$$mdtype` names no class it can make.
                Some(tokens) => at("tokens", cmp::revive_check(tokens))
                    .and_then(|()| convert("tokens", cmp::tokens_from_json(tokens))),
                None => at(
                    "tokens",
                    plugins::tokenize(
                        self.str("code").unwrap_or_default(),
                        &self.object("tokenizer"),
                    ),
                ),
            })
            .as_ref()
            .map_err(Fail::clone)
    }

    fn ast(&self) -> Result<&Node, Fail> {
        self.ast
            .get_or_init(|| match self.req.get("ast") {
                // Revived by the oracle with `Ast.fromJSON` too (see `tokens`).
                Some(ast) => at("ast", cmp::revive_check(ast))
                    .and_then(|()| convert("ast", cmp::node_from_json(ast))),
                None => at(
                    "ast",
                    parser::parse(
                        self.tokens()?,
                        self.str("code").unwrap_or_default(),
                        &self.parse_args(self.str("file")),
                    ),
                ),
            })
            .as_ref()
            .map_err(Fail::clone)
    }

    fn partials(&self) -> Result<&IndexMap<String, Node>, Fail> {
        self.partials
            .get_or_init(|| {
                let mut out = IndexMap::new();
                if let Some(asts) = self.req.get("partialAsts").and_then(Value::as_object) {
                    for (name, ast) in asts {
                        out.insert(name.clone(), convert("partials", cmp::node_from_json(ast))?);
                    }
                    return Ok(out);
                }
                // `Object.entries(req.partials || config.partials || {})`: an object's
                // entries, an array's items, a string's characters (compare.py passes
                // `config.partials` as `partials`).
                let entries: Vec<(String, Value)> = match self.req.get("partials") {
                    Some(Value::Object(o)) => {
                        o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                    }
                    Some(Value::Array(items)) => items
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (i.to_string(), v.clone()))
                        .collect(),
                    Some(Value::String(s)) => s
                        .encode_utf16()
                        .enumerate()
                        .map(|(i, u)| (i.to_string(), String::from_utf16_lossy(&[u]).into()))
                        .collect(),
                    _ => Vec::new(),
                };
                for (name, src) in entries {
                    let src = src.as_str().unwrap_or_default();
                    let tokens = at(
                        "partials",
                        plugins::tokenize(src, &self.object("tokenizer")),
                    )?;
                    let node = at(
                        "partials",
                        parser::parse(&tokens, src, &self.parse_args(Some(&name))),
                    )?;
                    out.insert(name, node);
                }
                Ok(out)
            })
            .as_ref()
            .map_err(Fail::clone)
    }

    /// The transform's values (the tree as JS holds it).
    fn values(&self) -> Result<&JsValue, Fail> {
        self.values
            .get_or_init(|| {
                let (ast, partials) = (self.ast()?, self.partials()?);
                at(
                    "transformed",
                    semantics::transform(ast, &self.config(), partials),
                )
            })
            .as_ref()
            .map_err(Fail::clone)
    }

    fn tree(&self) -> Step<Tree<'_>> {
        if let Some(tree) = self.req.get("renderTree") {
            return Ok(Tree::Given(JsValue::from_json(tree)));
        }
        match self.req.get("tree") {
            Some(tree) => Ok(Tree::Json(convert(
                "transformed",
                cmp::render_from_json(tree),
            )?)),
            None => Ok(Tree::Values(self.values()?)),
        }
    }

    /// The render tree the React stages render: the request's `renderTree`, or `tree` (from
    /// the oracle), as `JSON.parse` reads it (upstream's renderers read a `{"$$mdtype":
    /// "Tag"}` object as a tag), or the transform's values.
    fn js_tree(&self) -> Step<JsValue> {
        match self.req.get("renderTree").or_else(|| self.req.get("tree")) {
            Some(tree) => Ok(JsValue::from_json(tree)),
            None => Ok(self.values()?.clone()),
        }
    }

    /// One stage's output, as `(answer key, output)`.
    fn stage(&self, stage: &str) -> Step<(&'static str, Output)> {
        Ok(match stage {
            "tokens" => (
                "tokens",
                Output::Json(match self.req.get("tokens") {
                    // A request's own tokens are written as the oracle writes them: revived
                    // by `Ast.fromJSON`, then `JSON.stringify`'d, with the keys it gave.
                    Some(request) => {
                        self.tokens()?;
                        cmp::request_tokens_json_revived(request)
                    }
                    None => cmp::tokens_json(self.tokens()?),
                }),
            ),
            // Where node's `JSON.stringify` of the AST throws, the AST is "unwritable".
            "ast" => (
                "ast",
                match cmp::node_json_text(self.ast()?) {
                    Ok(text) => Output::Text(Some(text)),
                    Err(message) => Output::Unwritable(message),
                },
            ),
            "errors" => {
                let records = at("errors", semantics::validate(self.ast()?, &self.config()))?;
                ("errors", Output::Json(cmp::errors_json(&records)))
            }
            // What the oracle writes: `JSON.stringify(tree)`, with its depth limit. Where it
            // throws (past the limit, or a `toJSON` that throws), the tree is "unwritable",
            // as the oracle reports it: the transform itself did not throw.
            "transform" => (
                "transformed",
                match json_stringify(self.values()?, "") {
                    Ok(json) => Output::Text(json),
                    Err(message) => Output::Unwritable(message),
                },
            ),
            "html" => {
                let html = match self.tree()? {
                    Tree::Values(values) => semantics::html(values),
                    Tree::Json(tree) => semantics::html_tree(&tree),
                    Tree::Given(tree) => semantics::html(&tree),
                };
                ("html", Output::Json(Value::String(at("html", html)?)))
            }
            "format" => {
                let options = self.object("formatOptions");
                (
                    "formatted",
                    Output::Json(Value::String(at(
                        "formatted",
                        api::format(self.ast()?, &options),
                    )?)),
                )
            }
            "reactStatic" => {
                let tree = self.js_tree()?;
                (
                    "reactStatic",
                    Output::Json(Value::String(at("reactStatic", api::react_static(&tree))?)),
                )
            }
            "react" => {
                let tree = self.js_tree()?;
                let json = at("react", api::react(&tree, self.req.get("components")))?;
                ("react", Output::Text(Some(json)))
            }
            other => {
                return Err(Fail {
                    stage: "harness",
                    message: format!(
                        "harness: unknown stage {other:?} (tokens, ast, errors, transform, html, \
                         format, reactStatic, react, roundtrip)"
                    ),
                });
            }
        })
    }
}

/// The converters alone: each of upstream's outputs given, read and written back.
fn roundtrip(req: &Map<String, Value>, out: &mut Map<String, Value>) -> Step<()> {
    if let Some(tokens) = req.get("tokens") {
        let tokens = convert("tokens", cmp::tokens_from_json(tokens))?;
        out.insert("tokens".into(), cmp::tokens_json(&tokens));
    }
    if let Some(ast) = req.get("ast") {
        let node = convert("ast", cmp::node_from_json(ast))?;
        out.insert("ast".into(), cmp::node_json(&node));
    }
    if let Some(tree) = req.get("transformed") {
        let tree = convert("transformed", cmp::render_from_json(tree))?;
        out.insert("transformed".into(), cmp::render_json(&tree));
    }
    if let Some(partials) = req.get("partials").and_then(Value::as_object) {
        let mut asts = Map::new();
        for (name, ast) in partials {
            let node = convert("partials", cmp::node_from_json(ast))?;
            asts.insert(name.clone(), cmp::node_json(&node));
        }
        out.insert("partials".into(), Value::Object(asts));
    }
    Ok(())
}

/// The answer to one request: its outputs, and `failed`/`threw` for what threw; `text` gets
/// the outputs that are JSON text, to write into the answer as they are.
/// The request line with each lone surrogate's escape (`\\udxxx` that is not half of a pair)
/// replaced by `\\ufffd`, for serde_json, which rejects them; `None` where there is none.
fn lossy_request(line: &str) -> Option<String> {
    let b = line.as_bytes();
    let hex = |at: usize| -> Option<u16> {
        let d = std::str::from_utf8(b.get(at..at + 4)?).ok()?;
        u16::from_str_radix(d, 16)
            .ok()
            .filter(|_| d.bytes().all(|c| c.is_ascii_hexdigit()))
    };
    let escape = |at: usize| -> Option<u16> {
        (b.get(at) == Some(&b'\\') && b.get(at + 1) == Some(&b'u'))
            .then(|| hex(at + 2))
            .flatten()
    };
    let mut out: Option<String> = None;
    let (mut i, mut copied, mut in_string) = (0, 0, false);
    while i < b.len() {
        match b[i] {
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            b'\\' if in_string => match escape(i) {
                Some(u @ 0xD800..=0xDBFF)
                    if escape(i + 6).is_some_and(|l| (0xDC00..=0xDFFF).contains(&l)) =>
                {
                    let _ = u;
                    i += 12;
                }
                Some(0xD800..=0xDFFF) => {
                    let o = out.get_or_insert_with(String::new);
                    o.push_str(&line[copied..i]);
                    o.push_str("\\ufffd");
                    i += 6;
                    copied = i;
                }
                Some(_) => i += 6,
                None => i += 2,
            },
            _ => i += 1,
        }
    }
    let mut o = out?;
    o.push_str(&line[copied..]);
    Some(o)
}

/// The request's `config.variables` read exactly (`Value::from_json_text`), where it has them.
fn exact_variables(line: &str) -> Option<JsValue> {
    let request = JsValue::from_json_text(line).ok()?;
    let config = markdoc::next::schema::value::get_property(&request, "config").ok()?;
    let variables = markdoc::next::schema::value::get_property(&config, "variables").ok()?;
    (!variables.is_undefined()).then_some(variables)
}

fn answer(
    req: &Map<String, Value>,
    out: &mut Map<String, Value>,
    text: &mut Vec<(String, String)>,
) {
    let mut unwritable = Map::new();
    let mut failed = Map::new();
    let mut fail = |f: Fail| {
        if !failed.contains_key(f.stage) {
            failed.insert(f.stage.to_string(), f.message.into());
        }
    };
    let stages: Vec<&str> = match (req.get("stages"), req.get("stage")) {
        (Some(Value::Array(list)), _) => list.iter().filter_map(Value::as_str).collect(),
        (_, Some(Value::String(one))) => vec![one.as_str()],
        _ => Vec::new(),
    };
    if stages == ["roundtrip"] {
        if let Err(f) = roundtrip(req, out) {
            fail(f);
        }
    } else {
        let r = Request::new(req);
        for stage in stages {
            match r.stage(stage) {
                Ok((key, Output::Json(value))) => {
                    out.insert(key.into(), value);
                }
                Ok((key, Output::Text(json))) => text.extend(json.map(|j| (key.to_owned(), j))),
                Ok((key, Output::Unwritable(message))) => {
                    let (name, message) = match message.split_once(": ") {
                        Some((name, rest)) if !name.contains(' ') => (name, rest),
                        _ => ("", message.as_str()),
                    };
                    let mut error = Map::new();
                    error.insert("name".into(), name.into());
                    error.insert("message".into(), message.into());
                    unwritable.insert(key.into(), Value::Object(error));
                }
                Err(f) => fail(f),
            }
        }
    }
    if let Some((_, first)) = failed.iter().next() {
        out.insert("threw".into(), first.clone());
        out.insert("failed".into(), Value::Object(failed));
    }
    if !unwritable.is_empty() {
        out.insert("unwritable".into(), Value::Object(unwritable));
    }
}

/// tests.yaml's cases as oracle requests.
fn conformance() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/marktest/tests.yaml");
    let text = std::fs::read_to_string(path).expect("tests/marktest/tests.yaml");
    let cases: Vec<serde_yaml::Value> = serde_yaml::from_str(&text).expect("a list of cases");
    let mut stdout = BufWriter::new(std::io::stdout().lock());
    for (i, case) in cases.iter().enumerate() {
        let field = |k: &str| case.get(k).filter(|v| !v.is_null());
        let name = field("name").and_then(|n| n.as_str()).unwrap_or_default();
        let mut req = Map::new();
        req.insert("id".into(), format!("{:03} {name}", i + 1).into());
        let code = field("code").and_then(|c| c.as_str()).unwrap_or_default();
        req.insert("code".into(), code.into());
        if let Some(config) = field("config") {
            req.insert(
                "config".into(),
                serde_json::to_value(config).expect("a JSON-shaped config"),
            );
        }
        if field("slots").and_then(|s| s.as_bool()) == Some(true) {
            req.insert("slots".into(), true.into());
        }
        writeln!(stdout, "{}", cmp::to_json_string(&Value::Object(req))).expect("stdout");
    }
}

/// The request's `id`, read even when the whole line is not (serde_json reads JSON 128
/// levels deep at most): compare.py writes it first.
fn request_id(line: &str) -> Value {
    use serde::Deserialize;
    let rest = line.trim_start().strip_prefix('{').unwrap_or_default();
    let rest = rest.trim_start().strip_prefix("\"id\"").unwrap_or_default();
    let rest = rest.trim_start().strip_prefix(':').unwrap_or_default();
    // One value, whatever follows it.
    Value::deserialize(&mut serde_json::Deserializer::from_str(rest)).unwrap_or(Value::Null)
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--conformance") {
        return conformance();
    }
    // A panic is never right: report its message as the request's, quietly.
    let last_panic: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    {
        let last_panic = Arc::clone(&last_panic);
        panic::set_hook(Box::new(move |info| {
            let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_owned()
            };
            *last_panic.lock().unwrap() = format!("panic: {message}");
        }));
    }
    let stdin = std::io::stdin();
    let mut stdout = BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line.expect("stdin");
        if line.trim().is_empty() {
            continue;
        }
        let mut out = Map::new();
        let mut text = Vec::new();
        // serde_json rejects a lone surrogate's escape: it reads the request with each one
        // `\ufffd`, and the config's `variables` come from the exact text (see
        // `semantics::exact_variables`)
        let lossy = lossy_request(&line);
        semantics::set_exact_variables(lossy.as_ref().and_then(|_| exact_variables(&line)));
        let line = lossy.unwrap_or(line);
        match serde_json::from_str::<Map<String, Value>>(&line) {
            Err(e) => {
                out.insert("id".into(), request_id(&line));
                out.insert("harness".into(), format!("unreadable request: {e}").into());
            }
            Ok(req) => {
                out.insert("id".into(), req.get("id").cloned().unwrap_or(Value::Null));
                let mut fields = Map::new();
                if panic::catch_unwind(AssertUnwindSafe(|| answer(&req, &mut fields, &mut text)))
                    .is_err()
                {
                    let message = last_panic.lock().unwrap().clone();
                    fields.insert("threw".into(), message.clone().into());
                    let mut failed = Map::new();
                    failed.insert("panic".into(), message.into());
                    fields.insert("failed".into(), Value::Object(failed));
                }
                out.extend(fields);
            }
        }
        let mut answer = cmp::to_json_string(&Value::Object(out));
        // The JSON text outputs, as members of the answer object.
        for (key, json) in text {
            answer.pop(); // '}'
            answer.push(',');
            cmp::write_json(&mut answer, &Value::String(key));
            answer.push(':');
            answer.push_str(&json);
            answer.push('}');
        }
        writeln!(stdout, "{answer}").expect("stdout");
        // One answer per request as it is made: compare.py finds the request that crashed
        // the process by the answers before it.
        stdout.flush().expect("stdout");
    }
}
