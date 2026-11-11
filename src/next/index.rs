//! Port of upstream Markdoc v0.5.11's `index.ts`.
//!
//! `parse`, `resolve`, `transform`, `validate`, `createElement`, `mergeConfig`, and the default
//! export's instance, [`Markdoc`] (its statics are this crate's modules: see the table in
//! `CONFORMANCE.md`, "Upstream's API").
//!
//! [`parse`] is `Markdoc.parse(content, args)`: the text through the default tokenizer
//! (upstream's `new Tokenizer()`, every switch off) and the parser; [`parse_with`] takes a
//! tokenizer of the caller's (the conformance runner's has `allowIndentation` and
//! `allowComments` on), and [`parse_tokens`] is the `Token[]` form. Two switches in
//! [`ParserArgs`] are this crate's own, both off by default, as upstream has neither:
//!
//! - `parse_frontmatter`: the document's `frontmatter` attribute is the YAML parsed into an
//!   object (and YAML that is no mapping a `frontmatter-invalid` error); off, it is
//!   upstream's raw text.
//! - `strip_bom`: a byte-order mark at the start is not tokenized (upstream keeps it, so a
//!   heading on line 1 is then a paragraph). Positions still point into the text given.
//!
//! An `Err` from a function here is where upstream throws, and says what: its message is
//! JavaScript's `String(error)`, the error's name and message (`TypeError: Cannot create
//! property 'b' on string 'a'`, `RangeError: Maximum call stack size exceeded`), as
//! [`thrown`] gives it. The one other kind is a refusal of an input this crate does not model
//! (`not modeled: ...`; see each module's "Divergences from upstream").

use crate::ast::Node;
use crate::markdown_it::Token;
use crate::next::functions::default_functions;
use crate::next::parser::{self, ParserArgs};
use crate::next::schema::value::Value;
use crate::next::schema::{Config, default_nodes};
use crate::next::tags::default_tags;
use crate::next::tokenizer::Tokenizer;
use crate::next::transformer::{self, ResolvedNode, flat_push, resolve_node};
use crate::next::validator::{ValidateError, validate_tree};
use crate::types::{MarkdocError, Result, js_insert};
use indexmap::IndexMap;
use std::sync::OnceLock;

/// `new Tokenizer()`, made once: the tokenizer [`parse`] uses (upstream's module-level
/// `tokenizer`).
pub fn default_tokenizer() -> &'static Tokenizer {
    static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();
    TOKENIZER.get_or_init(Tokenizer::default)
}

/// `parse(content, args)`: the AST of `content`, tokenized by [`default_tokenizer`].
pub fn parse(content: &str, args: &ParserArgs) -> Result<Node> {
    parse_with(default_tokenizer(), content, args)
}

/// [`parse`] with `tokenizer`: `parse(tokenizer.tokenize(content), args)`. With
/// `args.strip_bom`, a byte-order mark that starts `content` is not tokenized; the nodes'
/// positions are still offsets into `content`.
pub fn parse_with(tokenizer: &Tokenizer, content: &str, args: &ParserArgs) -> Result<Node> {
    let text = if args.strip_bom {
        content.strip_prefix('\u{FEFF}').unwrap_or(content)
    } else {
        content
    };
    let tokens = tokenizer
        .tokenize(text)
        .map_err(|e| MarkdocError::ParseError(e.to_string()))?;
    parse_tokens(&tokens, content, args)
}

/// `parse(tokens, args)`: the AST of a token stream, which a tokenizer made from `source`
/// (read for positions, and for the frontmatter with `args.parse_frontmatter`; the tokens
/// are taken as they are, `args.strip_bom` aside).
pub fn parse_tokens(tokens: &[Token], source: &str, args: &ParserArgs) -> Result<Node> {
    parser::parse(tokens, source, args)
}

/// What an `Err` of `next` says upstream throws: JavaScript's `String(error)`, `Name:
/// message` (see the module's documentation).
pub fn thrown(error: &MarkdocError) -> &str {
    match error {
        MarkdocError::ParseError(message)
        | MarkdocError::TransformError(message)
        | MarkdocError::ValidationError(message) => message,
    }
}

/// `{...defaults, ...overrides}`.
fn spread<T: Clone>(
    defaults: &IndexMap<String, T>,
    overrides: &IndexMap<String, T>,
) -> IndexMap<String, T> {
    let mut out = defaults.clone();
    for (k, v) in overrides {
        js_insert(&mut out, k.clone(), v.clone());
    }
    out
}

/// `mergeConfig(config)`: the config with the default tags, nodes and functions under its own.
pub fn merge_config(config: &Config) -> Config {
    Config {
        tags: spread(default_tags(), &config.tags),
        nodes: spread(default_nodes(), &config.nodes),
        functions: spread(default_functions(), &config.functions),
        ..config.clone()
    }
}

/// `resolve(content, config)`: the document with its variables and function calls resolved.
pub fn resolve<'a>(content: &'a Node, config: &Config) -> Result<ResolvedNode<'a>> {
    resolve_node(content, config)
}

/// `resolve(content, config)` of a list of nodes: each one resolved (`content.flatMap((child)
/// => child.resolve(config))`, and a node resolves to one node).
pub fn resolve_all<'a>(content: &'a [Node], config: &Config) -> Result<Vec<ResolvedNode<'a>>> {
    content.iter().map(|node| resolve(node, config)).collect()
}

/// `transform(node, config)`: the render tree of a document (an `article` tag), or of any
/// node, as upstream returns it: JavaScript values, which [`crate::next::renderers::html`]
/// renders. Its JSON is [`crate::next::schema::value::json_stringify`] (what `JSON.stringify`
/// writes) or, as a [`crate::types::RenderableTreeNode`], [`Value::to_render`]; JSON leaves out
/// what it cannot hold, which rendering can see (an object member that is `undefined`), so
/// render the tree, not its JSON.
pub fn transform(node: &Node, config: &Config) -> Result<Value> {
    let config = merge_config(config);
    let content = resolve(node, &config)?;
    transformer::node(&content, &config)
}

/// `transform(nodes, config)` of a list of nodes: each one's render tree, an array a node
/// renders to spliced in.
pub fn transform_all(nodes: &[Node], config: &Config) -> Result<Vec<Value>> {
    let config = merge_config(config);
    let content = nodes
        .iter()
        .map(|node| resolve(node, &config))
        .collect::<Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for node in &content {
        flat_push(&mut out, transformer::node(node, &config)?);
    }
    Ok(out)
}

/// `validate(node, config)`: every node's errors, in document order.
pub fn validate(node: &Node, config: &Config) -> Result<Vec<ValidateError>> {
    let config = merge_config(config);
    validate_tree(node, &config)
}

/// `createElement(name, attributes, ...children)`: `{name, attributes, children}` (a plain
/// object, not a tag). `name` is any value (a string, or an object such as a React
/// component).
pub fn create_element(
    name: impl Into<Value>,
    attributes: IndexMap<String, Value>,
    children: Vec<Value>,
) -> Value {
    let mut element = IndexMap::new();
    element.insert("name".to_string(), name.into());
    element.insert("attributes".to_string(), Value::object(attributes));
    element.insert("children".to_string(), Value::array(children));
    Value::object(element)
}

/// `new Markdoc(config)`, upstream's default export: [`parse`] and the config-bound
/// `resolve`, `transform` and `validate` (`Markdoc.transform(content)` is
/// `transform(content, this.config)`). Its statics are this module's functions and the
/// crate's modules (`Markdoc.nodes` is [`default_nodes`], `Markdoc.format` is
/// [`crate::next::formatter::format`], ...).
#[derive(Clone, Default)]
pub struct Markdoc {
    pub config: Config,
}

impl Markdoc {
    /// `new Markdoc(config)`.
    pub fn new(config: Config) -> Self {
        Markdoc { config }
    }

    /// `markdoc.parse(content, args)`: [`parse`] (the instance's is upstream's `parse`, which
    /// takes no config).
    pub fn parse(&self, content: &str, args: &ParserArgs) -> Result<Node> {
        parse(content, args)
    }

    /// `markdoc.resolve(content)`: [`resolve`] with the instance's config.
    pub fn resolve<'a>(&self, content: &'a Node) -> Result<ResolvedNode<'a>> {
        resolve(content, &self.config)
    }

    /// `markdoc.transform(content)`: [`transform`] with the instance's config.
    pub fn transform(&self, content: &Node) -> Result<Value> {
        transform(content, &self.config)
    }

    /// `markdoc.validate(content)`: [`validate`] with the instance's config.
    pub fn validate(&self, content: &Node) -> Result<Vec<ValidateError>> {
        validate(content, &self.config)
    }
}

#[cfg(test)]
mod entry_points {
    //! `parse` and its switches, what an `Err` says, and the token meta bridge, each against
    //! what node prints for the same input (upstream's dist, `Markdoc.parse(src)` and the
    //! rest; see the comments).
    use super::*;
    use crate::next::renderers::html;
    use crate::next::tokenizer::TokenizerOptions;
    use crate::types::{NodeType, Scalar};

    fn args() -> ParserArgs {
        ParserArgs::default()
    }

    #[test]
    fn parse_uses_upstreams_default_tokenizer() {
        // node: Markdoc.parse('# Title\n\n    # Indented\n') is a heading and a paragraph
        // (`new Tokenizer()` keeps markdown-it's four-column checks); a tokenizer with
        // allowIndentation makes the second a heading too.
        let src = "# Title\n\n    # Indented\n";
        let types = |doc: &Node| {
            doc.children
                .iter()
                .map(|c| c.node_type.clone())
                .collect::<Vec<_>>()
        };
        let doc = parse(src, &args()).unwrap();
        assert_eq!(types(&doc), [NodeType::Heading, NodeType::Paragraph]);
        let indented = Tokenizer::new(TokenizerOptions {
            allow_indentation: true,
            ..TokenizerOptions::default()
        });
        let doc = parse_with(&indented, src, &args()).unwrap();
        assert_eq!(types(&doc), [NodeType::Heading, NodeType::Heading]);
        // The `Token[]` form takes what a tokenizer made.
        let tokens = indented.tokenize(src).unwrap();
        let doc = parse_tokens(&tokens, src, &args()).unwrap();
        assert_eq!(types(&doc), [NodeType::Heading, NodeType::Heading]);
    }

    #[test]
    fn frontmatter_is_upstreams_text_unless_asked_to_parse_it() {
        // node: Markdoc.parse('---\nFoo\n---\nBar\n').attributes is {frontmatter: 'Foo'}.
        let doc = parse("---\nFoo\n---\nBar\n", &args()).unwrap();
        assert_eq!(
            doc.attributes.get("frontmatter"),
            Some(&Scalar::String("Foo".into()))
        );
        assert!(doc.errors.is_empty());
        let yaml = ParserArgs {
            parse_frontmatter: true,
            ..args()
        };
        let doc = parse("---\ntitle: T\nn: 1\n---\nBar\n", &yaml).unwrap();
        let Some(Scalar::Object(fm)) = doc.attributes.get("frontmatter") else {
            panic!("an object")
        };
        assert_eq!(fm.get("title"), Some(&Scalar::String("T".into())));
        assert_eq!(fm.get("n"), Some(&Scalar::Number(1.0)));
        let doc = parse("---\nFoo\n---\nBar\n", &yaml).unwrap();
        assert_eq!(
            doc.attributes.get("frontmatter"),
            Some(&Scalar::String("Foo".into()))
        );
        assert_eq!(doc.errors[0].id, "frontmatter-invalid");
    }

    #[test]
    fn a_byte_order_mark_is_kept_unless_asked_to_strip_it() {
        // node: Markdoc.parse('\ufeff# Title\n').children[0].type is 'paragraph'.
        let src = "\u{FEFF}# Title\n";
        let doc = parse(src, &args()).unwrap();
        assert_eq!(doc.children[0].node_type, NodeType::Paragraph);
        let strip = ParserArgs {
            strip_bom: true,
            spans: true,
            ..args()
        };
        let doc = parse(src, &strip).unwrap();
        let heading = &doc.children[0];
        assert_eq!(heading.node_type, NodeType::Heading);
        // This crate's spans are offsets into the text given, the mark included.
        let span = heading.span.as_ref().unwrap();
        assert_eq!(span.start.offset, 3);
        assert_eq!(&src[span.start.offset..span.end.offset], "# Title\n");
    }

    #[test]
    fn an_err_says_what_upstream_throws() {
        // node: Markdoc.parse('{% x class="a" .b /%}') throws
        // "TypeError: Cannot create property 'b' on string 'a'" (the parser) and
        // Markdoc.parse('[{% /a %}](u)') "TypeError: Cannot read properties of undefined
        // (reading 'length')" (the tokenizer).
        let error = parse("{% x class=\"a\" .b /%}", &args()).unwrap_err();
        assert_eq!(
            thrown(&error),
            "TypeError: Cannot create property 'b' on string 'a'"
        );
        let error = parse("[{% /a %}](u)", &args()).unwrap_err();
        assert_eq!(
            thrown(&error),
            "TypeError: Cannot read properties of undefined (reading 'length')"
        );
    }

    /// A number literal past the largest double: `parseFloat` makes it Infinity.
    fn overlong() -> String {
        format!("1{}", "0".repeat(400))
    }

    fn config(json: serde_json::Value) -> Config {
        Config::from_json(&json).unwrap()
    }

    #[test]
    fn a_link_made_of_its_closing_token_has_an_undefined_href() {
        // node: the link close inside the open tag `a` is a link node of its own with
        // `{href: undefined}`. validate() sees the key (its type is wrong) and finds it
        // missing; the render tree leaves it out (`<a></a>`).
        let doc = parse("[{% a %}](u)\n", &args()).unwrap();
        let ids: Vec<String> = validate(&doc, &Config::default())
            .unwrap()
            .into_iter()
            .filter(|e| e.node_type == crate::types::NodeType::Link)
            .map(|e| format!("{}: {}", e.error.id, e.error.message))
            .collect();
        assert_eq!(
            ids,
            [
                "missing-closing: Node 'link' is missing closing",
                "child-invalid: Can't nest 'paragraph' in 'link'",
                "missing-opening: Node 'link' is missing opening",
                "attribute-type-invalid: Attribute 'href' must be type of 'String'",
                "attribute-missing-required: Missing required attribute: 'href'",
            ]
        );
        let tree = transform(&doc, &Config::default()).unwrap();
        assert_eq!(
            html::render(&tree).unwrap(),
            r#"<article><p><a href="u"><a></a><p></p></a></p></article>"#
        );
    }

    #[test]
    fn an_overlong_number_is_infinity_at_every_stage() {
        // node, with tags {x: {render: 'x', attributes: {a: {type: Number}}}}:
        // `{% x a=1e400 /%}` (written out) validates, transforms to `a: Infinity` (JSON
        // null) and renders `<x a="Infinity"></x>`; `-` gives -Infinity.
        let big = overlong();
        let src = format!("{{% x a={big} /%}}\n{{% x a=-{big} /%}}\n");
        let doc = parse(&src, &args()).unwrap();
        assert_eq!(
            doc.children[0].attributes.get("a"),
            Some(&Scalar::Number(f64::INFINITY))
        );
        // The token carries the grammar's own meta beside its JSON, which cannot hold it.
        let tokens = default_tokenizer().tokenize(&src).unwrap();
        assert_eq!(
            tokens[0].meta.as_ref().unwrap()["attributes"][0]["value"],
            serde_json::Value::Null
        );
        assert!(tokens[0].grammar_meta.is_some());
        let config = config(serde_json::json!({
            "tags": {"x": {"render": "x", "attributes": {"a": {"type": "Number"}}}}
        }));
        assert!(validate(&doc, &config).unwrap().is_empty());
        let tree = transform(&doc, &config).unwrap();
        assert_eq!(
            crate::next::compare::to_json_string(&tree.to_render().unwrap().to_json()),
            r#"{"$$mdtype":"Tag","name":"article","attributes":{},"children":[{"$$mdtype":"Tag","name":"x","attributes":{"a":null},"children":[]},{"$$mdtype":"Tag","name":"x","attributes":{"a":null},"children":[]}]}"#
        );
        assert_eq!(
            html::render(&tree).unwrap(),
            r#"<article><x a="Infinity"></x><x a="-Infinity"></x></article>"#
        );
    }

    #[test]
    fn an_overlong_number_in_a_path_a_fence_and_a_condition() {
        // node: `{% $v[1e400] %}` reads v.Infinity; a fence's info tag and the tags in its
        // content keep Infinity; `{% if 1e400 %}` is true.
        let big = overlong();
        let config = config(serde_json::json!({
            "variables": {"v": {"Infinity": "inf"}},
            "tags": {"y": {"render": "y", "attributes": {"a": {"type": "Number"}}}}
        }));
        let render = |src: String| {
            let doc = parse(&src, &args()).unwrap();
            html::render(&transform(&doc, &config).unwrap()).unwrap()
        };
        assert_eq!(
            render(format!("{{% $v[{big}] %}}")),
            "<article><p>inf</p></article>"
        );
        assert_eq!(
            render(format!(
                "```js {{% a={big} %}}\nc {{% y a=-{big} /%}}\n```\n"
            )),
            "<article><pre data-language=\"js\">c <y a=\"-Infinity\"></y>\n</pre></article>"
        );
        assert_eq!(
            render(format!("{{% if -{big} %}}\nyes\n{{% /if %}}\n")),
            "<article><p>yes</p></article>"
        );
    }
}

#[cfg(test)]
mod tests {
    //! Upstream's own outputs for the 105 conformance cases (`tests/next/golden/conformance.jsonl`,
    //! written by `tests/next/compare.py --golden`): its AST and partials' ASTs through this
    //! port's `validate`, `transform` and HTML renderer, and its render tree through the HTML
    //! renderer, each against upstream's output. `compare.py --stage transform|errors|html
    //! --from-oracle --conformance` makes the same comparison against node.
    use super::*;
    use crate::next::compare::{
        ValidateRecord, errors_json, first_difference, node_from_json, render_from_json,
        render_json,
    };
    use crate::next::renderers::html;
    use crate::next::schema::Partial;
    use std::sync::Arc;

    #[test]
    fn upstream_conformance_outputs() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/next/golden/conformance.jsonl"
        );
        let text = std::fs::read_to_string(path).expect("the golden file");
        let mut failures = Vec::new();
        let mut checked = [0; 4];
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let case: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
            let id = case["id"].as_str().unwrap_or("?").to_string();
            let no_config = serde_json::json!({});
            let config_json = case.get("config").unwrap_or(&no_config);
            let ast = node_from_json(&case["ast"]).expect("an upstream AST");
            let mut diff = |what: &str, expected: &serde_json::Value, got: serde_json::Value| {
                if let Some((at, a, b)) = first_difference(expected, &got) {
                    failures.push(format!("{id}: {what}{at}: upstream {a} | port {b}"));
                }
            };

            if let Some(errors) = case.get("errors") {
                let config = Config::from_json(config_json).expect("a config");
                let records: Vec<ValidateRecord> = validate(&ast, &config)
                    .expect("validate")
                    .into_iter()
                    .map(|e| ValidateRecord {
                        node_type: e.node_type.to_string(),
                        lines: e.lines,
                        location: e.location.map(|l| l.to_json()),
                        error: e.error,
                    })
                    .collect();
                diff("errors", errors, errors_json(&records));
                checked[0] += 1;
            }

            if let Some(tree) = case.get("transformed") {
                let mut config = Config::from_json(config_json).expect("a config");
                let mut partials = IndexMap::new();
                for (name, ast) in case["partials"].as_object().into_iter().flatten() {
                    let node = node_from_json(ast).expect("a partial's AST");
                    partials.insert(name.clone(), Partial::Node(Arc::new(node)));
                }
                config.partials = Some(partials);
                let transformed = transform(&ast, &config).expect("transform");
                diff(
                    "transformed",
                    tree,
                    render_json(&transformed.to_render().expect("to_render")),
                );
                checked[1] += 1;
                if let Some(expected) = case.get("html").and_then(|h| h.as_str()) {
                    let upstream_tree = render_from_json(tree).expect("a render tree");
                    diff(
                        "html",
                        &expected.into(),
                        html::render_tree(&upstream_tree).expect("renders").into(),
                    );
                    diff(
                        "html of the port's tree",
                        &expected.into(),
                        html::render(&transformed).expect("renders").into(),
                    );
                    checked[2] += 1;
                    checked[3] += 1;
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} difference(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
        assert_eq!(
            checked,
            [105, 105, 105, 105],
            "errors, trees, html, html of the port's tree"
        );
    }
}
