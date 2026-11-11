//! The markdown-it port against the JavaScript's own token streams, without node.
//!
//! `tests/markdown_it/golden/<set>.txt` holds, for every document of
//! `tests/markdown_it/<set>.jsonl`, the tokens that the markdown-it 15.0.1 bundled in upstream
//! Markdoc 0.5.11 produces, in each configuration its `# config` lines define and in both
//! `parse` and `parseInline` mode. `tests/markdown_it/golden.py` generates the files from the
//! node oracle (`tests/markdown_it/oracle_tokens.cjs`), and their header describes the
//! format. This test runs the same requests through the port, renders its tokens the same
//! way, and requires every section to be identical.
//!
//! Sets: `spec` (the CommonMark 0.31.2 examples, plus the 0.30 ones whose input differs) and
//! `regressions` (cases the fuzzer minimised, and hand-written probes). An input line may
//! carry `options`, merged over every configuration's options (the line's keys win).
//!
//! ```text
//! cargo test --test markdown_it_golden
//! python3 -I tests/markdown_it/golden.py --markdoc DIST/index.js    # regenerate (node)
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use markdoc::markdown_it::{AttrValue, Env, MarkdownIt, Token, js};
use serde_json::{Map, Value};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/markdown_it")
}

/// A JSON string as Python's `json.dumps(ensure_ascii=False)` writes it: only `"`, `\` and
/// the control characters below U+0020 are escaped.
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
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Compact JSON with sorted object keys (`json.dumps(sort_keys=True)`).
fn json(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // (meta and env hold strings only; integers print alike in Python and serde_json)
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => json_str(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json(out, x);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let sorted: BTreeMap<&String, &Value> = o.iter().collect();
            out.push('{');
            for (i, (k, x)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_str(out, k);
                out.push(':');
                json(out, x);
            }
            out.push('}');
        }
    }
}

/// `type` and `tag` are written bare when they are `[A-Za-z0-9_]+`, else as JSON.
fn bare(out: &mut String, s: &str) {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        out.push_str(s);
    } else {
        json_str(out, s);
    }
}

/// One token's line, then its children's, as golden.py's `token_lines`.
fn token_lines(t: &Token, depth: usize, lines: &mut Vec<String>) {
    let mut l = "  ".repeat(depth);
    bare(&mut l, &t.kind);
    if !t.tag.is_empty() {
        l.push_str(" tag=");
        bare(&mut l, &t.tag);
    }
    if let Some(attrs) = &t.attrs {
        l.push_str(" attrs=[");
        for (i, (name, value)) in attrs.iter().enumerate() {
            if i > 0 {
                l.push(',');
            }
            l.push('[');
            json_str(&mut l, name);
            l.push(',');
            match value {
                AttrValue::Str(s) => json_str(&mut l, s),
                AttrValue::Num(n) => l.push_str(&js::number_to_string(*n)),
            }
            l.push(']');
        }
        l.push(']');
    }
    if let Some([a, b]) = t.map {
        let _ = write!(l, " map={a},{b}");
    }
    if t.nesting != 0 {
        let _ = write!(l, " nesting={}", t.nesting);
    }
    if t.level != 0 {
        let _ = write!(l, " level={}", t.level);
    }
    for (name, value) in [
        ("content", &t.content),
        ("markup", &t.markup),
        ("info", &t.info),
    ] {
        if !value.is_empty() {
            let _ = write!(l, " {name}=");
            json_str(&mut l, value);
        }
    }
    if let Some(meta) = t.meta.as_ref().filter(|m| !m.is_null()) {
        l.push_str(" meta=");
        json(&mut l, meta);
    }
    if t.block {
        l.push_str(" block");
    }
    if t.hidden {
        l.push_str(" hidden");
    }
    if let Some(errors) = &t.errors {
        l.push_str(" errors=");
        json(&mut l, &Value::Array(errors.clone()));
    }
    if let Some(position) = t.position.as_ref().filter(|p| !p.is_null()) {
        l.push_str(" position=");
        json(&mut l, position);
    }
    if let Some(children) = &t.children {
        let _ = write!(l, " children={}", children.len());
    }
    lines.push(l);
    for c in t.children.iter().flatten() {
        token_lines(c, depth + 1, lines);
    }
}

/// `env` as JSON when it has keys (JS prints `{}` as nothing).
fn env_line(env: &Env) -> Option<String> {
    let mut map = Map::new();
    if let Some(refs) = &env.references {
        let mut r = Map::new();
        for (label, reference) in refs {
            let mut o = Map::new();
            o.insert("href".into(), Value::String(reference.href.clone()));
            o.insert("title".into(), Value::String(reference.title.clone()));
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
    let mut out = String::from("env ");
    json(&mut out, &Value::Object(map));
    Some(out)
}

/// A configuration line's request fields, with an input line's `options` merged over its
/// options, as oracle_tokens.cjs builds the parser from them.
fn build(fields: &Map<String, Value>, line_options: Option<&Map<String, Value>>) -> MarkdownIt {
    let preset = fields
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let mut options = fields
        .get("options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (k, v) in line_options.into_iter().flatten() {
        options.insert(k.clone(), v.clone());
    }
    let names = |key: &str| -> Vec<String> {
        fields
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut md = MarkdownIt::with_options(preset, &options).expect("config preset");
    if fields.contains_key("enable") {
        md.enable(&names("enable"), false).expect("config enable");
    }
    if fields.contains_key("disable") {
        md.disable(&names("disable"), false)
            .expect("config disable");
    }
    md
}

/// The port's output for one document, in golden lines; where JS throws, `Err` with the
/// golden line of the exception the port records (`error "message"`), or a panic's message.
fn render(md: &MarkdownIt, src: &str, inline: bool) -> Result<Vec<String>, String> {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut env = Env::default();
        let tokens = if inline {
            md.parse_inline(src, &mut env)
        } else {
            md.parse(src, &mut env)
        };
        (tokens, env)
    }));
    match result {
        Ok((_, env)) if env.thrown.is_some() => {
            let thrown = env.thrown.expect("checked");
            let mut line = "error ".to_owned();
            json_str(&mut line, &thrown.message);
            Err(line)
        }
        Ok((tokens, env)) => {
            let mut lines = Vec::new();
            for t in &tokens {
                token_lines(t, 0, &mut lines);
            }
            lines.extend(env_line(&env));
            Ok(lines)
        }
        Err(payload) => Err(payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic".to_owned())),
    }
}

struct Section {
    id: String,
    mode: String,
    configs: Vec<String>,
    lines: Vec<String>,
}

struct Golden {
    configs: Vec<(String, Map<String, Value>)>,
    sections: Vec<Section>,
}

fn read_golden(set: &str) -> Golden {
    let path = dir().join("golden").join(format!("{set}.txt"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut golden = Golden {
        configs: Vec::new(),
        sections: Vec::new(),
    };
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("=== ") {
            let mut words = header.split(' ').map(String::from);
            let (id, mode) = (words.next().unwrap(), words.next().unwrap());
            golden.sections.push(Section {
                id,
                mode,
                configs: words.collect(),
                lines: Vec::new(),
            });
        } else if let Some(section) = golden.sections.last_mut() {
            section.lines.push(line.to_owned());
        } else if let Some(config) = line.strip_prefix("# config ") {
            let (name, fields) = config.split_once(' ').expect("# config NAME JSON");
            let fields: Map<String, Value> = serde_json::from_str(fields).expect("config JSON");
            golden.configs.push((name.to_owned(), fields));
        }
    }
    golden
}

struct Input {
    id: String,
    src: String,
    options: Option<Map<String, Value>>,
}

fn read_inputs(set: &str) -> Vec<Input> {
    let path = dir().join(format!("{set}.jsonl"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("a JSON object per line");
            let field = |k: &str| v[k].as_str().expect("id and src strings").to_owned();
            Input {
                id: field("id"),
                src: field("src"),
                options: v.get("options").and_then(Value::as_object).cloned(),
            }
        })
        .collect()
}

fn check(set: &str) {
    let golden = read_golden(set);
    let inputs = read_inputs(set);
    let configs: HashMap<&str, &Map<String, Value>> = golden
        .configs
        .iter()
        .map(|(name, fields)| (name.as_str(), fields))
        .collect();
    let parsers: HashMap<&str, MarkdownIt> = golden
        .configs
        .iter()
        .map(|(name, fields)| (name.as_str(), build(fields, None)))
        .collect();
    let sources: HashMap<&str, &Input> = inputs.iter().map(|i| (i.id.as_str(), i)).collect();
    let regenerate =
        "regenerate with: python3 -I tests/markdown_it/golden.py --markdoc DIST/index.js";

    // Every (document, mode, configuration) is in exactly one section.
    let mut covered = HashSet::new();
    for s in &golden.sections {
        assert!(
            sources.contains_key(s.id.as_str()),
            "{set}: golden has {} but the inputs do not; {regenerate}",
            s.id
        );
        for c in &s.configs {
            assert!(
                configs.contains_key(c.as_str()),
                "{set}: section {} names unknown config {c}",
                s.id
            );
            assert!(
                covered.insert((s.id.clone(), s.mode.clone(), c.clone())),
                "{set}: {} {} {c} twice",
                s.id,
                s.mode
            );
        }
    }
    let expected = inputs.len() * 2 * golden.configs.len();
    assert_eq!(
        covered.len(),
        expected,
        "{set}: golden file is stale; {regenerate}"
    );

    let (mut same, mut failures) = (0, Vec::new());
    for s in &golden.sections {
        let input = sources[s.id.as_str()];
        let src = input.src.as_str();
        let inline = match s.mode.as_str() {
            "parse" => false,
            "parseInline" => true,
            other => panic!("{set}: unknown mode {other}"),
        };
        for c in &s.configs {
            let got = match &input.options {
                None => render(&parsers[c.as_str()], src, inline),
                Some(options) => render(&build(configs[c.as_str()], Some(options)), src, inline),
            };
            let ok = match &got {
                Ok(lines) => *lines == s.lines,
                Err(line) if line.starts_with("error ") => {
                    s.lines.len() == 1 && s.lines[0] == *line
                }
                Err(_) => s.lines.len() == 1 && s.lines[0].starts_with("error "),
            };
            if ok {
                same += 1;
                continue;
            }
            let mut report = format!("--- {} {} {c}  src={:?}\n", s.id, s.mode, src);
            match &got {
                Err(message) => {
                    let _ = writeln!(report, "    rust panicked: {message}");
                }
                Ok(lines) => {
                    let at = lines
                        .iter()
                        .zip(&s.lines)
                        .position(|(a, b)| a != b)
                        .unwrap_or(lines.len().min(s.lines.len()));
                    let none = "<none>".to_owned();
                    let _ = writeln!(report, "    first difference at line {}", at + 1);
                    let _ = writeln!(report, "    node: {}", s.lines.get(at).unwrap_or(&none));
                    let _ = writeln!(report, "    rust: {}", lines.get(at).unwrap_or(&none));
                }
            }
            failures.push(report);
        }
    }
    if !failures.is_empty() {
        let shown: String = failures.iter().take(15).map(String::as_str).collect();
        panic!(
            "{set}: {} of {} token streams differ from node's\n{shown}",
            failures.len(),
            same + failures.len()
        );
    }
    assert_eq!(same, expected);
}

#[test]
fn spec_matches_node() {
    check("spec");
}

#[test]
fn regressions_match_node() {
    check("regressions");
}
