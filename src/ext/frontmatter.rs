//! E9: the frontmatter helper, for tools that read a document's frontmatter without parsing
//! the document.
//!
//! [`split`] finds the frontmatter by upstream's rule (Markdoc's frontmatter plugin): line 0,
//! trimmed as JavaScript trims (so a byte-order mark before it is tolerated), is `---`, and the
//! first later line that trims to `---` closes it; the text between the two, trimmed, is the
//! frontmatter. Line breaks are `\n`, `\r\n` or `\r`, as markdown-it reads them. Without a
//! closing line there is no frontmatter.
//!
//! [`parse_yaml`] converts the YAML as the parser's own option ([`super::Frontmatter::Yaml`])
//! does: a mapping becomes an object in a JavaScript object's key order (array-index keys
//! first), numbers are `f64`, keys that are not strings are dropped, tagged values are null.
//! Unlike that option, which keeps the raw text and reports `frontmatter-invalid`, it returns
//! `Err` on YAML that is not a mapping, for callers that skip such files.
//! [`extract_frontmatter`] does both and returns the body.
//!
//! ```
//! use markdoc::ext::frontmatter::{extract_frontmatter, split};
//! use markdoc::types::Scalar;
//!
//! let source = "\u{FEFF}---\r\ntitle: Hello\r\ntags: [a, b]\r\n---\r\n# Body\r\n";
//! let found = split(source).unwrap();
//! assert_eq!(found.yaml, "title: Hello\r\ntags: [a, b]");
//! assert_eq!(&source[found.range.end..], "# Body\r\n");
//!
//! let (fm, body) = extract_frontmatter(source).unwrap();
//! let fm = fm.unwrap();
//! assert_eq!(fm["title"], Scalar::String("Hello".into()));
//! assert_eq!(fm.keys().collect::<Vec<_>>(), ["title", "tags"]);
//! assert_eq!(body, "# Body\r\n");
//!
//! // No closing line: no frontmatter, the text is all body.
//! assert!(split("---\ntitle: x\n").is_none());
//! assert!(extract_frontmatter("---\n: :\n---\n").is_err());
//! ```

use crate::markdown_it::js::is_js_space;
use crate::types::{MarkdocError, Result, Scalar, js_insert};
use indexmap::IndexMap;
use std::ops::Range;

/// A document's frontmatter, as [`split`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split<'a> {
    /// The text between the two `---` lines, trimmed (upstream's `frontmatter` token's
    /// content, apart from line breaks, which are kept as written).
    pub yaml: &'a str,
    /// The frontmatter in the source, in bytes: from the start of the text (a byte-order mark
    /// included) to the end of the closing line, its line break included. The body starts at
    /// `range.end`.
    pub range: Range<usize>,
}

/// `s.trim()` as JavaScript trims.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| is_js_space(u32::from(c)))
}

/// The lines of `source`, as byte ranges of their text (without the break) and where the next
/// line starts.
fn lines(source: &str) -> impl Iterator<Item = (Range<usize>, usize)> + '_ {
    let bytes = source.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        if start > bytes.len() || (start == bytes.len() && start != 0) {
            return None;
        }
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .map_or(bytes.len(), |i| start + i);
        let next = match bytes.get(end) {
            Some(b'\r') if bytes.get(end + 1) == Some(&b'\n') => end + 2,
            Some(_) => end + 1,
            None => bytes.len() + 1,
        };
        let line = (start..end, next.min(bytes.len()));
        start = next;
        Some(line)
    })
}

/// The frontmatter of `source` by upstream's rule (see the module's documentation), or `None`.
pub fn split(source: &str) -> Option<Split<'_>> {
    let mut lines = lines(source);
    let (first, _) = lines.next()?;
    if js_trim(&source[first.clone()]) != "---" {
        return None;
    }
    let (close, after) = lines.find(|(line, _)| js_trim(&source[line.clone()]) == "---")?;
    Some(Split {
        // From line 0's break to the closing line, trimmed (the breaks are JS whitespace).
        yaml: js_trim(&source[first.end..close.start]),
        range: 0..after,
    })
}

/// YAML to an object (see the module's documentation): `Err` when it does not parse or is not
/// a mapping.
pub fn parse_yaml(yaml: &str) -> Result<IndexMap<String, Scalar>> {
    let mapping: IndexMap<String, serde_yaml::Value> = serde_yaml::from_str(yaml)
        .map_err(|e| MarkdocError::ParseError(format!("Invalid YAML frontmatter: {e}")))?;
    let mut object = IndexMap::with_capacity(mapping.len());
    for (key, value) in mapping {
        js_insert(&mut object, key, yaml_to_scalar(value));
    }
    Ok(object)
}

/// The frontmatter of `content` parsed ([`split`], then [`parse_yaml`]), and the body after
/// it: `(None, content)` when there is none, `Err` when its YAML is not a mapping.
pub fn extract_frontmatter(content: &str) -> Result<(Option<IndexMap<String, Scalar>>, String)> {
    match split(content) {
        None => Ok((None, content.to_string())),
        Some(found) => Ok((
            Some(parse_yaml(found.yaml)?),
            content[found.range.end..].to_string(),
        )),
    }
}

/// One YAML value, as the parser's frontmatter option converts it.
fn yaml_to_scalar(value: serde_yaml::Value) -> Scalar {
    match value {
        serde_yaml::Value::Null | serde_yaml::Value::Tagged(_) => Scalar::Null,
        serde_yaml::Value::Bool(b) => Scalar::Boolean(b),
        serde_yaml::Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => Scalar::Number(i as f64),
            (None, Some(f)) => Scalar::Number(f),
            (None, None) => Scalar::Null,
        },
        serde_yaml::Value::String(s) => Scalar::String(s),
        serde_yaml::Value::Sequence(items) => {
            Scalar::Array(items.into_iter().map(yaml_to_scalar).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut object = IndexMap::with_capacity(map.len());
            for (key, value) in map {
                if let serde_yaml::Value::String(key) = key {
                    js_insert(&mut object, key, yaml_to_scalar(value));
                }
            }
            Scalar::Object(object)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::{Frontmatter, ParseOptions, parse};

    /// The frontmatter object the parser's YAML option gives the document.
    fn parsed_option(source: &str) -> Scalar {
        let options = ParseOptions {
            frontmatter: Frontmatter::Yaml,
            ..Default::default()
        };
        parse(source, &options).unwrap().attributes["frontmatter"].clone()
    }

    #[test]
    fn the_delimiters_are_upstreams() {
        // Trimmed lines, a BOM before the first, CRLF and lone CR breaks.
        for source in [
            "---\na: 1\n---\nbody",
            "  ---  \na: 1\n --- \nbody",
            "\u{FEFF}---\na: 1\n---\nbody",
            "---\r\na: 1\r\n---\r\nbody",
            "---\ra: 1\r---\rbody",
        ] {
            let found = split(source).unwrap_or_else(|| panic!("{source:?}"));
            assert_eq!(found.yaml, "a: 1", "{source:?}");
            assert_eq!(&source[found.range.end..], "body", "{source:?}");
        }
        // Not on line 0, not closed, or not exactly three dashes: no frontmatter.
        for source in [
            "\n---\na: 1\n---\n",
            "---\na: 1\n",
            "----\na\n----\n",
            "--- x\n---\n",
        ] {
            assert_eq!(split(source), None, "{source:?}");
        }
        // The first closing line wins; the frontmatter may be empty; the text may end there.
        assert_eq!(split("---\na\n---\nb\n---\n").unwrap().yaml, "a");
        assert_eq!(split("---\n---\nx").unwrap().yaml, "");
        let found = split("---\na: 1\n---").unwrap();
        assert_eq!((found.yaml, found.range), ("a: 1", 0..12));
    }

    #[test]
    fn it_agrees_with_the_tokenizer_on_what_is_frontmatter() {
        // The document's raw frontmatter (upstream's token content) is what `split` finds,
        // for the shapes above and for text that is no frontmatter.
        for source in [
            "---\na: 1\n---\nbody",
            "\u{FEFF}---\na: 1\n---\nbody",
            "  ---  \n  a: 1  \n --- \nbody",
            "---\n---\nx",
            "---\na: 1\n",
            "\n---\na: 1\n---\n",
        ] {
            let doc = parse(source, &ParseOptions::default()).unwrap();
            let raw = doc.attributes.get("frontmatter").and_then(|f| match f {
                Scalar::String(s) => Some(s.clone()),
                _ => None,
            });
            assert_eq!(raw.as_deref(), split(source).map(|s| s.yaml), "{source:?}");
        }
    }

    #[test]
    fn yaml_converts_as_the_parse_option_does() {
        let source = "---\nb: 1\n2: two\n1: one\nlist: [1, x, {k: v}]\nnested: {z: 1, a: 2}\nn: ~\nf: 1.5\n---\n";
        let parsed = parsed_option(source);
        let Scalar::Object(expected) = &parsed else {
            panic!("the option gives an object")
        };
        let got = parse_yaml(split(source).unwrap().yaml).unwrap();
        assert_eq!(&got, expected);
        // Array-index keys first, as a JavaScript object orders them.
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            ["1", "2", "b", "list", "nested", "n", "f"]
        );
    }

    #[test]
    fn bad_yaml_is_an_error_here_and_an_error_node_there() {
        let source = "---\n- not\n- a mapping\n---\nbody\n";
        assert!(extract_frontmatter(source).is_err());
        assert!(matches!(parsed_option(source), Scalar::String(_)));
        let options = ParseOptions {
            frontmatter: Frontmatter::Yaml,
            ..Default::default()
        };
        let doc = parse(source, &options).unwrap();
        assert_eq!(doc.errors[0].id, "frontmatter-invalid");
    }

    #[test]
    fn extract_frontmatter_keeps_the_old_contract() {
        // The old helper's cases (crate::frontmatter), with the new map type.
        let (fm, body) = extract_frontmatter("---\ntitle: T\nswVersion: 1.10\n---\n# H\n").unwrap();
        let fm = fm.unwrap();
        assert_eq!(fm["title"], Scalar::String("T".into()));
        assert_eq!(fm["swVersion"], Scalar::Number(1.1));
        assert_eq!(body, "# H\n");
        assert_eq!(
            extract_frontmatter("# No frontmatter\n").unwrap(),
            (None, "# No frontmatter\n".to_string())
        );
        // What the old helper (removed at the switch-over) returned for the same source.
        let new = extract_frontmatter("---\ntitle: T\n---\nbody").unwrap();
        assert_eq!(new.1, "body");
        assert_eq!(new.0.unwrap()["title"], Scalar::String("T".into()));
    }
}
