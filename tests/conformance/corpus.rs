//! Reading tests.yaml. Strict: a key this loader does not know is a panic,
//! so a case is never graded against half its definition.

use serde_yaml::Value as Y;

#[derive(Debug, Clone)]
pub struct Case {
    /// 0-based position in the corpus.
    pub idx: usize,
    /// 1-based line of `- name:` in tests.yaml.
    pub line: usize,
    pub name: String,
    pub code: String,
    pub config: Option<Y>,
    /// index.ts:108 `parse(test.code || '', test.slots)`.
    pub slots: bool,
    /// index.ts:51,54 `renderer = 'react'`; `renderer === 'html'`.
    pub html: bool,
    pub expected: Option<Y>,
    /// index.ts:116 `if (test.expectedError)`.
    pub expected_error: Option<String>,
}

pub fn load(text: &str) -> Vec<Case> {
    let root: Y = serde_yaml::from_str(text).expect("tests.yaml is YAML");
    let Y::Sequence(items) = root else {
        panic!("tests.yaml is not a sequence")
    };
    // serde_yaml does not keep positions; the i-th case starts at the i-th
    // top-level `- name:` line, which holds for this file (every case leads
    // with its name).
    let lines: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("- name:"))
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(lines.len(), items.len(), "case count vs `- name:` lines");

    items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let Y::Mapping(m) = item else {
                panic!("case {idx} is not a mapping")
            };
            let mut case = Case {
                idx,
                line: lines[idx],
                name: String::new(),
                code: String::new(),
                config: None,
                slots: false,
                html: false,
                expected: None,
                expected_error: None,
            };
            for (k, v) in m {
                let key = k.as_str().expect("string key");
                match key {
                    "name" => case.name = v.as_str().expect("name").to_string(),
                    "code" => case.code = v.as_str().expect("code").to_string(),
                    "config" => case.config = Some(v.clone()),
                    "slots" => case.slots = v.as_bool().expect("slots bool"),
                    // index.ts:125 reads `validation` only to decide whether to print
                    // validation output; it never decides a case. Type-checked, not kept.
                    "validation" => {
                        v.as_bool().expect("validation bool");
                    }
                    "renderer" => {
                        case.html = match v.as_str() {
                            Some("html") => true,
                            Some("react") => false,
                            other => panic!("unknown renderer {other:?}"),
                        }
                    }
                    "expected" => case.expected = Some(v.clone()),
                    "expectedError" => {
                        case.expected_error = Some(v.as_str().expect("expectedError").to_string())
                    }
                    other => panic!("case {idx} ({}): unknown key {other:?}", case.name),
                }
            }
            assert!(!case.name.is_empty(), "case {idx} has no name");
            case
        })
        .collect()
}
