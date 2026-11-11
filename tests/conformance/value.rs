//! The JSON-shaped value both sides of a grade reduce to.
//!
//! Upstream grades a tree case with `diffJson(expected, output.children)`
//! (index.ts:62). jsdiff's `diffJson` runs both sides through `canonicalize`,
//! which re-creates every object with its keys **sorted**, then line-diffs the
//! two `JSON.stringify` results. So the rule is: structural equality, object
//! key order ignored, array order significant, numbers compared as JavaScript
//! numbers (a YAML `10` and our `10.0` are the same number). [`eq`] is
//! exactly that rule.

#[derive(Clone, Debug)]
pub enum V {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<V>),
    /// Keys in the order they were produced (YAML authored order, or our
    /// HashMap's iteration order). Equality ignores order unless asked.
    Obj(Vec<(String, V)>),
}

impl V {
    pub fn from_yaml(y: &serde_yaml::Value) -> V {
        use serde_yaml::Value as Y;
        match y {
            Y::Null => V::Null,
            Y::Bool(b) => V::Bool(*b),
            Y::Number(n) => V::Num(n.as_f64().expect("yaml number")),
            Y::String(s) => V::Str(s.clone()),
            Y::Sequence(s) => V::Arr(s.iter().map(V::from_yaml).collect()),
            Y::Mapping(m) => V::Obj(
                m.iter()
                    .map(|(k, v)| (yaml_key(k), V::from_yaml(v)))
                    .collect(),
            ),
            Y::Tagged(t) => V::from_yaml(&t.value),
        }
    }

    /// Compact JSON in the value's own key order, for display.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, out: &mut String) {
        match self {
            V::Null => out.push_str("null"),
            V::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            V::Num(n) => out.push_str(&js_number(*n)),
            V::Str(s) => out.push_str(&serde_json::to_string(s).unwrap()),
            V::Arr(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            V::Obj(o) => {
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k).unwrap());
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn yaml_key(k: &serde_yaml::Value) -> String {
    use serde_yaml::Value as Y;
    match k {
        Y::String(s) => s.clone(),
        Y::Number(n) => js_number(n.as_f64().unwrap()),
        Y::Bool(b) => b.to_string(),
        Y::Null => "null".to_string(),
        other => panic!("unsupported YAML key {other:?}"),
    }
}

/// `String(n)` for the numbers the corpus contains.
pub fn js_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

pub fn eq(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Null, V::Null) => true,
        (V::Bool(x), V::Bool(y)) => x == y,
        (V::Num(x), V::Num(y)) => x == y,
        (V::Str(x), V::Str(y)) => x == y,
        (V::Arr(x), V::Arr(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q)),
        (V::Obj(x), V::Obj(y)) => {
            if x.len() != y.len() {
                return false;
            }
            x.iter().all(|(k, v)| {
                y.iter()
                    .find(|(k2, _)| k2 == k)
                    .is_some_and(|(_, v2)| eq(v, v2))
            })
        }
        _ => false,
    }
}

/// Where two values first differ, as a path. Display only.
pub fn first_difference(path: &str, a: &V, b: &V) -> Option<String> {
    let here = if path.is_empty() { "(root)" } else { path };
    match (a, b) {
        (V::Arr(x), V::Arr(y)) => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                if let Some(d) = first_difference(&format!("{path}[{i}]"), p, q) {
                    return Some(d);
                }
            }
            if x.len() != y.len() {
                return Some(format!(
                    "{here}: expected {} items, got {}",
                    x.len(),
                    y.len()
                ));
            }
            None
        }
        (V::Obj(x), V::Obj(y)) => {
            for (k, v) in x {
                let child = format!("{path}.{k}");
                match y.iter().find(|(k2, _)| k2 == k) {
                    Some((_, v2)) => {
                        if let Some(d) = first_difference(&child, v, v2) {
                            return Some(d);
                        }
                    }
                    None => {
                        return Some(format!(
                            "{child}: missing, expected {}",
                            elide(&v.to_json())
                        ));
                    }
                }
            }
            for (k, v) in y {
                if !x.iter().any(|(k1, _)| k1 == k) {
                    return Some(format!(
                        "{path}.{k}: unexpected, got {}",
                        elide(&v.to_json())
                    ));
                }
            }
            None
        }
        _ => {
            if eq(a, b) {
                None
            } else {
                Some(format!(
                    "{here}: expected {}, got {}",
                    elide(&a.to_json()),
                    elide(&b.to_json())
                ))
            }
        }
    }
}

pub fn elide(s: &str) -> String {
    const MAX: usize = 160;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(MAX).collect();
        t.push_str("...");
        t
    }
}
