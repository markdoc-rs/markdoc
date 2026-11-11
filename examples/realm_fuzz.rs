//! The Rust half of the realm's conversion fuzzer (`findings/conformance/work/realm/convfuzz.cjs`
//! writes the cases with node's answers): each case's value converted as upstream converts
//! values, `String(value)` ([`Value::to_js_string`]) or `JSON.stringify` of it, under a key or
//! at the top ([`json_stringify`]), compared with node's answer.
//!
//! Usage: `realm_fuzz CASES.jsonl [--show N]`. Prints each difference (up to N, default 20)
//! and the totals; exits 1 when a case differs that did not mutate its value in node (a call
//! that sets properties of `this`, which the port's values do not record: counted apart).

use markdoc::next::realm::Builtin;
use markdoc::next::schema::value::{JsString, Value, json_stringify};
use markdoc::types::JsObject;
use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::io::BufRead;

fn decode(d: &Json) -> Value {
    match d {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        Json::String(s) => Value::string(s.as_str()),
        Json::Array(_) => panic!("a bare array in a desc"),
        Json::Object(o) => {
            if o.contains_key("u") {
                Value::Undefined
            } else if let Some(Json::String(n)) = o.get("n") {
                Value::Number(match n.as_str() {
                    "-0" => -0.0,
                    "NaN" => f64::NAN,
                    "Infinity" => f64::INFINITY,
                    "-Infinity" => f64::NEG_INFINITY,
                    other => panic!("number {other}"),
                })
            } else if let Some(Json::Array(units)) = o.get("s16") {
                let units: Vec<u16> = units.iter().map(|u| u.as_u64().unwrap() as u16).collect();
                Value::String(JsString::from_utf16(&units))
            } else if let Some(Json::String(name)) = o.get("b") {
                Value::Builtin(
                    Builtin::all()
                        .find(|b| b.name() == name)
                        .unwrap_or_else(|| panic!("no built-in {name}")),
                )
            } else if let Some(Json::Array(items)) = o.get("a") {
                Value::array(items.iter().map(decode).collect())
            } else if let Some(Json::Array(entries)) = o.get("o") {
                let mut object = JsObject::new();
                for e in entries {
                    let k = e[0].as_str().unwrap().to_string();
                    object.insert(k, decode(&e[1]));
                }
                Value::object(object)
            } else {
                panic!("desc {d}")
            }
        }
    }
}

/// Node's answer as text to compare: a string's code units, or `!Name: message`, or
/// `undefined`.
fn expected(out: &Json) -> String {
    match out {
        Json::String(s) => format!("={s}"),
        Json::Object(o) if o.contains_key("t") => format!("!{}", o["t"].as_str().unwrap()),
        Json::Object(o) if o.contains_key("u") => "undefined".into(),
        Json::Object(o) if o.contains_key("s16") => {
            let units: Vec<u16> = o["s16"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_u64().unwrap() as u16)
                .collect();
            format!("={}", JsString::from_utf16(&units).to_string_lossy())
        }
        other => panic!("out {other}"),
    }
}

fn actual(case: &Json) -> String {
    let value = decode(&case["value"]);
    match case["kind"].as_str().unwrap() {
        "string" => match value.to_js_string() {
            Ok(s) => format!("={}", s.to_string_lossy()),
            Err(e) => format!("!{e}"),
        },
        _ => {
            let holder = match case.get("key").and_then(Json::as_str) {
                Some(key) => {
                    let mut o = JsObject::new();
                    o.insert(key.to_string(), value);
                    Value::object(o)
                }
                None => value,
            };
            let gap = case.get("gap").and_then(Json::as_str).unwrap_or("");
            match json_stringify(&holder, gap) {
                Ok(Some(s)) => format!("={s}"),
                Ok(None) => "undefined".into(),
                Err(e) => format!("!{e}"),
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: realm_fuzz CASES.jsonl [--show N]");
    let show: usize = args
        .iter()
        .position(|a| a == "--show")
        .and_then(|i| args.get(i + 1))
        .map(|n| n.parse().unwrap())
        .unwrap_or(20);
    let trace = args.iter().any(|a| a == "--trace");
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("the cases"));
    let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
    let mut shown = 0;
    for line in file.lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let case: Json = serde_json::from_str(&line).expect("a case");
        if trace {
            eprintln!("{}", case["id"]);
        }
        let want = expected(&case["out"]);
        let got = match std::panic::catch_unwind(|| actual(&case)) {
            Ok(got) => got,
            Err(_) => "panic".into(),
        };
        let mutated = case.get("mutated").is_some();
        let class = if got == want {
            "equal"
        } else if got == "panic" {
            "panic"
        } else if mutated {
            "differ (mutated in node)"
        } else {
            "differ"
        };
        *totals.entry(class).or_default() += 1;
        if class != "equal" && class != "differ (mutated in node)" && shown < show {
            shown += 1;
            println!(
                "{} {}\n  value {}{}\n  node: {}\n  port: {}",
                case["id"].as_str().unwrap(),
                case["kind"].as_str().unwrap(),
                case["value"],
                case.get("key")
                    .map(|k| format!(" key {k}"))
                    .unwrap_or_default(),
                want,
                got
            );
        }
    }
    println!("{totals:?}");
    let bad =
        totals.get("differ").copied().unwrap_or(0) + totals.get("panic").copied().unwrap_or(0);
    std::process::exit(if bad > 0 { 1 } else { 0 });
}
