//! Where `markdoc::next` throws `RangeError: Maximum call stack size exceeded`, per nesting
//! shape: the port's side of `findings/conformance/work/depth/node_shapes.cjs` (the same
//! shape names, inputs and measured calls).
//!
//! ```text
//! next_depth list                          the shape names
//! next_depth probe SHAPE N [STACK_KIB]     one run on a thread with STACK_KIB of stack
//!                                          (default 2048): {"shape","n","r"}
//! next_depth search SHAPE [HI] [STACK_KIB] the deepest n that succeeds (bisection, one
//!                                          process): {"shape","port","next"}
//! next_depth time SHAPE N [STACK_KIB]      the run's wall time in ms and its result
//! ```
//!
//! `r` is `ok`, `RangeError` or `other: ...`. The input is built before the measured call
//! and dropped after it, both on the probe's thread.

use indexmap::IndexMap;
use markdoc::ast::{Function, Node};
use markdoc::markdown_it::Token;
use markdoc::next::index;
use markdoc::next::parser::ParserArgs;
use markdoc::next::renderers::html;
use markdoc::next::schema::value::{TagValue, Value, json_stringify};
use markdoc::next::schema::{Config, Partial, Variables};
use markdoc::next::tokenizer::{Tokenizer, TokenizerOptions};
use markdoc::types::{NodeType, Scalar};
use std::sync::Arc;
use std::time::Instant;

fn rep(s: &str, n: usize) -> String {
    s.repeat(n)
}

fn rep_sep(s: &str, n: usize, sep: &str) -> String {
    vec![s; n].join(sep)
}

fn tokenizer(max_nesting: bool) -> Tokenizer {
    let mut markdown_it = serde_json::Map::new();
    if max_nesting {
        markdown_it.insert("maxNesting".into(), 1e9.into());
    }
    Tokenizer::new(TokenizerOptions {
        allow_indentation: true,
        allow_comments: true,
        markdown_it,
        ..TokenizerOptions::default()
    })
}

/// The tag grammar's values, `n` levels around `1` (node_shapes.cjs's VALUES).
fn value(kind: &str, n: usize) -> String {
    let (open, close) = match kind {
        "array" => ("[", "]"),
        "hash" => ("{a: ", "}"),
        "call" => ("f(", ")"),
        "call2" => ("f(1, ", ")"),
        "array2" => ("[1, ", "]"),
        "hash2" => ("{a: 1, b: ", "}"),
        "string_key" => ("{\"a\": ", "}"),
        "named" => ("f(k=", ")"),
        "mix" => ("[{a: f(", ")}]"),
        other => panic!("value kind {other}"),
    };
    format!("{}1{}", rep(open, n), rep(close, n))
}

/// What a probe runs: built first, then called (and dropped) on the probe's thread.
type Call = Box<dyn FnOnce() -> Result<(), String>>;

fn tokenize_call(t: Tokenizer, doc: String) -> Call {
    Box::new(move || {
        let tokens = t
            .tokenize(&doc)
            .map_err(|e| format!("{}: {}", e.name, e.message))?;
        // an error token (a SyntaxError the tokenizer caught) is not a pass
        let mut stack: Vec<&Token> = tokens.iter().collect();
        while let Some(t) = stack.pop() {
            if t.kind == "error" {
                return Err("other: error token".into());
            }
            stack.extend(t.children.iter().flatten());
        }
        Ok(())
    })
}

fn parse_call(t: Tokenizer, doc: String, slots: bool) -> Call {
    let tokens = t.tokenize(&doc).expect("tokenize");
    Box::new(move || {
        let args = ParserArgs {
            slots,
            ..ParserArgs::default()
        };
        index::parse_tokens(&tokens, &doc, &args)
            .map(drop)
            .map_err(|e| index::thrown(&e).to_owned())
    })
}

fn text() -> Node {
    Node::new(
        NodeType::Text,
        [("content".to_string(), Scalar::String("a".into()))],
        vec![],
        None,
    )
}

/// `n` levels below the document, made by `make(child, i)` from the bottom up.
fn chain(n: usize, leaf: Node, make: impl Fn(Node, usize) -> Node) -> Node {
    let mut node = leaf;
    for i in (0..n).rev() {
        node = make(node, i);
    }
    Node::new(NodeType::Document, Vec::new(), vec![node], None)
}

fn tag(child: Node, name: &str, attrs: Vec<(String, Scalar)>) -> Node {
    Node::new(NodeType::Tag, attrs, vec![child], Some(name.to_string()))
}

fn tag_chain(n: usize) -> Node {
    chain(n, text(), |c, _| tag(c, "x", vec![]))
}

fn type_chain(n: usize, ty: NodeType, attrs: Vec<(&str, Scalar)>) -> Node {
    chain(n, text(), |c, _| {
        Node::new(
            ty.clone(),
            attrs.iter().map(|(k, v)| (k.to_string(), v.clone())),
            vec![c],
            None,
        )
    })
}

fn slot_chain(n: usize) -> Node {
    chain(n, text(), |c, _| {
        let mut t = Node::new(NodeType::Tag, Vec::new(), vec![], Some("x".into()));
        t.slots.insert("s".into(), c);
        t
    })
}

fn nest_scalar(n: usize, kind: &str) -> Scalar {
    let mut v = Scalar::Number(1.0);
    for i in 0..n {
        let k = match kind {
            "mix" => ["array", "object", "call"][i % 3],
            k => k,
        };
        v = match k {
            "array" => Scalar::Array(vec![v]),
            "object" => Scalar::Object(IndexMap::from([("k".to_string(), v)])),
            "call" => Scalar::Function(Function::new(
                "not".into(),
                IndexMap::from([("0".to_string(), v)]),
            )),
            other => panic!("kind {other}"),
        };
    }
    v
}

fn attr_doc(v: Scalar) -> Node {
    Node::new(
        NodeType::Document,
        Vec::new(),
        vec![Node::new(
            NodeType::Tag,
            [("a".to_string(), v)],
            vec![],
            Some("x".into()),
        )],
        None,
    )
}

fn config(json: serde_json::Value) -> Config {
    Config::from_json(&json).expect("config")
}

/// `{tags: {x: {attributes: {a: {}}, slots: {s: {}}}}}`, with `render: 'div'` if `render`.
fn x_config(render: bool) -> Config {
    let mut x = serde_json::json!({"attributes": {"a": {}}, "slots": {"s": {}}});
    if render {
        x["render"] = "div".into();
    }
    config(serde_json::json!({ "tags": { "x": x } }))
}

/// Partials p0 -> p1 -> ... -> p(n-1), each including the next; the last holds `inner`.
fn partial_chain(n: usize, inner: Node, mut cfg: Config) -> (Node, Config) {
    let partial = |file: String| {
        Node::new(
            NodeType::Tag,
            [("file".to_string(), Scalar::String(file))],
            vec![],
            Some("partial".into()),
        )
    };
    let mut partials = IndexMap::new();
    let mut inner = Some(inner);
    for i in 0..n {
        let body = if i + 1 < n {
            partial(format!("p{}", i + 1))
        } else {
            inner.take().expect("once")
        };
        partials.insert(
            format!("p{i}"),
            Partial::Node(Arc::new(Node::new(
                NodeType::Document,
                Vec::new(),
                vec![body],
                None,
            ))),
        );
    }
    cfg.partials = Some(partials);
    let doc = Node::new(
        NodeType::Document,
        Vec::new(),
        vec![partial("p0".into())],
        None,
    );
    (doc, cfg)
}

fn nest_render(n: usize, kind: &str) -> Value {
    let mut v = Value::string("x");
    for _ in 0..n {
        v = match kind {
            "array" => Value::array(vec![v]),
            "object" => Value::object(IndexMap::from([("a".to_string(), v)])),
            "tag" => TagValue::new("b", IndexMap::<String, Value>::new(), vec![v]).into(),
            "pseudo" => Value::object(IndexMap::from([
                ("$$mdtype".to_string(), Value::string("Tag")),
                ("name".to_string(), Value::string("b")),
                ("attributes".to_string(), Value::empty_object()),
                ("children".to_string(), Value::array(vec![v])),
            ])),
            other => panic!("kind {other}"),
        };
    }
    v
}

fn transform_call(doc: Node, cfg: Config) -> Call {
    Box::new(move || {
        index::transform(&doc, &cfg)
            .map(drop)
            .map_err(|e| index::thrown(&e).to_owned())
    })
}

fn variables(v: Value) -> Config {
    Config {
        variables: Some(Variables::Value(Value::object(IndexMap::from([(
            "v".to_string(),
            v,
        )])))),
        ..Config::default()
    }
}

/// A hand-made token nested `n` deep in `children`, a text at the bottom.
fn token_chain(n: usize) -> Vec<Token> {
    let mut t = Token::new("text", "", 0);
    t.content = "x".into();
    for _ in 0..n {
        let mut parent = Token::new("inline", "", 0);
        parent.children = Some(vec![t]);
        t = parent;
    }
    vec![t]
}

/// Takes a deep JSON value apart without recursing (serde_json's own drop recurses).
fn drop_json(v: serde_json::Value) {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            serde_json::Value::Array(items) => stack.extend(items),
            serde_json::Value::Object(map) => stack.extend(map.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

/// Takes deep tokens apart without recursing.
fn drop_tokens(tokens: Vec<Token>) {
    let mut stack = tokens;
    while let Some(mut t) = stack.pop() {
        if let Some(children) = t.children.take() {
            stack.extend(children);
        }
        if let Some(meta) = t.meta.take() {
            drop_json(meta);
        }
    }
}

const SHAPES: &[&str] = &[
    "md-blockquote",
    "md-list",
    "md-ordered",
    "md-quoted-list",
    "md-image",
    "md-link-image",
    "md-bracket",
    "md-link-bracket",
    "md-indented-list",
    "md-indented-ordered",
    "md-blockquote-lines",
    "md-blockquote-after-2000-paragraphs",
    "md-image-after-2000-paragraphs",
    "md-image-text",
    "md-link-image-text",
    "val-array",
    "val-array-inline",
    "val-hash",
    "val-hash-inline",
    "val-call",
    "val-call-inline",
    "val-call2",
    "val-call2-inline",
    "val-array2",
    "val-array2-inline",
    "val-hash2",
    "val-hash2-inline",
    "val-string_key",
    "val-string_key-inline",
    "val-named",
    "val-named-inline",
    "val-mix",
    "val-mix-inline",
    "val-array-primary",
    "val-array-after-2000-tags",
    "val-array-after-2000-items",
    "val-array-annotation",
    "val-array-in-40-quotes",
    "val-call-in-40-lists",
    "val-hash-in-40-images",
    "md-quotes-around-200-arrays",
    "ast-para",
    "ast-annot",
    "ast-heading",
    "ast-item",
    "ast-loose",
    "ast-quote",
    "ast-row",
    "ast-ltable",
    "ast-slot",
    "ast-blocktag",
    "ast-closed",
    "ast-tagslot",
    "ast-blockslot",
    "tokens-children",
    "tokens-meta",
    "validate-tag",
    "validate-slot",
    "resolve-tag",
    "resolve-slot",
    "transform-tag",
    "transform-tag-render",
    "transform-blockquote",
    "transform-paragraph",
    "transform-inline",
    "transform-strong",
    "transform-link",
    "transform-heading",
    "transform-fence",
    "transform-list",
    "transform-if",
    "transform-slot",
    "transform-partials",
    "transform-value-array",
    "transform-value-object",
    "transform-value-call",
    "transform-value-mix",
    "transform-tags-around-500-arrays",
    "transform-300-partials-around-tags",
    "html-array",
    "html-tag",
    "html-pseudo",
    "html-attribute-array",
    "html-variable-array",
    "string-array",
    "json-array",
    "json-array-gap",
    "json-object",
    "json-tag",
    "debug-array",
    "debug-array-in-40-tags",
];

fn build(shape: &str, n: usize) -> Call {
    let deep = || tokenizer(true);
    let def = || tokenizer(false);
    let lines = |f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<Vec<_>>().join("\n") + "\n";
    if let Some(rest) = shape.strip_prefix("val-") {
        let (kind, inline) = match rest.strip_suffix("-inline") {
            Some(kind) => (kind, true),
            None => (rest, false),
        };
        let known = [
            "array",
            "hash",
            "call",
            "call2",
            "array2",
            "hash2",
            "string_key",
            "named",
            "mix",
        ];
        if known.contains(&kind) {
            let doc = if inline {
                format!("p {{% x a={} /%}} q\n", value(kind, n))
            } else {
                format!("{{% x a={} /%}}\n", value(kind, n))
            };
            return tokenize_call(def(), doc);
        }
    }
    match shape {
        "md-blockquote" => tokenize_call(deep(), rep("> ", n) + "x\n"),
        "md-list" => tokenize_call(deep(), rep("- ", n) + "x\n"),
        "md-ordered" => tokenize_call(deep(), rep("1. ", n) + "x\n"),
        "md-quoted-list" => tokenize_call(deep(), rep("> - ", n) + "x\n"),
        "md-image" => tokenize_call(deep(), rep("![", n) + "x" + &rep("](u)", n) + "\n"),
        "md-link-image" => tokenize_call(deep(), rep("[![", n) + "x" + &rep("](u)](v)", n) + "\n"),
        "md-bracket" => tokenize_call(deep(), rep("[", n) + "x\n"),
        "md-link-bracket" => tokenize_call(deep(), "[a](u) ".to_string() + &rep("[", n) + "x\n"),
        "md-indented-list" => tokenize_call(deep(), lines(&|i| rep("  ", i) + "- a")),
        "md-indented-ordered" => tokenize_call(deep(), lines(&|i| rep("   ", i) + "1. a")),
        "md-blockquote-lines" => tokenize_call(deep(), lines(&|i| rep("> ", i + 1) + "a")),
        "md-blockquote-after-2000-paragraphs" => tokenize_call(
            deep(),
            rep_sep("a *b* [c](d)", 2000, "\n\n") + "\n\n" + &rep("> ", n) + "x\n",
        ),
        "md-image-after-2000-paragraphs" => tokenize_call(
            deep(),
            rep_sep("a *b* ![c](d)", 2000, "\n\n")
                + "\n\n"
                + &rep("![", n)
                + "x"
                + &rep("](u)", n)
                + "\n",
        ),
        "md-image-text" => tokenize_call(deep(), rep("![a ", n) + "x" + &rep("](u) b", n) + "\n"),
        "md-link-image-text" => tokenize_call(
            deep(),
            rep("[a ![b ", n) + "x" + &rep("](u) c](v) d", n) + "\n",
        ),
        "val-array-primary" => tokenize_call(def(), format!("{{% x {} /%}}\n", value("array", n))),
        "val-array-after-2000-tags" => tokenize_call(
            def(),
            rep_sep("{% x a=[[1, 2], {b: f(1, $c)}] /%}", 2000, "\n")
                + &format!("\n{{% x a={} /%}}\n", value("array", n)),
        ),
        "val-array-after-2000-items" => tokenize_call(
            def(),
            format!(
                "{{% x a=[{}, {}] /%}}\n",
                rep_sep("[1], {b: f(1)}", 2000, ", "),
                value("array", n)
            ),
        ),
        "val-array-annotation" => {
            tokenize_call(def(), format!("# h {{% .c a={} %}}\n", value("array", n)))
        }
        "val-array-in-40-quotes" => tokenize_call(
            deep(),
            rep("> ", 40) + &format!("{{% x a={} /%}}\n", value("array", n)),
        ),
        "val-call-in-40-lists" => tokenize_call(
            deep(),
            rep("- ", 40) + &format!("{{% x a={} /%}}\n", value("call", n)),
        ),
        "val-hash-in-40-images" => tokenize_call(
            deep(),
            rep("![", 40) + &format!("{{% x a={} /%}}", value("hash", n)) + &rep("](u)", 40) + "\n",
        ),
        "md-quotes-around-200-arrays" => tokenize_call(
            deep(),
            rep("> ", n) + &format!("{{% x a={} /%}}\n", value("array", 200)),
        ),
        "ast-para" => parse_call(def(), rep_sep("p {% x %} q", n, "\n\n"), false),
        "ast-annot" => parse_call(def(), rep_sep("p {% x .a #b c=1 %} q", n, "\n\n"), false),
        "ast-heading" => parse_call(def(), rep_sep("# h {% x %} q", n, "\n"), false),
        "ast-item" => parse_call(def(), rep_sep("* a {% x %} b", n, "\n"), false),
        "ast-loose" => parse_call(def(), rep_sep("* a {% x %} b", n, "\n\n"), false),
        "ast-quote" => parse_call(def(), rep_sep("> a {% x %} b", n, "\n\n"), false),
        "ast-row" => parse_call(
            def(),
            "| h |\n|---|\n".to_string() + &rep_sep("| a {% x %} b |", n, "\n"),
            false,
        ),
        "ast-ltable" => parse_call(
            def(),
            "{% table %}\n".to_string()
                + &rep_sep("* a {% x %} b", n, "\n---\n")
                + "\n{% /table %}",
            false,
        ),
        "ast-slot" => parse_call(def(), rep_sep("p {% slot \"s\" %} q", n, "\n\n"), true),
        "ast-blocktag" => parse_call(deep(), rep_sep("{% x %}", n, "\n"), false),
        "ast-closed" => parse_call(
            deep(),
            rep_sep("{% x %}", n, "\n") + "\n" + &rep_sep("{% /x %}", n, "\n"),
            false,
        ),
        "ast-tagslot" => parse_call(deep(), rep_sep("{% x %}\n{% slot \"s\" %}", n, "\n"), true),
        "ast-blockslot" => parse_call(deep(), rep_sep("{% slot \"s\" %}", n, "\n"), true),
        "tokens-children" => {
            let tokens = token_chain(n);
            Box::new(move || {
                let r = index::parse_tokens(&tokens, "", &ParserArgs::default())
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned());
                drop_tokens(tokens);
                r
            })
        }
        "tokens-meta" => {
            let mut v = serde_json::json!(1);
            for _ in 0..n {
                v = serde_json::Value::Array(vec![v]);
            }
            let mut open = Token::new("tag_open", "", 1);
            open.meta = Some(serde_json::json!({"tag": "x", "attributes": []}));
            if let Some(serde_json::Value::Object(meta)) = open.meta.as_mut()
                && let Some(serde_json::Value::Array(attributes)) = meta.get_mut("attributes")
            {
                attributes.push(serde_json::json!({"type": "attribute", "name": "a"}));
                if let Some(serde_json::Value::Object(a)) = attributes.last_mut() {
                    a.insert("value".into(), v);
                }
            }
            let mut close = Token::new("tag_close", "", -1);
            close.meta = Some(serde_json::json!({"tag": "x"}));
            let tokens = vec![open, close];
            Box::new(move || {
                let r = index::parse_tokens(&tokens, "", &ParserArgs::default())
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned());
                drop_tokens(tokens);
                r
            })
        }
        "validate-tag" | "validate-slot" => {
            let doc = if shape == "validate-tag" {
                tag_chain(n)
            } else {
                slot_chain(n)
            };
            let cfg = x_config(false);
            Box::new(move || {
                index::validate(&doc, &cfg)
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned())
            })
        }
        "resolve-tag" | "resolve-slot" => {
            let doc = if shape == "resolve-tag" {
                tag_chain(n)
            } else {
                slot_chain(n)
            };
            Box::new(move || {
                index::resolve(&doc, &Config::default())
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned())
            })
        }
        "transform-tag" => transform_call(tag_chain(n), x_config(false)),
        "transform-tag-render" => transform_call(tag_chain(n), x_config(true)),
        "transform-blockquote" => transform_call(
            type_chain(n, NodeType::Blockquote, vec![]),
            Config::default(),
        ),
        "transform-paragraph" => transform_call(
            type_chain(n, NodeType::Paragraph, vec![]),
            Config::default(),
        ),
        "transform-inline" => {
            transform_call(type_chain(n, NodeType::Inline, vec![]), Config::default())
        }
        "transform-strong" => transform_call(
            type_chain(
                n,
                NodeType::Strong,
                vec![("marker", Scalar::String("*".into()))],
            ),
            Config::default(),
        ),
        "transform-link" => transform_call(
            type_chain(
                n,
                NodeType::Link,
                vec![("href", Scalar::String("u".into()))],
            ),
            Config::default(),
        ),
        "transform-heading" => transform_call(
            type_chain(n, NodeType::Heading, vec![("level", Scalar::Number(1.0))]),
            Config::default(),
        ),
        "transform-fence" => transform_call(
            type_chain(
                n,
                NodeType::Fence,
                vec![
                    ("content", Scalar::String("c".into())),
                    ("process", Scalar::Boolean(true)),
                ],
            ),
            Config::default(),
        ),
        "transform-list" => transform_call(
            chain(n, text(), |c, i| {
                if i % 2 == 1 {
                    Node::new(NodeType::Item, Vec::new(), vec![c], None)
                } else {
                    Node::new(
                        NodeType::List,
                        [("ordered".to_string(), Scalar::Boolean(false))],
                        vec![c],
                        None,
                    )
                }
            }),
            Config::default(),
        ),
        "transform-if" => transform_call(
            chain(n, text(), |c, _| {
                tag(
                    c,
                    "if",
                    vec![("primary".to_string(), Scalar::Boolean(true))],
                )
            }),
            Config::default(),
        ),
        "transform-slot" => transform_call(slot_chain(n), x_config(true)),
        "transform-partials" => {
            let (doc, cfg) = partial_chain(n, text(), Config::default());
            transform_call(doc, cfg)
        }
        "transform-value-array" => {
            transform_call(attr_doc(nest_scalar(n, "array")), x_config(false))
        }
        "transform-value-object" => {
            transform_call(attr_doc(nest_scalar(n, "object")), x_config(false))
        }
        "transform-value-call" => transform_call(attr_doc(nest_scalar(n, "call")), x_config(false)),
        "transform-value-mix" => transform_call(attr_doc(nest_scalar(n, "mix")), x_config(false)),
        "transform-tags-around-500-arrays" => {
            let leaf = Node::new(
                NodeType::Tag,
                [("a".to_string(), nest_scalar(500, "array"))],
                vec![],
                Some("x".into()),
            );
            transform_call(chain(n, leaf, |c, _| tag(c, "x", vec![])), x_config(false))
        }
        "transform-300-partials-around-tags" => {
            let mut inner = tag_chain(n);
            let first = inner.children.remove(0);
            let (doc, cfg) = partial_chain(300, first, x_config(false));
            transform_call(doc, cfg)
        }
        "html-array" | "html-tag" | "html-pseudo" => {
            let v = nest_render(n, &shape[5..]);
            Box::new(move || {
                html::render(&v)
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned())
            })
        }
        "html-attribute-array" => {
            let v: Value = TagValue::new(
                "b",
                IndexMap::from([("a".to_string(), nest_render(n, "array"))]),
                vec![],
            )
            .into();
            Box::new(move || {
                html::render(&v)
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned())
            })
        }
        "html-variable-array" => {
            let doc = index::parse("{% $v %}", &ParserArgs::default()).expect("parse");
            let cfg = variables(nest_render(n, "array"));
            Box::new(move || {
                let tree =
                    index::transform(&doc, &cfg).map_err(|e| index::thrown(&e).to_owned())?;
                html::render(&tree)
                    .map(drop)
                    .map_err(|e| index::thrown(&e).to_owned())
            })
        }
        "string-array" => {
            let v = nest_render(n, "array");
            Box::new(move || v.to_js_string().map(drop))
        }
        "json-array" | "json-object" | "json-tag" => {
            let v = nest_render(n, &shape[5..]);
            Box::new(move || json_stringify(&v, "").map(drop))
        }
        "json-array-gap" => {
            let v = nest_render(n, "array");
            Box::new(move || json_stringify(&v, "  ").map(drop))
        }
        "debug-array-in-40-tags" => {
            let doc = index::parse(
                &(rep("{% x %}\n", 40) + "{% debug($v) %}"),
                &ParserArgs::default(),
            )
            .expect("parse");
            let cfg = variables(nest_render(n, "array"));
            transform_call(doc, cfg)
        }
        "debug-array" => {
            let doc = index::parse("{% debug($v) %}", &ParserArgs::default()).expect("parse");
            let cfg = variables(nest_render(n, "array"));
            transform_call(doc, cfg)
        }
        other => panic!("unknown shape {other}"),
    }
}

/// The result as node_shapes.cjs writes it.
fn outcome(r: Result<(), String>) -> String {
    match r {
        Ok(()) => "ok".into(),
        Err(e) if e == "RangeError: Maximum call stack size exceeded" => "RangeError".into(),
        Err(e) if e.starts_with("other: ") => e,
        Err(e) => format!("other: {}", e.chars().take(120).collect::<String>()),
    }
}

fn run(shape: &str, n: usize, stack_kib: usize) -> (String, f64) {
    let shape = shape.to_owned();
    std::thread::Builder::new()
        .stack_size(stack_kib * 1024)
        .spawn(move || {
            let call = build(&shape, n);
            let start = Instant::now();
            let r = call();
            let ms = start.elapsed().as_secs_f64() * 1e3;
            (outcome(r), ms)
        })
        .expect("spawn")
        .join()
        .expect("the probe panicked")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let num = |i: usize, default: usize| arg(i).map_or(default, |s| s.parse().expect("a number"));
    match arg(0) {
        Some("list") => {
            for s in SHAPES {
                println!("{s}");
            }
        }
        Some("probe") => {
            let shape = arg(1).expect("SHAPE");
            let n = num(2, 1);
            let (r, _) = run(shape, n, num(3, 2048));
            println!("{}", serde_json::json!({"shape": shape, "n": n, "r": r}));
        }
        Some("time") => {
            let shape = arg(1).expect("SHAPE");
            let n = num(2, 1);
            let (r, ms) = run(shape, n, num(3, 2048));
            println!(
                "{}",
                serde_json::json!({"shape": shape, "n": n, "r": r, "ms": ms})
            );
        }
        Some("search") => {
            let shape = arg(1).expect("SHAPE");
            let stack = num(3, 2048);
            let ok = |n: usize| run(shape, n, stack).0 == "ok";
            let (mut lo, mut hi) = (0usize, num(2, 64));
            while ok(hi) {
                lo = hi;
                hi *= 2;
                if hi > 1 << 21 {
                    println!(
                        "{}",
                        serde_json::json!({"shape": shape, "port": format!(">{lo}")})
                    );
                    return;
                }
            }
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if ok(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let next = run(shape, hi, stack).0;
            println!(
                "{}",
                serde_json::json!({"shape": shape, "port": lo, "next": next})
            );
        }
        _ => {
            eprintln!(
                "usage: next_depth list | probe SHAPE N [STACK_KIB] \
                 | search SHAPE [HI] [STACK_KIB] | time SHAPE N [STACK_KIB]"
            );
            std::process::exit(2);
        }
    }
}
