//! Upstream Markdoc v0.5.11, ported beside the old pipeline.
//!
//! Every file ports one of upstream's, line for line, onto the markdown-it 15.0.1 port in
//! [`crate::markdown_it`] and the tag grammar's port in `crate::grammar`, so that from the
//! same source and the same configuration every output upstream exposes is upstream's:
//! tokens, AST, `validate()`'s errors, the render tree, HTML, and what a step throws (the
//! error's name and message). The old pipeline (`crate::parse`, `crate::transformer`, ...)
//! keeps working until the switch-over, when this module replaces it.
//!
//! ```
//! use markdoc::next::{index, parser::ParserArgs, renderers::html, schema::Config};
//!
//! let ast = index::parse("# Hello {% .greeting %}\n", &ParserArgs::default()).unwrap();
//! let config = Config::default();
//! assert!(index::validate(&ast, &config).unwrap().is_empty());
//! let tree = index::transform(&ast, &config).unwrap();
//! assert_eq!(
//!     html::render(&tree).unwrap(),
//!     r#"<article><h1 class="greeting">Hello </h1></article>"#
//! );
//! ```
//!
//! The pipeline, as `index.ts` exposes it ([`index`]):
//!
//! - [`index::parse`] (`Markdoc.parse(content, args)`): the default tokenizer (upstream's
//!   `new Tokenizer()`, every switch off) and the parser; [`index::parse_with`] takes a
//!   [`tokenizer::Tokenizer`] of the caller's, [`index::parse_tokens`] tokens. The AST is
//!   [`crate::ast::Node`] with [`crate::types::Scalar`] values, shaped as upstream's
//!   (`$$mdtype` Variable and Function values, attributes in a JS object's order); its
//!   `lines` and `location` are upstream's (0-based line maps, see [`parser`]), and
//!   [`crate::ast::Node::span`] is this crate's own position (1-based lines, UTF-16 columns,
//!   byte offsets), opt-in: only with [`parser::ParserArgs::spans`] on.
//! - [`index::validate`]: `validate()`'s entries as upstream's, `{type, lines, location,
//!   error}` ([`validator::ValidateError`]).
//! - [`index::transform`]: the render tree as upstream returns it, JavaScript values
//!   ([`schema::value::Value`]); [`schema::value::json_stringify`] writes its JSON as
//!   `JSON.stringify` does (node's depth limit), and [`schema::value::Value::to_render`] gives
//!   that JSON as a [`crate::types::RenderableTreeNode`] (to a lower depth, see its docs); both
//!   leave out what JSON cannot hold.
//! - [`renderers::html::render`]: the HTML of those values (`render_tree` renders a tree read
//!   from JSON); [`renderers::react::render`] the React elements of them (React behind a
//!   trait), [`renderers::react_static::render`] the JavaScript that builds them.
//! - [`formatter::format`]: an AST written back as Markdoc source (`Markdoc.format`).
//! - [`ast`]: `Ast.fromJSON`, `isAst`, `getAstValues`, ...; [`index::Markdoc`] the default
//!   export's config-bound instance. `CONFORMANCE.md` maps every export of upstream's
//!   `index.ts` to its item here.
//!
//! What a value inherits from JavaScript's built-ins (`$s.trim`, `$x.constructor`,
//! `$x.__proto__`) is [`realm`]: the built-in objects a value reaches, generated from node,
//! and what calling them does where upstream's conversions call a value's methods.
//!
//! An `Err` from any of them is where upstream throws, and says what:
//! [`index::thrown`] gives JavaScript's `String(error)`, e.g. `TypeError: Cannot create
//! property 'b' on string 'a'`. The only other `Err` is a refusal of an input this crate does
//! not model (`not modeled: ...`), listed with every other difference from upstream in each
//! module's "Divergences from upstream" section.
//!
//! The crate's own extensions are switches, off by default, as upstream has none: the
//! frontmatter's YAML parsed into an object ([`parser::ParserArgs::parse_frontmatter`]), a
//! byte-order mark left out of tokenizing ([`parser::ParserArgs::strip_bom`]).
//!
//! The harness that checks all of this against upstream's own dist (node) is
//! `tests/next/compare.py`, with `examples/next_compare` as its Rust half and [`compare`] its
//! converters; `tests/next/README.md` says how to run it. `tests/conformance.rs` grades
//! upstream's conformance corpus through this module.

pub mod ast;
pub mod formatter;
pub mod functions;
pub mod index;
pub mod parser;
pub mod realm;
pub mod renderers;
pub mod schema;
pub(crate) mod stack;
pub mod tags;
pub mod tokenizer;
pub mod transformer;
pub mod transforms;
pub mod validator;

#[doc(hidden)]
pub mod compare;
