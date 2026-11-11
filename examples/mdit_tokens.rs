//! The Rust half of the markdown-it port's verification harness: reads the requests
//! tests/markdown_it/oracle_tokens.cjs reads (JSONL on stdin) and prints the same JSON
//! for the port's token stream (one line per request, in node's `JSON.stringify` form).
//!
//!   request: {id, src, preset?, options?, enable?, disable?, mode?: "parse"|"parseInline",
//!             env?: {references?: {LABEL: {href, title}}, ...}}
//!   output:  {id, tokens, env?} or {id, error} (where JS throws: an Err, the exception the
//!            engine records on `env.thrown`, or a panic)
//!
//! Run through tests/markdown_it/compare.py, or directly:
//!   cargo run --release --example mdit_tokens < requests.jsonl

use std::collections::HashMap;
use std::io::{BufRead, BufWriter, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use markdoc::markdown_it::{AttrValue, Env, MarkdownIt, Reference, Token, js};
use serde_json::{Map, Value};

/// A JSON string as `JSON.stringify` writes it.
fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A number as `JSON.stringify` writes it (`String(n)`; non-finite numbers are `null`).
fn json_num(out: &mut String, n: f64) {
    if n.is_finite() {
        out.push_str(&js::number_to_string(n))
    } else {
        out.push_str("null")
    }
}

fn json_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => out.push_str(&i.to_string()),
            (None, Some(f)) => json_num(out, f),
            _ => out.push_str(&n.to_string()),
        },
        Value::String(s) => json_str(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_value(out, x);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_str(out, k);
                out.push(':');
                json_value(out, x);
            }
            out.push('}');
        }
    }
}

/// `plain(token)` of oracle_tokens.cjs: fields in a fixed order, null/''/false-hidden omitted.
fn json_token(out: &mut String, t: &Token) {
    let mut first = true;
    let mut key = |out: &mut String, k: &str| {
        out.push_str(if first { "{" } else { "," });
        first = false;
        json_str(out, k);
        out.push(':');
    };
    if !t.kind.is_empty() {
        key(out, "type");
        json_str(out, &t.kind);
    }
    if !t.tag.is_empty() {
        key(out, "tag");
        json_str(out, &t.tag);
    }
    if let Some(attrs) = &t.attrs {
        key(out, "attrs");
        out.push('[');
        for (i, (name, value)) in attrs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('[');
            json_str(out, name);
            out.push(',');
            match value {
                AttrValue::Str(s) => json_str(out, s),
                AttrValue::Num(n) => json_num(out, *n),
            }
            out.push(']');
        }
        out.push(']');
    }
    if let Some([a, b]) = t.map {
        key(out, "map");
        out.push_str(&format!("[{a},{b}]"));
    }
    key(out, "nesting");
    out.push_str(&t.nesting.to_string());
    key(out, "level");
    out.push_str(&t.level.to_string());
    for (name, value) in [
        ("content", &t.content),
        ("markup", &t.markup),
        ("info", &t.info),
    ] {
        if !value.is_empty() {
            key(out, name);
            json_str(out, value);
        }
    }
    if let Some(meta) = t.meta.as_ref().filter(|m| !m.is_null()) {
        key(out, "meta");
        json_value(out, meta);
    }
    key(out, "block");
    out.push_str(if t.block { "true" } else { "false" });
    if t.hidden {
        key(out, "hidden");
        out.push_str("true");
    }
    if let Some(errors) = &t.errors {
        key(out, "errors");
        json_value(out, &Value::Array(errors.clone()));
    }
    if let Some(position) = t.position.as_deref().filter(|p| !p.is_null()) {
        key(out, "position");
        json_value(out, position);
    }
    if let Some(children) = &t.children {
        key(out, "children");
        json_tokens(out, children);
    }
    if first {
        out.push('{');
    }
    out.push('}');
}

fn json_tokens(out: &mut String, tokens: &[Token]) {
    out.push('[');
    for (i, t) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_token(out, t);
    }
    out.push(']');
}

/// `env` as JSON, or `None` when it has no keys (JS `{}`).
fn env_json(env: &Env) -> Option<String> {
    let mut map = Map::new();
    if let Some(refs) = &env.references {
        let mut labels: Vec<&String> = refs.keys().collect();
        labels.sort();
        let mut r = Map::new();
        for label in labels {
            let reference = &refs[label];
            let mut o = Map::new();
            o.insert("title".into(), Value::String(reference.title.clone()));
            o.insert("href".into(), Value::String(reference.href.clone()));
            r.insert(label.clone(), Value::Object(o));
        }
        map.insert("references".into(), Value::Object(r));
    }
    for (k, v) in &env.extra {
        map.insert(k.clone(), v.clone());
    }
    if map.is_empty() {
        return None;
    }
    let mut out = String::new();
    json_value(&mut out, &Value::Object(map));
    Some(out)
}

/// The request's `env` (`references` as the reference rule stores them, other keys kept in
/// `extra`), or a fresh one.
fn env_of(req: &Map<String, Value>) -> Env {
    let mut env = Env::default();
    for (key, value) in req
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        match (key.as_str(), value.as_object()) {
            ("references", Some(references)) => {
                let field = |r: &Value, name: &str| {
                    r.get(name)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                env.references = Some(
                    references
                        .iter()
                        .map(|(label, r)| {
                            let (href, title) = (field(r, "href"), field(r, "title"));
                            (label.clone(), Reference { href, title })
                        })
                        .collect(),
                );
            }
            _ => {
                env.extra.insert(key.clone(), value.clone());
            }
        }
    }
    env
}

fn names(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| {
        a.iter()
            .map(|x| x.as_str().unwrap_or_default().to_owned())
            .collect()
    })
}

fn build(req: &Map<String, Value>) -> Result<MarkdownIt, String> {
    let preset = req
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let options = req
        .get("options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut md = MarkdownIt::with_options(preset, &options).map_err(|e| e.to_string())?;
    if let Some(list) = names(req.get("enable")) {
        md.enable(&list, false).map_err(|e| e.to_string())?;
    }
    if let Some(list) = names(req.get("disable")) {
        md.disable(&list, false).map_err(|e| e.to_string())?;
    }
    Ok(md)
}

fn main() {
    // A panic is where JS throws: report its message as the request's error, quietly.
    let last_panic: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    {
        let last_panic = Arc::clone(&last_panic);
        panic::set_hook(Box::new(move |info| {
            let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_owned()
            };
            *last_panic.lock().unwrap() = msg;
        }));
    }

    let mut parsers: HashMap<String, Result<MarkdownIt, String>> = HashMap::new();
    let stdin = std::io::stdin();
    let mut stdout = BufWriter::new(std::io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line.expect("stdin");
        if line.trim().is_empty() {
            continue;
        }
        let req: Map<String, Value> = serde_json::from_str(&line).expect("a JSON object per line");
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let src = req
            .get("src")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let inline_mode = req.get("mode").and_then(Value::as_str) == Some("parseInline");

        let key = serde_json::to_string(&[
            req.get("preset"),
            req.get("options"),
            req.get("enable"),
            req.get("disable"),
        ])
        .expect("serializable");
        let md = parsers.entry(key).or_insert_with(|| build(&req));

        let mut out = String::from("{\"id\":");
        json_value(&mut out, &id);
        let result = match md {
            Err(e) => Err(e.clone()),
            Ok(md) => panic::catch_unwind(AssertUnwindSafe(|| {
                let mut env = env_of(&req);
                let tokens = if inline_mode {
                    md.parse_inline(&src, &mut env)
                } else {
                    md.parse(&src, &mut env)
                };
                (tokens, env)
            }))
            .map_err(|_| last_panic.lock().unwrap().clone())
            // what the engine records where JS throws: its message, as node's error
            .and_then(|(tokens, mut env)| match env.thrown.take() {
                Some(thrown) => Err(thrown.message),
                None => Ok((tokens, env)),
            }),
        };
        match result {
            Ok((tokens, env)) => {
                out.push_str(",\"tokens\":");
                json_tokens(&mut out, &tokens);
                if let Some(env) = env_json(&env) {
                    out.push_str(",\"env\":");
                    out.push_str(&env);
                }
            }
            Err(message) => {
                out.push_str(",\"error\":");
                json_str(&mut out, &message);
            }
        }
        out.push('}');
        writeln!(stdout, "{out}").expect("stdout");
    }
}
