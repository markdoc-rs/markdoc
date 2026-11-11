//! Tests of the parser port: upstream's `parser.test.ts` and `transforms/table.test.ts`
//! (their AST assertions; `validate()`'s are the semantics port's), the 105 conformance
//! cases' ASTs against upstream's (node-free, from `tests/next/golden/conformance.jsonl`),
//! and this crate's positions, frontmatter and JS-object behaviour.
//!
//! The token streams are upstream's own tokenizer's (`fixtures.json`, written by
//! `fixtures.cjs` beside this file), as the tokenizer port lives on another branch.

use super::{ParserArgs, parse};
use crate::ast::{Node, PathSegment, Variable};
use crate::markdown_it::Token;
use crate::next::compare::{first_difference, node_json, tokens_from_json};
use crate::types::{Location, LocationEdge, NodeType, Scalar, ValidationLevel};
use indexmap::IndexMap;
use serde_json::Value;
use std::sync::OnceLock;

fn fixtures() -> &'static serde_json::Map<String, Value> {
    static FIXTURES: OnceLock<serde_json::Map<String, Value>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| serde_json::from_str(include_str!("fixtures.json")).expect("fixtures.json"))
}

fn source(name: &str) -> &'static str {
    fixtures()[name]["src"].as_str().expect("a fixture's src")
}

fn parse_with(name: &str, args: &ParserArgs) -> Node {
    let case = &fixtures()
        .get(name)
        .unwrap_or_else(|| panic!("no fixture {name:?}"));
    let tokens = tokens_from_json(&case["tokens"]).expect("fixture tokens");
    parse(&tokens, source(name), args).expect("parses")
}

/// `convert(example)` / `Markdoc.parse(tokens)`: default arguments.
fn convert(name: &str) -> Node {
    parse_with(name, &ParserArgs::default())
}

/// The default arguments and this crate's spans ([`ParserArgs::spans`]).
fn spanned() -> ParserArgs {
    ParserArgs {
        spans: true,
        ..ParserArgs::default()
    }
}

/// `convert(name)` with this crate's spans on.
fn located(name: &str) -> Node {
    parse_with(name, &spanned())
}

/// The default arguments and the crate's switch that parses the frontmatter's YAML.
fn yaml_frontmatter() -> ParserArgs {
    ParserArgs {
        parse_frontmatter: true,
        ..ParserArgs::default()
    }
}

fn content(node: &Node) -> Option<&Scalar> {
    node.attributes.get("content")
}

fn string(s: &str) -> Scalar {
    Scalar::String(s.to_string())
}

fn attrs(pairs: &[(&str, Scalar)]) -> IndexMap<String, Scalar> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn walk(node: &Node) -> Vec<&Node> {
    fn go<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
        for c in n.slots.values().chain(n.children.iter()) {
            out.push(c);
            go(c, out);
        }
    }
    let mut out = Vec::new();
    go(node, &mut out);
    out
}

/// The text a node's span covers.
fn text_at<'a>(src: &'a str, node: &Node) -> &'a str {
    let l = node.span.as_ref().expect("a span");
    &src[l.start.offset..l.end.offset]
}

fn tag<'a>(doc: &'a Node, name: &str) -> &'a Node {
    walk(doc)
        .into_iter()
        .find(|n| n.node_type == NodeType::Tag && n.tag.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no tag {name}"))
}

// --- parser.test.ts: handling options ----------------------------------------------------

#[test]
fn options_no_args() {
    let example = convert("heading");
    let heading = &example.children[0];
    assert_eq!(heading.node_type, NodeType::Heading);
    assert_eq!(heading.location.as_ref().unwrap().file, None);
}

#[test]
fn options_filename_as_string_and_as_property() {
    for args in [
        ParserArgs::file("foo.md"),
        ParserArgs {
            file: Some("foo.md".into()),
            ..ParserArgs::default()
        },
    ] {
        let example = parse_with("heading", &args);
        let heading = &example.children[0];
        assert_eq!(heading.node_type, NodeType::Heading);
        assert_eq!(
            heading.location.as_ref().unwrap().file.as_deref(),
            Some("foo.md")
        );
    }
}

#[test]
fn options_location_off() {
    let example = parse_with(
        "heading",
        &ParserArgs {
            file: Some("foo.md".into()),
            location: false,
            // Spans need upstream's locations: asked for without them, there are none.
            spans: true,
            ..ParserArgs::default()
        },
    );
    assert!(example.children[0].location.is_none());
    for node in std::iter::once(&example).chain(walk(&example)) {
        assert!(node.location.is_none() && node.lines.is_empty() && node.span.is_none());
    }
}

#[test]
fn options_location_on() {
    // Upstream: `{file: 'foo.md', start: {line: 0}, end: {line: 1}}`, its 0-based map, and
    // `lines` `[0, 1]`. This crate's span has 1-based lines and covers the node.
    for args in [
        ParserArgs {
            file: Some("foo.md".into()),
            location: true,
            spans: true,
            ..ParserArgs::default()
        },
        ParserArgs {
            spans: true,
            ..ParserArgs::file("foo.md")
        },
    ] {
        let example = parse_with("heading", &args);
        let edge = |line| LocationEdge {
            line: Some(line),
            character: None,
        };
        assert_eq!(
            example.children[0].location,
            Some(Location {
                file: Some("foo.md".into()),
                start: edge(0),
                end: edge(1),
            })
        );
        assert_eq!(example.children[0].lines, [0, 1]);
        let span = example.children[0].span.as_ref().unwrap();
        assert_eq!(span.file.as_deref(), Some("foo.md"));
        assert_eq!((span.start.line, span.end.line), (1, 1));
        // The document has neither.
        assert!(example.location.is_none() && example.lines.is_empty());
    }
}

// --- handling frontmatter ------------------------------------------------------------------

#[test]
fn simple_frontmatter() {
    // Upstream: `frontmatter: 'foo: bar'`; with the crate's switch, the YAML parsed.
    let example = convert("simple frontmatter");
    assert_eq!(example.node_type, NodeType::Document);
    assert_eq!(
        example.attributes.get("frontmatter"),
        Some(&string("foo: bar"))
    );
    assert!(example.children.is_empty());
    let parsed = parse_with("simple frontmatter", &yaml_frontmatter());
    assert_eq!(
        parsed.attributes.get("frontmatter"),
        Some(&Scalar::Object(attrs(&[("foo", string("bar"))])))
    );
}

#[test]
fn frontmatter_followed_by_content() {
    let example = parse_with("frontmatter followed by content", &yaml_frontmatter());
    assert_eq!(
        example.attributes.get("frontmatter"),
        Some(&Scalar::Object(attrs(&[
            ("foo", string("bar")),
            ("test", Scalar::Boolean(true))
        ])))
    );
    assert_eq!(example.children.len(), 1);
    assert_eq!(example.children[0].node_type, NodeType::Heading);
    assert_eq!(
        example.children[0].attributes,
        attrs(&[("level", Scalar::Number(1.0))])
    );
}

// --- handling attributes -------------------------------------------------------------------

#[test]
fn attributes_for_emphasis() {
    let marker = |name: &str, n: usize| {
        let doc = convert(name);
        doc.children[0].children[0].children[n]
            .attributes
            .get("marker")
            .cloned()
    };
    assert_eq!(marker("emphasis a*b*c", 1), Some(string("*")));
    assert_eq!(marker("emphasis a**b**c", 1), Some(string("**")));
    assert_eq!(marker("emphasis _foo_ bar", 0), Some(string("_")));
    assert_eq!(marker("emphasis __foo__ bar", 0), Some(string("__")));
    assert_eq!(marker("emphasis foo *bar* baz", 1), Some(string("*")));
    assert_eq!(marker("emphasis foo **bar** baz", 1), Some(string("**")));
}

#[test]
fn attributes_for_heading() {
    let document = convert("sample heading");
    assert_eq!(
        document.children[0].attributes,
        attrs(&[("level", Scalar::Number(1.0))])
    );
}

#[test]
fn attributes_for_list() {
    let unordered = convert("unordered list");
    let ordered = convert("ordered list");
    assert_eq!(
        unordered.children[0].attributes.get("ordered"),
        Some(&Scalar::Boolean(false))
    );
    assert_eq!(
        ordered.children[0].attributes.get("ordered"),
        Some(&Scalar::Boolean(true))
    );
}

#[test]
fn attributes_for_ordered_list_start() {
    let start = |name: &str| convert(name).children[0].attributes.get("start").cloned();
    assert_eq!(start("unordered list"), None);
    assert_eq!(start("numbered list from 3"), Some(Scalar::Number(3.0)));
    assert_eq!(start("numbered list from 1"), None);
}

#[test]
fn attributes_for_links() {
    let link = |name: &str| convert(name).children[0].children[0].children[0].clone();
    assert_eq!(
        link("link with one word").attributes,
        attrs(&[("href", string("/bar"))])
    );
    assert_eq!(
        link("link with one word and a title").attributes,
        attrs(&[("href", string("/bar")), ("title", string("title"))])
    );
    assert_eq!(link("link").attributes, attrs(&[("href", string("/bar"))]));
}

#[test]
fn attributes_for_text() {
    let document = convert("text");
    let p = &document.children[0].children[0].children[0];
    assert_eq!(
        p.attributes,
        attrs(&[("content", string("This is a test"))])
    );
}

#[test]
fn attributes_for_code_fence() {
    let simple = convert("fence simple");
    let complex = convert("fence complex");
    let empty = convert("fence empty");
    let ruby = attrs(&[
        ("content", string("This is a test\n")),
        ("language", string("ruby")),
    ]);
    assert_eq!(simple.children[0].attributes, ruby);
    assert_eq!(complex.children[0].attributes, ruby);
    assert_eq!(
        empty.children[0].attributes,
        attrs(&[("content", string("This is a test\n"))])
    );
}

#[test]
fn attributes_for_images() {
    let image = |name: &str| convert(name).children[0].children[0].children[0].clone();
    let plain = image("image with no title");
    assert_eq!(plain.attributes.get("title"), None);
    assert_eq!(plain.attributes.get("src"), Some(&string("/url")));
    assert_eq!(plain.attributes.get("alt"), Some(&string("foo")));
    let titled = image("image with a title");
    assert_eq!(titled.attributes.get("title"), Some(&string("title")));
    assert_eq!(titled.attributes.get("src"), Some(&string("/url")));
    assert_eq!(titled.attributes.get("alt"), Some(&string("foo")));
}

#[test]
fn attributes_for_table_with_alignments() {
    let document = convert("table with alignments");
    let (thead, tbody) = (
        &document.children[0].children[0],
        &document.children[0].children[1],
    );
    for (row, at) in [(&thead.children[0], "th"), (&tbody.children[0], "td")] {
        let aligns: Vec<_> = row
            .children
            .iter()
            .map(|c| c.attributes.get("align").cloned())
            .collect();
        assert_eq!(
            aligns,
            vec![
                Some(string("left")),
                Some(string("center")),
                Some(string("right"))
            ],
            "{at}"
        );
    }
}

// --- structure -----------------------------------------------------------------------------

#[test]
fn handling_a_header() {
    let example = convert("handling a header");
    assert_eq!(example.children.len(), 2);
    let heading = &example.children[0];
    assert_eq!(heading.node_type, NodeType::Heading);
    assert_eq!(heading.attributes.get("level"), Some(&Scalar::Number(1.0)));
    assert_eq!(heading.children[0].node_type, NodeType::Inline);
    assert_eq!(
        content(&heading.children[0].children[0]),
        Some(&string("Sample Heading"))
    );
    let paragraph = &example.children[1];
    assert_eq!(paragraph.node_type, NodeType::Paragraph);
    assert_eq!(
        content(&paragraph.children[0].children[0]),
        Some(&string("This is a sample paragraph"))
    );
}

#[test]
fn handling_an_image() {
    let example = convert("handling an image");
    let paragraph = &example.children[0];
    assert_eq!(paragraph.node_type, NodeType::Paragraph);
    assert!(paragraph.attributes.is_empty());
    let image = &paragraph.children[0].children[0];
    assert_eq!(image.node_type, NodeType::Image);
    assert_eq!(
        image.attributes,
        attrs(&[("alt", string("Alt")), ("src", string("/logo.png"))])
    );
    assert!(image.children.is_empty());
}

#[test]
fn handling_lists() {
    for (name, ordered) in [("lists with bullets", false), ("lists with numbers", true)] {
        let example = convert(name);
        let list = &example.children[0];
        assert_eq!(list.node_type, NodeType::List);
        assert_eq!(
            list.attributes.get("ordered"),
            Some(&Scalar::Boolean(ordered))
        );
        let texts: Vec<_> = list
            .children
            .iter()
            .map(|item| {
                assert_eq!(item.node_type, NodeType::Item);
                assert_eq!(item.children[0].node_type, NodeType::Inline);
                content(&item.children[0].children[0]).cloned()
            })
            .collect();
        assert_eq!(texts, vec![Some(string("foo")), Some(string("bar"))]);
    }
}

#[test]
fn handling_fenced_code() {
    let example = convert("fenced code with a language");
    assert_eq!(example.children[0].node_type, NodeType::Fence);
    assert_eq!(
        example.children[0].attributes,
        attrs(&[
            ("content", string("puts \"foo\"\n")),
            ("language", string("ruby"))
        ])
    );

    let example = convert("fenced code with an annotation");
    let fence = &example.children[0];
    assert_eq!(
        fence.attributes,
        attrs(&[
            ("content", string("test\n")),
            ("language", string("ruby")),
            ("id", string("foo")),
            (
                "class",
                Scalar::Object(attrs(&[("bar", Scalar::Boolean(true))]))
            ),
        ])
    );
}

#[test]
fn tags_at_block_level_with_a_class() {
    let example = convert("block tag with a class");
    let callout = &example.children[0];
    assert_eq!(callout.node_type, NodeType::Tag);
    assert_eq!(callout.tag.as_deref(), Some("callout"));
    assert_eq!(
        callout.attributes,
        attrs(&[(
            "class",
            Scalar::Object(attrs(&[
                ("foo", Scalar::Boolean(true)),
                ("bar", Scalar::Boolean(true))
            ]))
        )])
    );
    assert_eq!(callout.children[0].node_type, NodeType::Heading);
    assert_eq!(
        callout.children[0].attributes.get("level"),
        Some(&Scalar::Number(3.0))
    );
    assert_eq!(
        content(&callout.children[0].children[0].children[0]),
        Some(&string("Heading"))
    );
    assert_eq!(callout.children[1].node_type, NodeType::Paragraph);
    assert_eq!(
        content(&callout.children[1].children[0].children[0]),
        Some(&string("This is a paragraph"))
    );
}

#[test]
fn tags_at_block_level_with_nesting() {
    let example = convert("block tags nested");
    let outer = &example.children[0];
    let inner = &outer.children[0];
    assert_eq!(
        (outer.tag.as_deref(), inner.tag.as_deref()),
        (Some("callout"), Some("callout"))
    );
    assert_eq!(inner.children[0].node_type, NodeType::Paragraph);
    assert_eq!(
        content(&inner.children[0].children[0].children[0]),
        Some(&string("This is a test"))
    );
}

#[test]
fn annotations_in_a_header() {
    let heading = |name: &str| convert(name).children[0].clone();
    let with_id = heading("annotation id in a header");
    assert_eq!(
        with_id.attributes,
        attrs(&[("level", Scalar::Number(1.0)), ("id", string("foo-bar"))])
    );
    assert_eq!(
        content(&with_id.children[0].children[0]),
        Some(&string("Sample Heading "))
    );
    let with_class = heading("annotation class in a header");
    assert_eq!(
        with_class.attributes.get("class"),
        Some(&Scalar::Object(attrs(&[
            ("foo-bar", Scalar::Boolean(true)),
            ("test", Scalar::Boolean(true))
        ])))
    );
    let complex = heading("annotation complex values in a header");
    // Upstream's key order: the heading's own, then each annotation's, as assigned.
    assert_eq!(
        complex.attributes,
        attrs(&[
            ("level", Scalar::Number(1.0)),
            ("id", string("asdf")),
            (
                "class",
                Scalar::Object(attrs(&[
                    ("foo-bar", Scalar::Boolean(true)),
                    ("test", Scalar::Boolean(true))
                ]))
            ),
            ("foo", string("bar")),
        ])
    );
}

#[test]
fn handling_variables() {
    let variable = |path: Vec<PathSegment>| Scalar::Variable(Variable::new(path));
    let by_itself = convert("variable by itself");
    assert_eq!(
        content(&by_itself.children[0].children[0].children[0]),
        Some(&variable(vec!["test".into()]))
    );
    let in_text = convert("variable in text");
    let inline = &in_text.children[0].children[0];
    assert_eq!(
        content(&inline.children[0]),
        Some(&string("This is a test: "))
    );
    assert_eq!(inline.children[1].node_type, NodeType::Text);
    assert_eq!(
        content(&inline.children[1]),
        Some(&variable(vec!["test".into()]))
    );
    let nested = convert("variable with nested property access");
    assert_eq!(
        content(&nested.children[0].children[0].children[0]),
        Some(&variable(vec![
            "bar".into(),
            "baz".into(),
            PathSegment::Number(1.0),
            "test".into()
        ]))
    );
}

#[test]
fn nested_tags_with_indentation_do_not_throw() {
    let tokens = tokens_from_json(&fixtures()["nested tags with indentation"]["tokens"]).unwrap();
    assert!(
        parse(
            &tokens,
            source("nested tags with indentation"),
            &ParserArgs::default()
        )
        .is_ok()
    );
}

#[test]
fn parsing_comments() {
    let example = convert("comments");
    assert_eq!(example.children[0].node_type, NodeType::Paragraph);
    assert_eq!(example.children[1].node_type, NodeType::Comment);
    assert_eq!(content(&example.children[1]), Some(&string("foo")));
}

#[test]
fn attribute_errors() {
    let ids = |node: &Node| node.errors.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
    let duplicates = convert("duplicate attributes");
    assert_eq!(duplicates.children[0].tag.as_deref(), Some("foo"));
    assert_eq!(ids(&duplicates.children[0]), vec!["duplicate-attribute"; 3]);
    let ids_ = convert("duplicate ids");
    assert_eq!(ids(&ids_.children[0]), vec!["duplicate-attribute"; 2]);
    let annotation = convert("duplicate annotation values");
    assert_eq!(annotation.children[0].node_type, NodeType::Paragraph);
    assert_eq!(ids(&annotation.children[0]), vec!["duplicate-attribute"]);
    let across = convert("duplicates across annotations");
    assert_eq!(ids(&across.children[0]), vec!["duplicate-attribute"]);
    let classes = convert("multiple classes");
    assert!(classes.children[0].errors.is_empty());
}

#[test]
fn error_for_annotations_in_a_fence() {
    let example = convert("annotations in a fence");
    assert!(example.annotations.is_empty());
    assert_eq!(example.children[0].errors[0].id, "no-inline-annotations");
}

#[test]
fn identifies_inlines() {
    let example = convert("identifies inlines");
    assert!(!example.inline);
    let heading = &example.children[0];
    assert!(!heading.inline && !heading.children[0].inline);
    assert!(heading.children[0].children[0].inline);
    let foo = &example.children[1];
    assert_eq!(foo.tag.as_deref(), Some("foo"));
    assert!(!foo.inline);
    let inline = &foo.children[0].children[0];
    assert!(!foo.children[0].inline && !inline.inline);
    let kinds: Vec<_> = inline
        .children
        .iter()
        .map(|c| (c.node_type.clone(), c.tag.clone(), c.inline))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (NodeType::Text, None, true),
            (NodeType::Tag, Some("bar".into()), true),
            (NodeType::Text, None, true),
        ]
    );
    assert!(inline.children[1].children[0].inline);
    let list = &example.children[2];
    assert!(!list.inline && !list.children[0].inline && !list.children[0].children[0].inline);
    assert!(list.children[0].children[0].children[0].inline);
}

#[test]
fn structural_errors() {
    let unmatched = convert("unmatched closing tag");
    assert_eq!(unmatched.children[0].errors[0].id, "missing-closing");
    let missing_opening = convert("missing opening");
    assert_eq!(missing_opening.children[1].errors[0].id, "missing-opening");
    let missing_closing = convert("missing closing tag");
    assert_eq!(missing_closing.children[0].errors[0].id, "missing-closing");
}

fn tags_in(doc: &Node) -> Vec<String> {
    walk(doc)
        .into_iter()
        .filter(|n| n.node_type == NodeType::Tag)
        .filter_map(|n| n.tag.clone())
        .collect()
}

#[test]
fn table_keeps_the_default_if_tag() {
    let tags = tags_in(&convert("table keeps if"));
    assert!(tags.contains(&"if".to_string()));
}

#[test]
fn table_drops_unregistered_tags_by_default() {
    let tags = tags_in(&convert("table drops unregistered"));
    assert!(!tags.contains(&"if-pref".to_string()));
}

#[test]
fn table_keeps_the_conditional_tags_it_is_given() {
    let doc = parse_with(
        "table registered tags",
        &ParserArgs {
            conditional_tags: Some(vec!["if".into(), "if-pref".into()]),
            ..ParserArgs::default()
        },
    );
    let tags = tags_in(&doc);
    assert!(tags.contains(&"if".to_string()));
    assert!(tags.contains(&"if-pref".to_string()));
    assert!(!tags.contains(&"unregistered-if-tag".to_string()));
}

// --- transforms/table.test.ts (the AST side of what `validate` reports) -----------------------

fn table_syntax_errors(doc: &Node) -> Vec<(String, ValidationLevel, usize)> {
    std::iter::once(doc)
        .chain(walk(doc))
        .flat_map(|n| n.errors.iter())
        .filter(|e| e.id == "table-syntax")
        .map(|e| {
            (
                e.message.clone(),
                e.level.clone(),
                // Upstream's location: the row's, 0-based.
                e.location
                    .as_ref()
                    .and_then(|l| l["start"]["line"].as_u64())
                    .map_or(usize::MAX, |l| l as usize),
            )
        })
        .collect()
}

#[test]
fn table_errors_for_non_row_content() {
    let errors = table_syntax_errors(&convert("table: non-row content"));
    // One for the paragraph inside the conditional, one for the bare paragraph at row level,
    // each at the content (upstream's test: lines 7 and 9).
    assert_eq!(errors.len(), 2);
    for (message, level, _) in &errors {
        assert_eq!(*level, ValidationLevel::Critical);
        assert!(message.contains("paragraph") && message.contains("indented"));
    }
    let mut lines: Vec<_> = errors.iter().map(|e| e.2).collect();
    lines.sort();
    assert_eq!(lines, vec![7, 9]);
}

#[test]
fn table_no_errors_for_valid_conditionals() {
    for name in [
        "table: valid conditional rows",
        "table: conditionals within a cell",
        "table: conditional with rows and hrs",
        "table: comments in a table",
    ] {
        assert_eq!(table_syntax_errors(&convert(name)), vec![], "{name}");
    }
}

#[test]
fn table_error_for_an_invalid_tag_in_a_conditional() {
    let errors = table_syntax_errors(&convert("table: invalid tag in a conditional"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].1, ValidationLevel::Critical);
    assert!(errors[0].0.contains("tag callout"));
    // At the callout tag (upstream's test: line 5).
    assert_eq!(errors[0].2, 5);
}

#[test]
fn table_error_for_a_tag_at_row_level() {
    let errors = table_syntax_errors(&convert("table: non-conditional tag at row level"));
    assert_eq!(errors.len(), 1);
    assert!(errors[0].0.contains("tag"));
    // Upstream's test: line 4.
    assert_eq!(errors[0].2, 4);
}

// --- the conformance corpus, against upstream's ASTs --------------------------------------------

#[test]
fn conformance_asts_are_upstreams() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/next/golden/conformance.jsonl"
    );
    let text = std::fs::read_to_string(path).expect("tests/next/golden/conformance.jsonl");
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let case: Value = serde_json::from_str(line).unwrap();
        let id = case["id"].as_str().unwrap_or("?");
        let (Some(tokens), Some(ast)) = (case.get("tokens"), case.get("ast")) else {
            continue;
        };
        let args = ParserArgs {
            file: case["file"].as_str().map(str::to_owned),
            slots: case["slots"].as_bool().unwrap_or(false),
            ..ParserArgs::default()
        };
        let tokens = tokens_from_json(tokens).unwrap();
        let mine = parse(&tokens, case["code"].as_str().unwrap_or(""), &args).unwrap();
        let got = node_json(&mine);
        if let Some((at, a, b)) = first_difference(ast, &got) {
            failures.push(format!("{id}: {at}: upstream {a} | port {b}"));
        }
        checked += 1;
    }
    assert_eq!(checked, 105);
    assert!(failures.is_empty(), "{failures:#?}");
}

// --- positions --------------------------------------------------------------------------------

/// Every node with its location's text and start line (spans on).
fn spans(name: &str) -> Vec<(NodeType, Option<String>, String, usize)> {
    let src = source(name);
    let doc = located(name);
    walk(&doc)
        .into_iter()
        .map(|n| {
            let l = n.span.as_ref().expect("every node has a span");
            (
                n.node_type.clone(),
                n.tag.clone(),
                src[l.start.offset..l.end.offset].to_string(),
                l.start.line,
            )
        })
        .collect()
}

fn text_spans(name: &str) -> Vec<(String, usize)> {
    spans(name)
        .into_iter()
        .filter(|(t, ..)| *t == NodeType::Text)
        .map(|(_, _, s, line)| (s, line))
        .collect()
}

fn tag_spans(name: &str) -> Vec<String> {
    spans(name)
        .into_iter()
        .filter(|(t, ..)| *t == NodeType::Tag)
        .map(|(_, _, s, _)| s)
        .collect()
}

#[test]
fn a_nodes_span_is_the_text_it_covers() {
    assert_eq!(
        text_spans("pos: heading and prose"),
        vec![("Heading".into(), 1), ("Some prose.".into(), 3)]
    );
    let blocks: Vec<_> = spans("pos: heading and prose")
        .into_iter()
        .filter(|(t, ..)| matches!(t, NodeType::Heading | NodeType::Paragraph))
        .map(|(_, _, s, line)| (s, line))
        .collect();
    assert_eq!(
        blocks,
        vec![("# Heading\n".into(), 1), ("Some prose.\n".into(), 3)]
    );
}

#[test]
fn frontmatter_does_not_shift_the_lines_below_it() {
    assert_eq!(text_spans("pos: frontmatter"), vec![("Heading".into(), 6)]);
}

#[test]
fn a_tag_knows_where_it_is_and_not_where_its_paragraph_is() {
    assert_eq!(
        tag_spans("pos: two inline tags"),
        vec!["{% tag id=\"a\" /%}", "{% tag id=\"b\" /%}"]
    );
}

#[test]
fn prose_after_a_tag_is_where_it_is() {
    assert_eq!(
        text_spans("pos: prose after a tag"),
        vec![("Inside the callout.".into(), 2), ("After.".into(), 5)]
    );
    assert_eq!(
        tag_spans("pos: prose after a tag"),
        vec!["{% callout type=\"note\" %}"]
    );
}

#[test]
fn an_escaped_tag_opener_does_not_shift_what_follows() {
    let spans = text_spans("pos: escaped tag opener");
    assert_eq!(
        spans.first(),
        Some(&("Literal \\{% not a tag %} here.".into(), 1))
    );
    assert_eq!(spans.last(), Some(&("After.".into(), 3)));
}

#[test]
fn a_character_column_counts_utf16_units() {
    let doc = located("pos: utf-16 columns");
    let strong = walk(&doc)
        .into_iter()
        .find(|n| n.node_type == NodeType::Strong)
        .unwrap();
    let l = strong.span.as_ref().unwrap();
    assert_eq!(l.start.line, 1);
    assert_eq!(l.start.character, Some(5), "five characters precede it");
    assert_eq!(l.start.offset, 6, "six bytes precede it");
    assert_eq!(text_at(source("pos: utf-16 columns"), strong), "**bold**");
}

#[test]
fn a_byte_order_mark_is_accounted_for() {
    // Upstream keeps the mark: line 1 is a paragraph that starts with it.
    let kept = spans("pos: byte-order mark kept");
    assert_eq!(kept[0].0, NodeType::Paragraph);
    assert_eq!(kept.last().unwrap().2, "Text.");
    // Tokens of the text without it (the crate's switch), positions in the file with it.
    let src = source("pos: byte-order mark stripped");
    let doc = located("pos: byte-order mark stripped");
    let heading = &doc.children[0];
    assert_eq!(heading.node_type, NodeType::Heading);
    assert_eq!(text_at(src, heading), "# Heading\n");
    assert_eq!(heading.span.as_ref().unwrap().start.offset, 3);
    assert_eq!(
        text_spans("pos: byte-order mark stripped"),
        vec![("Heading".into(), 1), ("Text.".into(), 3)]
    );
}

#[test]
fn an_interpolation_knows_where_it_is() {
    let doc = located("pos: interpolation");
    let variable = walk(&doc)
        .into_iter()
        .find(|n| matches!(content(n), Some(Scalar::Variable(_))))
        .expect("the variable is a node");
    assert_eq!(
        text_at(source("pos: interpolation"), variable),
        "{% $bot250 %}"
    );
}

#[test]
fn a_list_tables_cells_are_located() {
    let src = source("pos: list table");
    let doc = located("pos: list table");
    let table_tag = tag(&doc, "table");
    assert_eq!(text_at(src, table_tag), "{% table %}");
    let table = &table_tag.children[0];
    assert_eq!(table.node_type, NodeType::Table);
    assert!(text_at(src, table).starts_with("{% table %}"));
    let cells: Vec<_> = walk(table)
        .into_iter()
        .filter(|n| n.node_type == NodeType::Td)
        .map(|n| text_at(src, n))
        .collect();
    assert_eq!(cells, vec!["* A\n", "* B\n"]);
    assert_eq!(
        text_spans("pos: list table"),
        vec![("Before.".into(), 1), ("A".into(), 5), ("B".into(), 6)]
    );
}

#[test]
fn an_offset_is_always_on_a_character_boundary() {
    for name in fixtures().keys() {
        let src = source(name);
        let tokens = tokens_from_json(&fixtures()[name.as_str()]["tokens"]).unwrap();
        let Ok(doc) = parse(&tokens, src, &spanned()) else {
            continue;
        };
        for n in std::iter::once(&doc).chain(walk(&doc)) {
            let l = n.span.as_ref().expect("a span");
            assert!(
                src.is_char_boundary(l.start.offset) && src.is_char_boundary(l.end.offset),
                "{name}: {:?} {}..{}",
                n.node_type,
                l.start.offset,
                l.end.offset
            );
            assert!(l.start.offset <= l.end.offset && l.end.offset <= src.len());
        }
    }
}

#[test]
fn an_unclosed_tag_is_reported_at_its_opener() {
    let doc = located("pos: unclosed tag");
    let callout = tag(&doc, "callout");
    let error = &callout.errors[0];
    assert_eq!(error.id, "missing-closing");
    assert!(error.location.is_none(), "upstream's has none");
    let at = error.span.as_ref().unwrap();
    let opener = callout.span.as_ref().unwrap();
    assert_eq!(
        (at.start.line, at.start.character, at.start.offset),
        (
            opener.start.line,
            opener.start.character,
            opener.start.offset
        )
    );
    assert_eq!(at.start.line, 3);
}

#[test]
fn a_close_with_nothing_open_is_reported_where_it_is() {
    let src = source("pos: close with nothing open");
    let doc = located("pos: close with nothing open");
    let close = tag(&doc, "callout");
    assert_eq!(close.errors[0].id, "missing-opening");
    assert_eq!(close.errors[0].span.as_ref().unwrap().start.line, 5);
    assert_eq!(text_at(src, close), "{% /callout %}");
}

#[test]
fn inline_tokens_span_their_own_syntax() {
    let all = spans("pos: inline constructs");
    let get = |ty: NodeType| -> Vec<String> {
        all.iter()
            .filter(|(t, ..)| *t == ty)
            .map(|(_, _, s, _)| s.clone())
            .collect()
    };
    assert_eq!(get(NodeType::Em), vec!["*b {% x /%}*"]);
    assert_eq!(get(NodeType::Code), vec!["`c {% y %}`"]);
    assert_eq!(
        get(NodeType::Link),
        vec!["[d {% z /%}](/u \"t\")", "<http://a.b>"]
    );
    assert_eq!(
        tag_spans("pos: inline constructs"),
        vec!["{% x /%}", "{% z /%}", "{% w /%}"]
    );
    assert_eq!(
        get(NodeType::Text),
        vec!["A ", "b ", " ", " ", "d ", " ", "http://a.b", " \\* &amp; "]
    );
}

#[test]
fn breaks_and_the_text_after_them() {
    assert_eq!(
        text_spans("pos: softbreak and hardbreak"),
        vec![
            ("one".into(), 1),
            ("two".into(), 2),
            ("three".into(), 3),
            ("four ".into(), 4)
        ]
    );
    let breaks: Vec<_> = spans("pos: softbreak and hardbreak")
        .into_iter()
        .filter(|(t, ..)| matches!(t, NodeType::Hardbreak | NodeType::Softbreak))
        .map(|(t, _, s, _)| (t, s))
        .collect();
    assert_eq!(
        breaks,
        vec![
            (NodeType::Hardbreak, "  \n".into()),
            (NodeType::Hardbreak, "\\\n".into()),
            (NodeType::Softbreak, "\n".into())
        ]
    );
    assert_eq!(tag_spans("pos: softbreak and hardbreak"), vec!["{% t /%}"]);
}

#[test]
fn containers_markers_are_skipped() {
    let all = spans("pos: blockquote and list");
    let starts: Vec<_> = all
        .iter()
        .filter(|(t, ..)| {
            matches!(
                t,
                NodeType::Blockquote | NodeType::List | NodeType::Item | NodeType::Tag
            )
        })
        .map(|(t, tag, s, line)| {
            (
                t.clone(),
                tag.clone(),
                s.lines().next().unwrap_or("").to_string(),
                *line,
            )
        })
        .collect();
    assert_eq!(
        starts,
        vec![
            (NodeType::Blockquote, None, "> - a {% t1 /%}".into(), 1),
            (NodeType::List, None, "- a {% t1 /%}".into(), 1),
            (NodeType::Item, None, "- a {% t1 /%}".into(), 1),
            (NodeType::Tag, Some("t1".into()), "{% t1 /%}".into(), 1),
            (NodeType::Tag, Some("t2".into()), "{% t2 /%}".into(), 2),
            (NodeType::Tag, Some("t3".into()), "{% t3 %}".into(), 4),
        ]
    );
    assert_eq!(
        text_spans("pos: blockquote and list"),
        vec![("a ".into(), 1), ("b ".into(), 2), ("c".into(), 5)]
    );
}

#[test]
fn table_cells_are_found_in_their_rows() {
    let src = source("pos: pipe table");
    let doc = located("pos: pipe table");
    let cells: Vec<_> = walk(&doc)
        .into_iter()
        .filter(|n| matches!(n.node_type, NodeType::Th | NodeType::Td))
        .map(|n| text_at(src, n))
        .collect();
    assert_eq!(
        cells,
        vec![" a ", " {% x /%} ", " b \\| c ", " d {% y /%} "]
    );
    assert_eq!(tag_spans("pos: pipe table"), vec!["{% x /%}", "{% y /%}"]);
    let texts = text_spans("pos: pipe table");
    assert_eq!(
        texts,
        vec![("a".into(), 1), ("b \\| c".into(), 3), ("d ".into(), 3)]
    );
}

#[test]
fn tags_in_a_fence_are_located() {
    let src = source("pos: fence with tags");
    let doc = located("pos: fence with tags");
    let fence = walk(&doc)
        .into_iter()
        .find(|n| n.node_type == NodeType::Fence)
        .unwrap();
    assert!(text_at(src, fence).starts_with("```js {% .c %}"));
    assert_eq!(
        tag_spans("pos: fence with tags"),
        vec!["{% t1 /%}", "{% t2 /%}"]
    );
    let lines: Vec<_> = walk(fence)
        .into_iter()
        .map(|n| n.span.as_ref().unwrap().start.line)
        .collect();
    assert_eq!(lines, vec![4, 4, 5, 5]);
}

#[test]
fn a_heading_with_a_closing_sequence() {
    assert_eq!(
        text_spans("pos: heading with closing hashes"),
        vec![("Title ".into(), 1)]
    );
}

#[test]
fn crlf_line_endings() {
    let src = source("pos: crlf");
    assert_eq!(tag_spans("pos: crlf"), vec!["{% t /%}", "{% u /%}"]);
    let doc = located("pos: crlf");
    assert_eq!(text_at(src, &doc.children[0]), "# A\r\n");
    assert_eq!(
        text_spans("pos: crlf"),
        vec![("A".into(), 1), ("para ".into(), 3), ("next ".into(), 4)]
    );
}

#[test]
fn a_tag_across_lines() {
    assert_eq!(
        tag_spans("pos: multi-line inline tag"),
        vec!["{% t\n> x=1 %}"]
    );
    assert_eq!(
        text_spans("pos: multi-line inline tag"),
        vec![("a ".into(), 1), (" b".into(), 2)]
    );
}

#[test]
fn reference_links() {
    assert_eq!(
        tag_spans("pos: reference link"),
        vec!["{% t /%}", "{% u /%}"]
    );
    let links: Vec<_> = spans("pos: reference link")
        .into_iter()
        .filter(|(t, ..)| *t == NodeType::Link)
        .map(|(.., s, _)| s)
        .collect();
    assert_eq!(links, vec!["[a][r]", "[b]"]);
}

#[test]
fn the_typographers_text() {
    assert_eq!(tag_spans("pos: entity and smart quotes"), vec!["{% t /%}"]);
    assert_eq!(
        text_spans("pos: entity and smart quotes"),
        vec![("\"Hi\" &copy; (c) -- ".into(), 1)]
    );
}

#[test]
fn trailing_unicode_space_is_content() {
    // A paragraph is trimmed of ASCII whitespace only: the no-break space stays its text's.
    let src = source("pos: nbsp at line end");
    let doc = located("pos: nbsp at line end");
    let text = walk(&doc)
        .into_iter()
        .find(|n| n.node_type == NodeType::Text)
        .unwrap();
    assert_eq!(text_at(src, text), "Depending\u{a0}");
}

// --- frontmatter ---------------------------------------------------------------------------

#[test]
fn frontmatter_is_parsed_in_yaml_order() {
    let doc = parse_with("fm: valid", &yaml_frontmatter());
    assert_eq!(
        doc.attributes.get("frontmatter"),
        Some(&Scalar::Object(attrs(&[
            ("b", Scalar::Number(1.0)),
            ("a", Scalar::Array(vec![string("x"), Scalar::Number(2.0)])),
            ("c", Scalar::Object(attrs(&[("d", Scalar::Boolean(true))]))),
        ])))
    );
    assert!(doc.errors.is_empty());
}

#[test]
fn frontmatter_that_is_no_mapping_stays_text_with_an_error() {
    // Off (upstream's way), the text and no error.
    let raw = located("fm: invalid");
    assert_eq!(raw.attributes.get("frontmatter"), Some(&string("Foo")));
    assert!(raw.errors.is_empty());
    let doc = parse_with(
        "fm: invalid",
        &ParserArgs {
            spans: true,
            ..yaml_frontmatter()
        },
    );
    assert_eq!(doc.attributes.get("frontmatter"), Some(&string("Foo")));
    assert_eq!(doc.errors.len(), 1);
    assert_eq!(doc.errors[0].id, "frontmatter-invalid");
    assert_eq!(doc.errors[0].level, ValidationLevel::Error);
    let at = doc.errors[0].span.as_ref().unwrap();
    assert_eq!((at.start.line, at.end.line), (1, 4));
    // The rest of the document is parsed as upstream parses it.
    assert_eq!(doc.children.len(), 1);
}

#[test]
fn empty_frontmatter_is_an_empty_object() {
    let doc = parse_with("fm: empty", &yaml_frontmatter());
    assert_eq!(
        doc.attributes.get("frontmatter"),
        Some(&Scalar::Object(IndexMap::new()))
    );
    assert!(doc.errors.is_empty());
}

// --- upstream's quirks ---------------------------------------------------------------------

#[test]
fn an_unclosed_inline_tag_leaves_its_inline_open() {
    // node: the paragraph and its inline stay open, the paragraph's close misses its
    // opening, and the next paragraph lands in the inline.
    let doc = convert("quirk: unclosed inline tag");
    assert_eq!(doc.children.len(), 1);
    let paragraph = &doc.children[0];
    assert_eq!(paragraph.errors[0].id, "missing-closing");
    let inline = &paragraph.children[0];
    assert_eq!(inline.errors[0].id, "missing-closing");
    let kinds: Vec<_> = inline
        .children
        .iter()
        .map(|c| (c.node_type.clone(), c.errors.first().map(|e| e.id.clone())))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (NodeType::Text, None),
            (NodeType::Tag, None),
            (NodeType::Paragraph, Some("missing-opening".into())),
            (NodeType::Paragraph, None),
        ]
    );
}

#[test]
fn a_class_onto_a_string_throws_as_js_does() {
    let tokens = tokens_from_json(&fixtures()["quirk: class onto a string"]["tokens"]).unwrap();
    let error = parse(
        &tokens,
        source("quirk: class onto a string"),
        &ParserArgs::default(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Cannot create property 'bar' on string 'x'")
    );
}

#[test]
fn a_class_onto_other_values() {
    let doc = convert("quirk: class onto values");
    let class = |i: usize| doc.children[i].attributes.get("class").cloned();
    // `{"2":true,"a":1,"bar":true}`: an index key first.
    assert_eq!(
        crate::next::compare::to_json_string(&class(0).unwrap().to_json()),
        r#"{"2":true,"a":1,"bar":true}"#
    );
    // `.length` makes it one long, `.3` sets index 3 (holes, which JSON writes as null).
    assert_eq!(
        crate::next::compare::to_json_string(&class(1).unwrap().to_json()),
        "[1,null,null,true]"
    );
    assert_eq!(
        class(2),
        Some(Scalar::Object(attrs(&[("bar", Scalar::Boolean(true))])))
    );
}

#[test]
fn object_prototype_names() {
    let doc = convert("quirk: prototype names");
    let first = &doc.children[0];
    assert_eq!(
        first.attributes,
        attrs(&[
            ("constructor", Scalar::Number(1.0)),
            ("toString", Scalar::Number(3.0))
        ])
    );
    assert_eq!(
        first
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Attribute 'constructor' already set",
            "Attribute '__proto__' already set",
            "Attribute 'toString' already set"
        ]
    );
    assert_eq!(
        doc.children[1].attributes.get("class"),
        Some(&Scalar::Object(attrs(&[(
            "constructor",
            Scalar::Boolean(true)
        )])))
    );
}

#[test]
fn slots_are_keyed_as_a_js_object() {
    let doc = parse_with(
        "quirk: slots",
        &ParserArgs {
            slots: true,
            ..ParserArgs::default()
        },
    );
    let foo = &doc.children[0];
    assert_eq!(foo.slots.keys().collect::<Vec<_>>(), vec!["0", "b"]);
    let b = &foo.slots["b"];
    assert_eq!(
        content(&b.children[0].children[0].children[0]),
        Some(&string("B2"))
    );
    // Without slots they are children.
    let doc = convert("quirk: slots");
    assert!(doc.children[0].slots.is_empty());
    assert_eq!(doc.children[0].children.len(), 4);
}

#[test]
fn objects_the_parser_builds_take_a_js_objects_key_order() {
    // JS: `class = {}; class.b = true; class[2] = true; class.a = true; class[1] = ...`:
    // index keys ascending first, then the others as set; the annotation that set `class`
    // holds that object. Keys come in any order without a shift per key (they are appended
    // and ordered once), an inline annotation's too.
    let (tokens, _) = tokens_with_and_without_grammar_meta(
        "{% x class={} .b .2 .a .1 .0 /%}\n\np {% x %} q {% .9 .k .3 %}\n",
    );
    let src = "{% x class={} .b .2 .a .1 .0 /%}\n\np {% x %} q {% .9 .k .3 %}\n";
    let doc = parse(&tokens, src, &ParserArgs::default()).expect("parses");
    let keys = |value: &Scalar| match value {
        Scalar::Object(o) => o.keys().cloned().collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    let tag = &doc.children[0];
    assert_eq!(keys(&tag.attributes["class"]), ["0", "1", "2", "b", "a"]);
    assert_eq!(keys(&tag.annotations[0].value), ["0", "1", "2", "b", "a"]);
    // The inline annotation's parent is the paragraph (the open tag `x` is no block).
    let paragraph = &doc.children[1];
    assert_eq!(keys(&paragraph.attributes["class"]), ["3", "9", "k"]);
    // Slots named by numbers, in a JS object's order.
    let src = "{% x %}\n{% slot \"b\" %}\nB\n{% /slot %}\n{% slot \"2\" %}\n2\n{% /slot %}\n{% slot \"0\" %}\n0\n{% /slot %}\n{% /x %}\n";
    let (tokens, _) = tokens_with_and_without_grammar_meta(src);
    let args = ParserArgs {
        slots: true,
        ..ParserArgs::default()
    };
    let doc = parse(&tokens, src, &args).expect("parses");
    assert_eq!(
        doc.children[0].slots.keys().collect::<Vec<_>>(),
        ["0", "2", "b"]
    );
    // 20,000 index keys after 20,000 named ones, the index keys descending: each is
    // appended, and the object ordered once.
    let n = 20_000;
    let named = (0..n)
        .map(|i| format!(".k{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let indices = (0..n)
        .rev()
        .map(|i| format!(".{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let src = format!("{{% x class={{}} {named} {indices} /%}}\n");
    let (tokens, _) = tokens_with_and_without_grammar_meta(&src);
    let start = std::time::Instant::now();
    let doc = parse(&tokens, &src, &ParserArgs::default()).expect("parses");
    let class = keys(&doc.children[0].attributes["class"]);
    assert_eq!(class.len(), 2 * n);
    assert_eq!((class[0].as_str(), class[n - 1].as_str()), ("0", "19999"));
    assert_eq!(
        (class[n].as_str(), class[2 * n - 1].as_str()),
        ("k0", "k19999")
    );
    // Quadratic, this was seconds even in release (a shift of every later key per key).
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn parse_errors() {
    let src = source("quirk: parse errors");
    let doc = located("quirk: parse errors");
    let block = &doc.children[0];
    assert_eq!(block.node_type, NodeType::Error);
    assert_eq!(block.errors[0].id, "parse-error");
    assert_eq!(block.errors[0].level, ValidationLevel::Critical);
    assert_eq!(text_at(src, block), "{% bad = %}");
    let inline = &doc.children[1].children[0].children[1];
    assert_eq!(inline.node_type, NodeType::Error);
    assert!(inline.inline);
    assert_eq!(text_at(src, inline), "{% bad = %}");
    assert_eq!(inline.errors[0].span.as_ref().unwrap().start.offset, 15);
    // Upstream's: `null` inline, UTF-16 offsets into the source in a block.
    assert_eq!(inline.errors[0].location, Some(Value::Null));
    assert!(block.errors[0].location.as_ref().unwrap()["start"]["offset"].is_u64());
}

#[test]
fn a_link_close_inside_an_open_tag() {
    // The tag is still open when the link closes: the close is a link node of its own
    // (`{href: undefined}`: absent from JSON, a key validate() sees), and the link stays
    // open.
    let src = source("quirk: link close inside a tag");
    let doc = located("quirk: link close inside a tag");
    let link = walk(&doc)
        .into_iter()
        .find(|n| {
            n.node_type == NodeType::Link && n.errors.iter().any(|e| e.id == "missing-opening")
        })
        .unwrap();
    assert_eq!(
        link.attributes.iter().collect::<Vec<_>>(),
        [(&"href".to_string(), &Scalar::Undefined)]
    );
    assert_eq!(node_json(link)["attributes"], serde_json::json!({}));
    assert_eq!(link.errors[0].id, "missing-opening");
    assert_eq!(text_at(src, link), "](/u)");
}

#[test]
fn degenerate_tags_span_what_the_rules_take() {
    // `{%}`: `findTagEnd` finds the `%}` that shares its `%`; the rules take all three.
    assert_eq!(
        spans("pos: degenerate tags")
            .into_iter()
            .filter(|(t, ..)| *t == NodeType::Error)
            .map(|(.., s, line)| (s, line))
            .collect::<Vec<_>>(),
        vec![
            ("{%}".to_string(), 1),
            ("{%%}".to_string(), 1),
            ("{%}".to_string(), 3)
        ]
    );
    assert_eq!(
        text_spans("pos: degenerate tags"),
        vec![("a ".into(), 1), (" b ".into(), 1), (" c".into(), 1)]
    );
}

#[test]
fn a_text_after_an_emptied_delimiter_starts_at_its_content() {
    // node: `{% s %} *a **b** *c*` closes the em with one `*` of `**`; the other is emptied
    // and joins the text after it, which still starts at its own first character.
    let texts = text_spans("pos: emptied delimiter");
    assert_eq!(
        texts,
        vec![
            (" ".into(), 1),
            ("a ".into(), 1),
            ("b".into(), 1),
            (" ".into(), 1),
            ("c".into(), 1)
        ]
    );
    let src = source("pos: emptied delimiter");
    let doc = located("pos: emptied delimiter");
    let starts: Vec<usize> = walk(&doc)
        .into_iter()
        .filter(|n| n.node_type == NodeType::Text)
        .map(|n| n.span.as_ref().unwrap().start.offset)
        .collect();
    assert_eq!(starts, vec![7, 9, 13, 16, 18], "{src:?}");
}

// --- an annotation holds the object `class` holds ------------------------------------------

/// A node's annotations as (type, name, the value's JSON).
fn annotations(node: &Node) -> Vec<(String, String, String)> {
    node.annotations
        .iter()
        .map(|a| {
            (
                a.attr_type.clone(),
                a.name.clone(),
                crate::next::compare::to_json_string(&a.value.to_json()),
            )
        })
        .collect()
}

fn triples(expected: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    expected
        .iter()
        .map(|(t, n, v)| (t.to_string(), n.to_string(), v.to_string()))
        .collect()
}

#[test]
fn an_annotation_shows_later_changes_to_the_class_it_set() {
    // node: `annotations[0].value` is the very object `attributes.class` is.
    let doc = convert("alias: class object");
    assert_eq!(
        annotations(&doc.children[0]),
        triples(&[
            ("attribute", "class", r#"{"a":1,"b":true}"#),
            ("class", "b", "true")
        ])
    );
    // `length = true` is `length = 1`.
    let doc = convert("alias: class array length");
    assert_eq!(
        annotations(&doc.children[0]),
        triples(&[("attribute", "class", "[1]"), ("class", "length", "true")])
    );
}

#[test]
fn a_later_inline_annotation_changes_the_class_an_earlier_one_set() {
    let doc = convert("alias: inline annotations");
    assert_eq!(
        annotations(&doc.children[0]),
        triples(&[
            ("attribute", "class", r#"{"a":true,"b":true}"#),
            ("class", "b", "true")
        ])
    );
    // The inline parent is a tag (a tight item's paragraph is hidden): the tag's own
    // annotation shows the change.
    let doc = convert("alias: tag as inline parent");
    assert_eq!(
        annotations(tag(&doc, "x")),
        triples(&[
            ("attribute", "class", r#"{"a":1,"b":true}"#),
            ("class", "b", "true")
        ])
    );
}

#[test]
fn a_replaced_class_leaves_its_annotation_as_it_was() {
    let doc = convert("alias: class replaced");
    assert_eq!(
        annotations(&doc.children[0]),
        triples(&[
            ("attribute", "class", r#"{"a":1,"b":true}"#),
            ("class", "b", "true"),
            ("attribute", "class", r#"{"c":2,"d":true}"#),
            ("class", "d", "true")
        ])
    );
    // An object `.b` makes is shared with no annotation.
    assert_eq!(
        annotations(&doc.children[1]),
        triples(&[("class", "b", "true"), ("class", "c", "true")])
    );
    assert_eq!(
        doc.children[1].attributes["class"],
        Scalar::Object(attrs(&[
            ("b", Scalar::Boolean(true)),
            ("c", Scalar::Boolean(true))
        ]))
    );
    assert_eq!(
        annotations(&doc.children[2]),
        triples(&[
            ("attribute", "class", "[1,null,true]"),
            ("class", "2", "true"),
            ("attribute", "class", "null"),
            ("class", "d", "true")
        ])
    );
}

// --- `.name` onto a string, number or boolean `class` ----------------------------------------

/// The message of the error parsing a fixture throws.
fn parse_error(name: &str) -> String {
    let tokens = tokens_from_json(&fixtures()[name]["tokens"]).unwrap();
    match parse(&tokens, source(name), &ParserArgs::default()) {
        Err(crate::types::MarkdocError::ParseError(message)) => message,
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn a_class_onto_a_primitive_throws_what_v8_throws() {
    // node, strict mode: `const o = value; o[name] = true`.
    for (name, message) in [
        (
            "primitive: string length",
            "Cannot assign to read only property 'length' of string 'abc'",
        ),
        (
            "primitive: string index",
            "Cannot assign to read only property '2' of string 'abc'",
        ),
        (
            "primitive: string past its end",
            "Cannot create property '3' on string 'abc'",
        ),
        (
            "primitive: string utf-16 index",
            "Cannot assign to read only property '1' of string '\u{1F600}'",
        ),
        (
            "primitive: string non-canonical index",
            "Cannot create property '01' on string 'abc'",
        ),
        (
            "primitive: string minus zero",
            "Cannot create property '-0' on string 'abc'",
        ),
        (
            "primitive: number length",
            "Cannot create property 'length' on number '1.5'",
        ),
        (
            "primitive: number in exponent form",
            "Cannot create property 'b' on number '1e+21'",
        ),
        (
            "primitive: boolean index",
            "Cannot create property '0' on boolean 'true'",
        ),
    ] {
        assert_eq!(parse_error(name), format!("TypeError: {message}"), "{name}");
    }
}

#[test]
fn proto_onto_a_primitive_class_does_nothing() {
    // Object.prototype's `__proto__` setter returns at once on a primitive: no error.
    let doc = convert("primitive: __proto__");
    let classes: Vec<_> = doc
        .children
        .iter()
        .map(|x| (x.attributes["class"].clone(), x.errors.len()))
        .collect();
    assert_eq!(
        classes,
        vec![
            (string("s"), 0),
            (Scalar::Number(1.0), 0),
            (Scalar::Boolean(true), 0)
        ]
    );
}

// --- holes, prototypes, extra properties: tests/next_model.rs pins them against node -----------

// --- the table transform's walk --------------------------------------------------------------

#[test]
fn a_table_in_a_slot_and_a_table_in_a_cell_are_transformed() {
    let doc = parse_with(
        "table: in a slot and in a cell",
        &ParserArgs {
            slots: true,
            ..ParserArgs::default()
        },
    );
    let tables: Vec<_> = walk(&doc)
        .into_iter()
        .filter(|n| n.node_type == NodeType::Tag && n.tag.as_deref() == Some("table"))
        .map(|n| {
            n.children
                .iter()
                .map(|c| c.node_type.clone())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(tables, vec![vec![NodeType::Table], vec![NodeType::Table]]);
}

/// The table transform over a chain `depth` tags deep with a `{% table %}` (two one-item
/// lists) at the bottom, on a thread with a 256 KB stack: how many levels down the table
/// tag is and what its table holds, or the error.
fn walk_a_chain(depth: usize) -> Result<(usize, Vec<NodeType>), String> {
    let worker = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let item = |text: &str| {
                Node::new(
                    NodeType::Item,
                    IndexMap::new(),
                    vec![Node::new(
                        NodeType::Text,
                        attrs(&[("content", string(text))]),
                        Vec::new(),
                        None,
                    )],
                    None,
                )
            };
            let list =
                |text: &str| Node::new(NodeType::List, IndexMap::new(), vec![item(text)], None);
            let mut node = Node::new(
                NodeType::Tag,
                IndexMap::new(),
                vec![list("a"), list("b")],
                Some("table".into()),
            );
            for _ in 0..depth {
                node = Node::new(NodeType::Tag, IndexMap::new(), vec![node], Some("x".into()));
            }
            let mut doc = Node::new(NodeType::Document, IndexMap::new(), vec![node], None);
            crate::next::transforms::table::transform(&mut doc, None).map_err(|e| e.to_string())?;

            let mut levels = 0;
            let mut bottom = &doc;
            while let Some(child) = bottom.children.first() {
                if bottom.tag.as_deref() == Some("table") {
                    break;
                }
                bottom = child;
                levels += 1;
            }
            let shape: Vec<NodeType> = bottom
                .children
                .iter()
                .flat_map(|t| t.children.iter().map(|c| c.node_type.clone()))
                .collect();
            Ok((levels, shape))
            // The tree drops here, a node at a time (Node's own Drop).
        })
        .expect("a thread");
    worker.join().expect("no overflow")
}

#[test]
fn the_table_transform_walks_a_tree_to_its_depth_limit() {
    use super::MAX_AST_DEPTH;
    // Upstream's walk is a recursive generator; this one keeps its stack on the heap, so it
    // runs on a 256 KB stack, and stops where upstream's runs out of stack. Transformed, a
    // table's text is 5 levels below its tag (table, tbody, tr, td, text) and so 6 below the
    // chain: at the limit with a chain MAX_AST_DEPTH - 6 deep.
    assert_eq!(
        walk_a_chain(MAX_AST_DEPTH - 6),
        Ok((MAX_AST_DEPTH - 5, vec![NodeType::Thead, NodeType::Tbody]))
    );
    // One level more: the table's text is past the limit only once the table is
    // transformed (the walk sees what the transform made, as upstream's does).
    let too_deep = Err("Parse error: RangeError: Maximum call stack size exceeded".to_string());
    assert_eq!(walk_a_chain(MAX_AST_DEPTH - 5), too_deep);
    assert_eq!(walk_a_chain(200_000), too_deep);
}

// --- positions beside escapes ----------------------------------------------------------------

#[test]
fn a_code_span_after_an_escaped_backtick() {
    // `\`` is text: the run after it opens the code span.
    let s: Vec<_> = spans("pos: escaped backtick before a code span")
        .into_iter()
        .filter(|(t, ..)| matches!(t, NodeType::Code | NodeType::Tag))
        .map(|(t, _, s, _)| (t, s))
        .collect();
    assert_eq!(
        s,
        vec![
            (NodeType::Code, "`a`".to_string()),
            (NodeType::Tag, "{% x /%}".to_string()),
            (NodeType::Code, "`b`".to_string())
        ]
    );
}

#[test]
fn a_tag_beside_an_escaped_pipe_in_a_cell() {
    // The cell's content is `{% x /%}|`: the tag ends before the backslash, and the text
    // `|` is the two characters it came from.
    assert_eq!(tag_spans("pos: escaped pipe in a cell"), vec!["{% x /%}"]);
    assert_eq!(
        text_spans("pos: escaped pipe in a cell"),
        vec![("\\|".into(), 1)]
    );
    // With a NUL in the cell (U+FFFD in markdown-it's content).
    assert_eq!(
        tag_spans("pos: NUL and an escaped pipe in a cell"),
        vec!["{% x /%}"]
    );
    assert_eq!(
        text_spans("pos: NUL and an escaped pipe in a cell"),
        vec![("a\\|".into(), 1), ("\0".into(), 1)]
    );
}

// --- upstream's own ASTs (fixtures.cjs's `oracle` cases) --------------------------------------

/// The fixtures that carry upstream's AST (or the error it threw), parsed by the port with
/// the same arguments: every one equal to node's, whole (`lines`, `location` and errors
/// included), or throwing node's message. Among them the hand-made streams of `pos:` (a
/// closing's map pushed onto the lines nodes share, `position` values copied as they are, the
/// property a popped document's `undefined` is read for) and `errors:` (a token's `errors`
/// entries kept as they are, a `meta.error` that is not one Markdoc makes).
#[test]
fn the_oracle_fixtures_are_upstreams() {
    let mut checked = 0;
    let mut failures = Vec::new();
    for (name, case) in fixtures() {
        if case.get("ast").is_none() && case.get("threw").is_none() {
            continue;
        }
        checked += 1;
        let tokens = tokens_from_json(&case["tokens"]).expect("fixture tokens");
        let args = ParserArgs {
            slots: case["slots"].as_bool().unwrap_or(false),
            file: case["args"]["file"].as_str().map(String::from),
            location: case["args"]["location"].as_bool().unwrap_or(true),
            ..ParserArgs::default()
        };
        match (parse(&tokens, source(name), &args), case.get("ast")) {
            (Ok(doc), Some(ast)) => {
                if let Some((at, a, b)) = first_difference(ast, &node_json(&doc)) {
                    failures.push(format!("{name}: {at}: upstream {a} | port {b}"));
                }
            }
            (Err(e), None) => {
                // Both are JS's `String(error)`: `Name: message`.
                let theirs = case["threw"].as_str().unwrap_or_default();
                let ours = e.to_string();
                let ours = ours.strip_prefix("Parse error: ").unwrap_or(&ours);
                if ours != theirs {
                    failures.push(format!("{name}: threw {ours:?}, upstream {theirs:?}"));
                }
            }
            (Ok(_), None) => failures.push(format!("{name}: upstream threw {}", case["threw"])),
            (Err(e), Some(_)) => failures.push(format!("{name}: the port threw {e}")),
        }
    }
    assert_eq!(checked, 47);
    assert!(failures.is_empty(), "{failures:#?}");
}

// --- an own `__proto__` key ---------------------------------------------------------------------

#[test]
fn an_own_proto_key_is_set_by_a_proto_annotation() {
    // node: `{% x class={__proto__: 1} .__proto__ /%}` makes `class` (and the annotation
    // that set it) `{"__proto__": true}`: the grammar's `{[key]: value}` gives the object an
    // own `__proto__`, which `class[name] = value` then sets. Without one (`{a: 1}`, or a
    // `__proto__` after the first key, which `Object.assign` hands to the setter) it is
    // the prototype setter: nothing changes.
    let class = |name: &str| {
        let doc = convert(name);
        let class = doc.children[0].attributes.get("class").cloned();
        crate::next::compare::to_json_string(&class.expect("a class").to_json())
    };
    assert_eq!(class("proto: own key"), r#"{"__proto__":true}"#);
    // The annotation that set the class holds the same object.
    let doc = convert("proto: own key");
    let x = &doc.children[0];
    assert_eq!(x.annotations[0].value, x.attributes["class"]);
    assert_eq!(class("proto: own string key"), r#"{"__proto__":true}"#);
    assert_eq!(class("proto: own key twice"), r#"{"__proto__":true}"#);
    assert_eq!(
        class("proto: computed key, then again"),
        r#"{"__proto__":true,"b":true}"#
    );
    assert_eq!(
        class("proto: own key keeps its place"),
        r#"{"a":1,"b":3,"c":true}"#
    );
    assert_eq!(class("proto: no own key"), r#"{"a":1}"#);
    // Inline: the paragraph's class, set by its first annotation, changed by the later ones.
    let doc = convert("proto: own key, inline twice");
    let paragraph = &doc.children[0];
    assert_eq!(
        crate::next::compare::to_json_string(&paragraph.attributes["class"].to_json()),
        r#"{"__proto__":true}"#
    );
}

// --- depth: tokens nested in `children`, and the tree --------------------------------------------

/// `n` inline tokens nested in `children`, `leaf` in the innermost: what only hand-made
/// tokens look like (`nest: ...` fixtures, at depth 3).
fn nested_inline(n: usize, leaf: Token) -> Vec<Token> {
    let mut token = leaf;
    for _ in 0..n {
        let mut outer = Token::new("inline", "", 0);
        outer.children = Some(vec![token]);
        token = outer;
    }
    vec![token]
}

/// Takes tokens apart one at a time: `Token`'s own drop recurses into `children`.
fn drop_tokens(tokens: Vec<Token>) {
    let mut stack = tokens;
    let mut values = Vec::new();
    while let Some(mut token) = stack.pop() {
        stack.extend(token.children.take().into_iter().flatten());
        values.extend(token.meta.take());
    }
    // serde_json's own drop recurses.
    while let Some(value) = values.pop() {
        match value {
            Value::Array(items) => values.extend(items),
            Value::Object(map) => values.extend(map.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

/// How many levels below the document the tree's deepest node is.
fn depth(doc: &Node) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(doc, 0)];
    while let Some((node, d)) = stack.pop() {
        deepest = deepest.max(d);
        stack.extend(
            node.slots
                .values()
                .chain(&node.children)
                .map(|c| (c, d + 1)),
        );
    }
    deepest
}

/// `parse` on a thread with a 2 MiB stack (what every test thread has, and the robustness
/// policy's), the tree's depth or the error; the tree and the tokens are dropped there.
fn parse_on_a_2_mib_stack(tokens: Vec<Token>, src: String, slots: bool) -> Result<usize, String> {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let args = ParserArgs {
                slots,
                ..ParserArgs::default()
            };
            let result = parse(&tokens, &src, &args)
                .map(|doc| depth(&doc))
                .map_err(|e| e.to_string());
            drop_tokens(tokens);
            result
        })
        .expect("a thread")
        .join()
        .expect("no overflow")
}

const RANGE_ERROR: &str = "Parse error: RangeError: Maximum call stack size exceeded";

#[test]
fn nested_tokens_parse_to_the_nesting_limit() {
    use super::MAX_TOKEN_NESTING;
    let text = |content: &str| {
        let mut t = Token::new("text", "", 0);
        t.content = content.into();
        t
    };
    // node (a fresh process): a text token MAX_TOKEN_NESTING levels down parses, to a tree
    // one level deeper (each inline token a node, the text in the innermost)...
    assert_eq!(
        parse_on_a_2_mib_stack(
            nested_inline(MAX_TOKEN_NESTING, text("x")),
            String::new(),
            false
        ),
        Ok(MAX_TOKEN_NESTING + 1)
    );
    // ...and one level deeper is V8's RangeError, whatever the token (node's fresh process
    // still parses a hidden one there: not modeled).
    let mut hidden = text("x");
    hidden.hidden = true;
    for leaf in [text("x"), hidden] {
        assert_eq!(
            parse_on_a_2_mib_stack(
                nested_inline(MAX_TOKEN_NESTING + 1, leaf),
                String::new(),
                false
            ),
            Err(RANGE_ERROR.into())
        );
    }
    // Far deeper costs no stack: the walk over `children` is iterative.
    assert_eq!(
        parse_on_a_2_mib_stack(nested_inline(100_000, text("x")), String::new(), false),
        Err(RANGE_ERROR.into())
    );
}

/// `units` copies of the first unit of a fixture's tokens: `head` tokens, then `per` tokens
/// a unit (its maps `lines` further down each time), then the fixture's last `tail` tokens;
/// a map of the head reaching the end grows to the last line.
fn repeated(
    name: &str,
    head: usize,
    per: usize,
    lines: usize,
    tail: usize,
    units: usize,
) -> Vec<Token> {
    let fixture = tokens_from_json(&fixtures()[name]["tokens"]).expect("fixture tokens");
    let mut tokens: Vec<Token> = fixture[..head].to_vec();
    for token in &mut tokens {
        if let Some(map) = token.map.as_mut() {
            map[1] = units * lines;
        }
    }
    for k in 0..units {
        for token in &fixture[head..head + per] {
            let mut token = token.clone();
            if let Some(map) = token.map.as_mut() {
                map[0] += k * lines;
                map[1] += k * lines;
            }
            tokens.push(token);
        }
    }
    tokens.extend_from_slice(&fixture[fixture.len() - tail..]);
    tokens
}

#[test]
fn the_tree_parses_to_its_depth_limit() {
    use super::MAX_AST_DEPTH;
    use crate::next::compare::tokens_json;
    // An inline tag left open leaves its inline node and its block open, so each paragraph
    // (or tight list item) nests two levels deeper: n paragraphs are 2n + 2 deep, n items
    // 2n + 3. The tokens are node's for one, repeated (checked against node's for three).
    let paragraphs = |n: usize| repeated("depth: paragraphs", 0, 3, 2, 0, n);
    let items = |n: usize| repeated("depth: tight items", 1, 5, 1, 1, n);
    for (name, tokens) in [
        ("depth: paragraphs", paragraphs(3)),
        ("depth: tight items", items(3)),
    ] {
        assert_eq!(tokens_json(&tokens), fixtures()[name]["tokens"], "{name}");
    }
    let n = (MAX_AST_DEPTH - 3) / 2;
    // node (a fresh process, `parse` called from the top level of a script): 2,320
    // paragraphs, 4,642 deep, and 2,320 tight items, 4,643 deep, parse...
    assert_eq!(
        parse_on_a_2_mib_stack(paragraphs(n), "p {% x %} q\n\n".repeat(n), false),
        Ok(MAX_AST_DEPTH - 1)
    );
    assert_eq!(
        parse_on_a_2_mib_stack(items(n), "* a {% x %} b\n".repeat(n), false),
        Ok(MAX_AST_DEPTH)
    );
    // ...and 2,321 of either, 4,644 and 4,645 deep, throw.
    assert_eq!(
        parse_on_a_2_mib_stack(paragraphs(n + 1), "p {% x %} q\n\n".repeat(n + 1), false),
        Err(RANGE_ERROR.into())
    );
    assert_eq!(
        parse_on_a_2_mib_stack(items(n + 1), "* a {% x %} b\n".repeat(n + 1), false),
        Err(RANGE_ERROR.into())
    );
    // Far deeper is the same error, and costs no stack.
    assert_eq!(
        parse_on_a_2_mib_stack(paragraphs(30_000), "p {% x %} q\n\n".repeat(30_000), false),
        Err(RANGE_ERROR.into())
    );
}

#[test]
fn deep_meta_values_parse_to_the_value_limit() {
    use super::MAX_VALUE_DEPTH;
    // A hand-made tag token whose attribute value nests `depth` deep (arrays and objects in
    // turn). Upstream's parser keeps any such value (fixture-free: it never looks inside);
    // so does the port where the stack grows, and where it cannot it stops at
    // MAX_VALUE_DEPTH.
    let tag = |depth: usize| {
        // (built without `json!`, which copies its operands through serde, recursively)
        let mut value = serde_json::json!("x");
        for i in 0..depth {
            value = if i % 2 == 0 {
                Value::Array(vec![value])
            } else {
                Value::Object(serde_json::Map::from_iter([("k".to_string(), value)]))
            };
        }
        let mut attribute = serde_json::json!({"type": "attribute", "name": "a"});
        attribute["value"] = value;
        let mut meta = serde_json::json!({"tag": "x"});
        meta["attributes"] = Value::Array(vec![attribute]);
        let mut token = Token::new("tag", "", 0);
        token.meta = Some(meta);
        vec![token]
    };
    let grows = crate::depth::can_grow();
    let past = if grows { 100_000 } else { MAX_VALUE_DEPTH + 1 };
    assert_eq!(
        parse_on_a_2_mib_stack(tag(MAX_VALUE_DEPTH), String::new(), false),
        Ok(1)
    );
    let outcome = if grows {
        Ok(1)
    } else {
        Err(RANGE_ERROR.to_string())
    };
    assert_eq!(
        parse_on_a_2_mib_stack(tag(past), String::new(), false),
        outcome
    );
    // The same for a text token's `meta.variable`.
    let mut text = Token::new("text", "", 0);
    text.content = "x".into();
    let mut variable = serde_json::json!(1);
    for _ in 0..past {
        variable = Value::Array(vec![variable]);
    }
    let mut meta = serde_json::json!({});
    meta["variable"] = variable;
    text.meta = Some(meta);
    assert_eq!(
        parse_on_a_2_mib_stack(vec![text], String::new(), false),
        outcome
    );
}

/// The tokens of `src` (the default tokenizer), and the same tokens without
/// [`Token::grammar_meta`], as tokens read from JSON are.
fn tokens_with_and_without_grammar_meta(src: &str) -> (Vec<Token>, Vec<Token>) {
    let tokens = crate::next::tokenizer::Tokenizer::default()
        .tokenize(src)
        .expect("tokens");
    let mut json_only = tokens.clone();
    let mut stack: Vec<&mut Token> = json_only.iter_mut().collect();
    while let Some(token) = stack.pop() {
        token.grammar_meta = None;
        stack.extend(token.children.iter_mut().flatten());
    }
    (tokens, json_only)
}

#[test]
fn the_grammars_deepest_values_parse_from_grammar_meta_and_from_json() {
    // The deepest values the tag grammar makes (MAX_NESTING, 300 nested values): 299 calls,
    // arrays or hashes in one another with a variable in the last. Both ways into the
    // parser (the grammar's own values, and the token JSON alone) parse them, to the same
    // AST; upstream's grammar parses deeper still (the grammar's documented gap).
    let call = format!("{}$x{}", "f(".repeat(299), ")".repeat(299));
    let array = format!("{}$x{}", "[".repeat(299), "]".repeat(299));
    let hash = format!("{}$x{}", "{a: ".repeat(299), "}".repeat(299));
    for value in [&call, &array, &hash] {
        for src in [
            format!("{{% x a={value} /%}}\n"),
            format!("p {{% {value} %}}\n"),
        ] {
            let (tokens, json_only) = tokens_with_and_without_grammar_meta(&src);
            let args = ParserArgs::default();
            let from_grammar = parse(&tokens, &src, &args).expect("from grammar_meta");
            let from_json = parse(&json_only, &src, &args).expect("from the JSON meta");
            assert_eq!(
                first_difference(&node_json(&from_grammar), &node_json(&from_json)),
                None,
                "{src}"
            );
            for tokens in [tokens, json_only] {
                assert!(parse_on_a_2_mib_stack(tokens, src.clone(), false).is_ok());
            }
        }
    }
}

#[test]
fn hand_made_grammar_meta_parses_to_the_value_limit() {
    use super::MAX_VALUE_DEPTH;
    use crate::grammar;
    // A tag token whose grammar values (`Token::grammar_meta`, beside its JSON) nest `depth`
    // deep, in calls, arrays and hashes in turn: where the stack cannot grow the grammar
    // never makes one past MAX_VALUE_DEPTH, a hand-made one is RangeError there, as a deep
    // JSON `meta` is; where it grows, any depth parses (as upstream's).
    let tag = |depth: usize| {
        let mut value = grammar::Value::Variable(grammar::Variable {
            path: vec![grammar::Value::String("x".into())],
        });
        for i in 1..depth {
            value = match i % 3 {
                0 => grammar::Value::Array(vec![value]),
                1 => {
                    let mut parameters = grammar::Object::new();
                    parameters.set("0".into(), value);
                    grammar::Value::Function(grammar::Function {
                        name: "f".into(),
                        parameters,
                    })
                }
                _ => {
                    let mut object = grammar::Object::new();
                    object.set("a".into(), value);
                    grammar::Value::Object(object)
                }
            };
        }
        let mut token = Token::new("tag", "", 0);
        token.meta = Some(serde_json::json!({
            "tag": "x",
            "attributes": [{"type": "attribute", "name": "a", "value": null}],
        }));
        token.grammar_meta = Some(Box::new(grammar::Meta {
            tag: Some("x".into()),
            attributes: Some(Some(vec![grammar::Attribute {
                kind: grammar::AttributeType::Attribute,
                name: "a".into(),
                value,
            }])),
            variable: None,
        }));
        vec![token]
    };
    assert_eq!(
        parse_on_a_2_mib_stack(tag(MAX_VALUE_DEPTH), String::new(), false),
        Ok(1)
    );
    if crate::depth::can_grow() {
        // (as deep as a grammar value's own recursive drop takes on this stack)
        assert_eq!(
            parse_on_a_2_mib_stack(tag(3_000), String::new(), false),
            Ok(1)
        );
    } else {
        assert_eq!(
            parse_on_a_2_mib_stack(tag(MAX_VALUE_DEPTH + 1), String::new(), false),
            Err(RANGE_ERROR.into())
        );
    }
}

#[test]
fn hand_made_meta_as_upstream_reads_it() {
    // node (findings/conformance/work/fix-parser/handmade*.cjs): what upstream's parser makes
    // of a `meta` Markdoc's tokenizer never writes. An annotation token in a paragraph, and
    // a tag, each with `meta.attributes` as given.
    let annotation = |attributes: Option<Value>| {
        let mut open = Token::new("paragraph_open", "p", 1);
        open.block = true;
        let mut inline = Token::new("inline", "", 0);
        let mut text = Token::new("text", "", 0);
        text.content = "a".into();
        let mut annotation = Token::new("annotation", "", 0);
        annotation.meta = attributes.map(|a| serde_json::json!({ "attributes": a }));
        inline.children = Some(vec![text, annotation]);
        vec![open, inline, Token::new("paragraph_close", "p", -1)]
    };
    let tag = |attributes: Value| {
        let mut tag = Token::new("tag", "", 0);
        tag.meta = Some(serde_json::json!({ "tag": "x", "attributes": attributes }));
        vec![tag]
    };
    let parsed = |tokens: Vec<Token>| parse(&tokens, "", &ParserArgs::default());
    let threw = |tokens: Vec<Token>| parsed(tokens).unwrap_err().to_string();
    // `TypeError: attributes is not iterable`, whatever the value (null, missing, a number,
    // a boolean, an object).
    for attributes in [
        Some(Value::Null),
        None,
        Some(serde_json::json!(5)),
        Some(serde_json::json!(true)),
        Some(serde_json::json!({"a": 1})),
    ] {
        assert_eq!(
            threw(annotation(attributes)),
            "Parse error: TypeError: attributes is not iterable"
        );
    }
    assert_eq!(
        threw(tag(serde_json::json!({"a": 1}))),
        "Parse error: TypeError: attributes is not iterable"
    );
    // `TypeError: Cannot destructure property 'name' of 'attribute' as it is null.`
    assert_eq!(
        threw(tag(serde_json::json!([null]))),
        "Parse error: TypeError: Cannot destructure property 'name' of 'attribute' as it is null."
    );
    // A string's characters, a number or an array: annotations upstream, as they are, and
    // nothing else.
    for (attributes, annotations) in [
        (serde_json::json!("ab"), serde_json::json!(["a", "b"])),
        (
            serde_json::json!([5, [1, [2, null]]]),
            serde_json::json!([5, [1, [2, null]]]),
        ),
    ] {
        let doc = parsed(tag(attributes)).expect("parses");
        let x = &doc.children[0];
        assert!(x.attributes.is_empty() && x.errors.is_empty());
        assert_eq!(node_json(x)["annotations"], annotations);
    }
    let doc = parsed(annotation(Some(serde_json::json!("ab")))).expect("parses");
    assert!(doc.children[0].attributes.is_empty());
    // A missing name is `undefined`: `{"undefined": 2}`, with `Attribute 'undefined' already
    // set`; a class `{"undefined": true}`.
    let doc = parsed(tag(serde_json::json!([
        {"type": "attribute", "value": 1},
        {"type": "attribute", "value": 2},
    ])))
    .expect("parses");
    let x = &doc.children[0];
    assert_eq!(x.attributes, attrs(&[("undefined", Scalar::Number(2.0))]));
    assert_eq!(x.errors[0].message, "Attribute 'undefined' already set");
    let doc = parsed(tag(serde_json::json!([{"type": "class", "value": true}]))).expect("parses");
    assert_eq!(
        doc.children[0].attributes,
        attrs(&[(
            "class",
            Scalar::Object(attrs(&[("undefined", Scalar::Boolean(true))]))
        )])
    );
    // A missing value is `undefined` (findings/conformance/work/join/undefined_value.cjs):
    // the key is there (validate() sees it), JSON leaves it and the annotation's `value`
    // out, and it is not "already set".
    let doc = parsed(tag(serde_json::json!([
        {"type": "attribute", "name": "a"},
        {"type": "attribute", "name": "a", "value": 2},
    ])))
    .expect("parses");
    let x = &doc.children[0];
    assert_eq!(x.attributes, attrs(&[("a", Scalar::Number(2.0))]));
    assert!(x.errors.is_empty());
    assert_eq!(
        node_json(x)["annotations"],
        serde_json::json!([
            {"type": "attribute", "name": "a"},
            {"type": "attribute", "name": "a", "value": 2},
        ])
    );
    let doc = parsed(tag(serde_json::json!([{"type": "attribute", "name": "a"}]))).expect("parses");
    assert_eq!(
        doc.children[0].attributes,
        attrs(&[("a", Scalar::Undefined)])
    );
    assert_eq!(
        node_json(&doc.children[0])["attributes"],
        serde_json::json!({})
    );
    let doc = parsed(tag(serde_json::json!([{"type": "class", "name": "c"}]))).expect("parses");
    assert_eq!(
        node_json(&doc.children[0])["attributes"],
        serde_json::json!({"class": {}})
    );
}

// --- spans are opt-in ---------------------------------------------------------------------------

/// How many nodes of `doc` (itself included) and errors on them have a span.
fn spans_in(doc: &Node) -> usize {
    std::iter::once(doc)
        .chain(doc.walk())
        .map(|n| {
            usize::from(n.span.is_some()) + n.errors.iter().filter(|e| e.span.is_some()).count()
        })
        .sum()
}

/// `node` with every span taken out, its errors' included.
fn without_spans(node: &mut Node) {
    node.span = None;
    for error in &mut node.errors {
        error.span = None;
    }
    for child in node.children.iter_mut().chain(node.slots.values_mut()) {
        without_spans(child);
    }
    if let Some(proto) = node.slots_prototype.as_deref_mut() {
        without_spans(proto);
    }
}

#[test]
fn spans_are_off_by_default_and_change_nothing_else() {
    for (name, case) in fixtures() {
        let tokens = tokens_from_json(&case["tokens"]).expect("fixture tokens");
        let src = source(name);
        for (off, on) in [
            (ParserArgs::default(), spanned()),
            (
                yaml_frontmatter(),
                ParserArgs {
                    spans: true,
                    ..yaml_frontmatter()
                },
            ),
        ] {
            match (parse(&tokens, src, &off), parse(&tokens, src, &on)) {
                (Ok(off), Ok(mut on)) => {
                    // Off: no node and no error has a span, and the crate's JSON has no `span`.
                    assert_eq!(spans_in(&off), 0, "{name}");
                    // (A fixture whose values JSON cannot hold, as JS's cannot, is left out.)
                    if let Ok(json) = serde_json::to_string(&off) {
                        assert!(!json.contains("\"span\":"), "{name}: {json}");
                    }
                    // On: every node has one.
                    assert!(
                        std::iter::once(&on)
                            .chain(on.walk())
                            .all(|n| n.span.is_some())
                    );
                    // Upstream's JSON (`lines`, `location`, the errors) is the same either way,
                    // and so is the whole tree once the spans are taken out.
                    assert_eq!(node_json(&off), node_json(&on), "{name}");
                    without_spans(&mut on);
                    assert_eq!(format!("{off:?}"), format!("{on:?}"), "{name}");
                }
                (Err(off), Err(on)) => assert_eq!(format!("{off:?}"), format!("{on:?}")),
                (off, on) => panic!("{name}: {off:?} / {on:?}"),
            }
        }
    }
}

#[test]
fn validate_errors_have_spans_only_with_spans_on() {
    use crate::next::index::{parse as parse_text, validate};
    use crate::next::schema::Config;
    let src =
        "{% unknown %}\n\n{% if %}\nx\n\n# T {% .a %}\n\n{% /missing %}\n\n{% x a=1 a=2 /%}\n";
    let config = Config::default();
    let off = validate(&parse_text(src, &ParserArgs::default()).unwrap(), &config).unwrap();
    let on = validate(&parse_text(src, &spanned()).unwrap(), &config).unwrap();
    assert!(off.len() >= 3, "{off:?}");
    assert_eq!(off.len(), on.len());
    for (off, on) in off.iter().zip(&on) {
        assert!(off.span.is_none() && off.error.span.is_none(), "{off:?}");
        assert!(on.span.is_some(), "{on:?}");
        // Upstream's fields are the same either way.
        assert_eq!(
            (&off.node_type, &off.lines, &off.location),
            (&on.node_type, &on.lines, &on.location)
        );
        assert_eq!(off.error.id, on.error.id);
        assert_eq!(off.error.message, on.error.message);
        assert_eq!(off.error.location, on.error.location);
    }
}
