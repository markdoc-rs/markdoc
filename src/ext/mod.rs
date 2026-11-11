//! Extensions to upstream Markdoc: layers this crate keeps beside the conformant core.
//!
//! The core ([`crate::next`]) is upstream Markdoc 0.5.11. Every layer here is opt-in and off
//! by default: a caller that does not reach for one gets exactly what upstream produces, and a
//! layer that is switched off changes nothing upstream produces. Each layer is public API of
//! this crate (it targets github.com/markdoc-rs), so none of them knows about a particular
//! corpus; data that belongs to one corpus lives in that corpus's schema overlay.
//!
//! | Layer | Module | What it adds |
//! |---|---|---|
//! | E1 | [`ParseOptions`], [`parse()`] | One value for the tokenizer's options and the parser's arguments, the BOM and frontmatter switches, read from a `[parse]` table |
//! | E2 | [`bundle`] | The TOML schema bundle (`schema/tags.toml`) and overlays, loaded into a [`crate::next::schema::Config`] |
//! | E3 | [`control_flow`] | `switch`, `case` and `for` as schemas, and an AST pass for conditionals |
//! | E4 | [`partials`] | Partial resolvers, a loader that fills `config.partials`, and AST expansion |
//! | E5 | [`crossrefs`] | `tag`/`tagref` cross-reference checks |
//! | E6 | [`occurrences`](mod@occurrences) | Every `{% %}` the tokenizer recognised, with its span and partner |
//! | E7 | [`functions`] | `include`/`includes`, and an adapter for positional functions |
//! | E8 | [`variables`] | `config.variables` from frontmatter and caller maps, with `$markdoc.frontmatter` |
//! | E9 | [`frontmatter`] | Upstream's frontmatter delimiters, and YAML to [`crate::types::Scalar`] |
//!
//! E1's items and E6's entry points are also here at the root (`ext::parse`,
//! `ext::occurrences`, ...); the other layers are used through their modules. The layers
//! meet in two places: a [`ParseOptions`] (a bundle's `[parse]` table,
//! [`bundle::Bundle::parse_options`]) is what [`parse()`], [`occurrences()`] and
//! [`partials::load_partials`] read a text with, so they agree on what is a tag; and a
//! [`crate::next::schema::Config`] is what the others fill ([`bundle::Bundle::apply`],
//! [`control_flow::apply_bundle`], [`functions::all`], [`variables::Bindings::apply`],
//! `config.partials`). A layer nobody calls is not in either.
//!
//! ```
//! use markdoc::ext::bundle::Bundle;
//! use markdoc::ext::partials::{InMemoryPartialResolver, load_partials};
//! use markdoc::ext::variables::frontmatter_variables;
//! use markdoc::ext::{OccurrenceKind, control_flow, functions, occurrences, parse};
//! use markdoc::next::{index, renderers::html};
//!
//! let mut bundle = Bundle::builtin();
//! bundle
//!     .overlay("[parse]\nstrip_bom = true\nfrontmatter = \"yaml\"\n")
//!     .unwrap();
//! let options = bundle.parse_options();
//! let source = "\u{FEFF}---\nproducts: [Bot250, Bot600]\n---\n\
//!               {% for $markdoc.frontmatter.products as=\"p\" %}\n\
//!               {% notice %}\n{% $p %}\n{% /notice %}\n\
//!               {% /for %}\n\n\
//!               {% if include($markdoc.frontmatter.products, \"Bot600\") %}\n\
//!               {% partial file=\"end.md\" /%}\n\
//!               {% /if %}\n";
//! let doc = parse(source, &options).unwrap();
//!
//! let mut config = bundle.config();
//! control_flow::apply_bundle(&bundle, &mut config);
//! config.functions.extend(functions::all());
//! frontmatter_variables(&doc).apply(&mut config);
//! let resolver = InMemoryPartialResolver::new().with("end.md", "\u{FEFF}# End\n");
//! config.partials = Some(load_partials(&doc, &resolver, &options).unwrap());
//!
//! assert_eq!(
//!     html::render(&index::transform(&doc, &config).unwrap()).unwrap(),
//!     "<article><callout type=\"note\"><p>Bot250</p></callout>\
//!      <callout type=\"note\"><p>Bot600</p></callout><h1>End</h1></article>"
//! );
//! // The same options find the same tags: for, notice, $p, /notice, /for, if, partial, /if.
//! let tags = occurrences(source, &options).unwrap();
//! assert_eq!(tags.len(), 8);
//! assert!(matches!(&tags[0].kind, OccurrenceKind::Open { name, .. } if name == "for"));
//! assert_eq!(tags[0].partner, Some(4));
//! ```

mod parse;

pub mod bundle;
pub mod control_flow;
pub mod crossrefs;
pub mod frontmatter;
pub mod functions;
pub mod occurrences;
pub mod partials;
pub mod variables;

pub use occurrences::{Occurrence, OccurrenceKind, occurrences, parse_with_occurrences};
pub use parse::{Frontmatter, ParseOptions, parse};

#[cfg(test)]
mod tests;
