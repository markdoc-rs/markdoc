//! Markdoc for Rust: a port of [Markdoc](https://markdoc.dev) v0.5.11, the JavaScript original
//! by Stripe, onto a port of markdown-it 15.0.1.
//!
//! From the same source and the same configuration, every output upstream exposes is
//! upstream's: tokens, AST, [`validate`]'s errors, the render tree, HTML, and what a step
//! throws. `tests/conformance.rs` grades upstream's own conformance corpus (105 of 105), and
//! CONFORMANCE.md lists every difference.
//!
//! ```
//! use markdoc::parser::ParserArgs;
//! use markdoc::renderers::html;
//! use markdoc::schema::Config;
//!
//! let ast = markdoc::parse("# Hello {% .greeting %}\n", &ParserArgs::default()).unwrap();
//! let config = Config::default();
//! assert!(markdoc::validate(&ast, &config).unwrap().is_empty());
//! let tree = markdoc::transform(&ast, &config).unwrap();
//! assert_eq!(
//!     html::render(&tree).unwrap(),
//!     r#"<article><h1 class="greeting">Hello </h1></article>"#
//! );
//! ```
//!
//! The pipeline is upstream's `index.ts`: [`parse`] (or [`parse_with`] a [`tokenizer::Tokenizer`]
//! of the caller's, [`parse_tokens`] from tokens), [`validate`], [`transform`] (a
//! [`schema::value::Value`], as upstream returns it) and [`renderers::html::render`]. The
//! modules are upstream's: [`parser`], [`tokenizer`], [`schema`], [`tags`], [`functions`],
//! [`transformer`], [`validator`], [`renderers`] (`html`, `react`, `reactStatic`),
//! [`transforms`] and [`formatter`] (`Markdoc.format`), plus the tag grammar ([`grammar`]), the
//! AST ([`ast`]), the values ([`types`]), the built-in objects a value reaches ([`realm`]) and
//! the model of node's call stack ([`depth`]). Upstream's `Ast` namespace (`fromJSON`,
//! `isAst`, `getAstValues`, ...) is [`next::ast`] until the physical move gives it a root
//! path (the root's `ast` is the crate's AST types).
//!
//! The crate's own extensions are in [`ext`], every one off until a caller turns it on: parse
//! switches and a TOML schema bundle with overlays, control flow, partials from files,
//! cross-references, tag occurrences, more functions, variable bindings, a frontmatter helper.
//! So are the parser's own: this crate's positions ([`ast::Node::span`], 1-based lines, UTF-16
//! columns and byte offsets, on nodes and errors) are found only with
//! [`parser::ParserArgs::spans`] on (they cost about half of a parse); a tool that reports
//! positions, such as a linter, turns them on (`ParserArgs { spans: true, ..Default::default()
//! }`, or [`ext::ParseOptions::with_spans`]). Upstream's own `lines` and `location` are there
//! either way.
//!
//! [`next`] is the path all of this had while the old pipeline was being replaced; it is kept,
//! deprecated, for one release.

pub mod ast;
pub mod depth;
pub mod ext;
pub mod grammar;
pub mod markdown_it;
/// **Deprecated**: the old path of the crate's API, kept for one release. Use the root paths.
///
/// Every `markdoc::next::` path names the same item as the root path without `next::`, and
/// the index functions are at the root: `markdoc::next::index::parse` is [`crate::parse`],
/// `markdoc::next::schema::Config` is [`crate::schema::Config`],
/// `markdoc::next::renderers::html` is [`crate::renderers::html`]. Nothing changes but the
/// path, so a consumer moves by deleting `next::` (and `index::`). This module goes in the
/// next release.
///
/// What follows is the module's documentation from before the switch-over, when the old
/// pipeline still stood beside it.
pub mod next;
pub mod positions;
#[doc(hidden)]
pub mod sourcemap;
pub mod types;

#[doc(inline)]
pub use next::{
    formatter, functions, index, parser, realm, renderers, schema, tags, tokenizer, transformer,
    transforms, validator,
};

/// The converters of the harness that compares the crate with upstream's dist
/// (`tests/next/compare.py`).
#[doc(hidden)]
pub use next::compare;

#[doc(inline)]
pub use next::index::{
    Markdoc, create_element, default_tokenizer, merge_config, parse, parse_tokens, parse_with,
    resolve, resolve_all, thrown, transform, transform_all, validate,
};

pub use ast::{Function, Node, PathSegment, Variable};
/// The `indexmap` crate this crate's maps are made of, so that a consumer names the same
/// version (attributes, slots, `Scalar::Object` and every schema map are `IndexMap`s).
pub use indexmap::{self, IndexMap};
