# markdoc

Rust implementation of the [Markdoc](https://markdoc.dev) document
language: CommonMark plus a `{% tag %}` extension, with frontmatter,
variables, functions, partials, conditionals and a configurable schema.
It is a port of Stripe's [JavaScript implementation](https://github.com/markdoc/markdoc),
v0.5.11, onto a line-by-line port of the markdown-it 15.0.1 it tokenizes with:
from the same source and configuration, every output upstream exposes is
upstream's (tokens, AST, validation errors, render tree, HTML, and what a step
throws). [CONFORMANCE.md](CONFORMANCE.md) says how that is checked and lists
every difference.

This crate is **application-agnostic**: it parses, validates, transforms and
renders to HTML, but knows nothing about how its output is used. The PDF
renderer and the Leptos integration each live in their own sibling crate.

```
.mdoc source
     │
     ▼
   parse (tokenizer + parser; ext::parse for the crate's parse options)
     │
     ▼
   Node  ──►  validate  ──►  Vec<ValidateError>
     │
     ▼
 transform (with a schema::Config: tags, nodes, variables, functions, partials)
     │
     ▼
 schema::value::Value  (to_render() for a RenderableTreeNode)
     │
     ├──►  renderers::html::render                         ── built in
     ├──►  markdoc-pdf                                     ── sibling
     └──►  leptos-markdoc <Markdoc/>                       ── sibling
```

## Usage

```rust
use markdoc::parser::ParserArgs;
use markdoc::renderers::html;
use markdoc::schema::Config;

let doc = markdoc::parse("# Hello\n\nThis is **Markdoc**.", &ParserArgs::default())?;
let config = Config::default();
let errors = markdoc::validate(&doc, &config)?; // upstream's ids, levels and messages
let tree = markdoc::transform(&doc, &config)?;  // upstream's render tree, as values
let html = html::render(&tree)?;                // "<article><h1>Hello</h1>..."
```

`schema::Config::default()` is upstream's empty config: `transform` and
`validate` add upstream's nodes, its five tags (`if`, `else`, `partial`, `slot`,
`table`) and its six functions (`and`, `or`, `not`, `equals`, `default`,
`debug`), and nothing else. A tag the config does not declare renders only its
children, and only declared attributes reach the output.

## Extensions (`markdoc::ext`)

Everything the crate adds to upstream is a layer in `markdoc::ext`, off until a
caller turns it on, so that with no layer every output is upstream's:

| Layer | What it adds |
|-------|--------------|
| `ext::{parse, ParseOptions, Frontmatter}` | Parse options in one value: the tokenizer's switches (`allow_indentation`, `allow_comments`, the typographer), BOM stripping, frontmatter as YAML |
| `ext::bundle` | The tag schema as TOML: the crate's own [`schema/tags.toml`](schema/tags.toml) (`Bundle::builtin()`), overlays merged field by field, `[parse]`/`[render]` options, `[validation]` and `[functions.*]` declarations |
| `ext::control_flow` | `{% switch %}`/`{% case %}` and `{% for %}`, and an AST pass that evaluates control flow before transform |
| `ext::partials` | Partials read from files (`FsPartialResolver`) or memory, cycle-checked, into `Config.partials` or expanded into the tree |
| `ext::crossrefs` | `{% tag %}` anchors and `{% tagref %}` references |
| `ext::occurrences` | Every tag occurrence in a source, with byte spans and partners, from the real tokenizer |
| `ext::functions` | `include`/`includes`, `all`, and `positional` for functions written as `fn(&[Scalar])` |
| `ext::variables` | `$markdoc.frontmatter`, `$config`, `$global` and others bound into `Config.variables` |
| `ext::frontmatter` | Splitting and reading frontmatter on its own |

```rust
use markdoc::ext::bundle::Bundle;
use markdoc::ext::parse;
use markdoc::renderers::html;

let mut bundle = Bundle::builtin();
bundle.overlay(&std::fs::read_to_string("tags.overlay.toml")?)?;
let config = bundle.config();
let doc = parse("{% callout type=\"warning\" %}\nMind the gap.\n{% /callout %}\n", &bundle.parse_options())?;
let html = html::render(&markdoc::transform(&doc, &config)?)?;
```

A function a schema file declares (`[functions.<name>]`) has no code: the tool
registers it (`config.functions`), applies the bundle, and calls
`Bundle::check_functions`, which fails while a declared function has nothing
to run (it would otherwise resolve to `null` and hide what it guards).

## Control flow

`{% if %}` and `{% else %}` are upstream's, evaluated at transform time.
`ext::control_flow` adds `{% switch %}`/`{% case %}` and `{% for %}`
(declared in `schema/tags.toml` with `layer = "control_flow"`, so they load
only with the layer):

```markdoc
{% switch $robot.status %}
{% case "ok" %}Ready for work.{% /case %}
{% case "service" %}Out for service.{% /case %}
{% case %}Unknown status.{% /case %}
{% /switch %}

{% for $fleet as="robot" index="n" %}
- Robot {% $n %}: {% $robot.name %}, {% $robot.payload %} kg
{% /for %}
```

[`examples/control_flow.rs`](examples/control_flow.rs) renders
[`examples/control_flow.md`](examples/control_flow.md) through the bundle, the
control-flow layer, two functions of its own and frontmatter variables:

```sh
cargo run --example control_flow
```

## Module map

| Module | Purpose |
|--------|---------|
| `parse`, `parse_with`, `parse_tokens`, `validate`, `transform`, `transform_all`, `resolve`, `merge_config`, `create_element` | Upstream's `index.ts` (also as `index::*`) |
| `tokenizer` | `Tokenizer`, `TokenizerOptions`; `tokenizer::utils::parse_tags` (lexical tags) |
| `parser` | `ParserArgs`, tokens to `ast::Node` |
| `grammar` | One tag body: `grammar::parse` |
| `ast` | `Node`, `Variable`, `Function`, `PathSegment` |
| `schema` | `Config`, `Schema`, `SchemaAttribute`, `ConfigFunction`, `TypeSpec`, ...; `schema::value::Value` |
| `tags`, `functions`, `transforms` | Upstream's built-in tags, functions and the table transform |
| `transformer`, `validator` | Upstream's transformer and validator |
| `renderers::html` | The HTML renderer |
| `types` | `Scalar`, `Tag`, `RenderableTreeNode`, `NodeType`, `ValidationError`, `MarkdocError`, ... |
| `positions` | `LineIndex` (byte offset to line and column) |
| `markdown_it` | The markdown-it 15.0.1 port the tokenizer runs on |
| `ext` | The crate's extensions (above) |
| `next` | **Deprecated**: the same items under their pre-switch paths, for one release |

## Sibling crates

| Crate | Builds on this for… |
|-------|---------------------|
| [`markdoc-pdf`](../markdoc-pdf) | CLI + library that renders a parsed tree to PDF (krilla + parley) |
| [`leptos-markdoc`](../leptos-markdoc) | Leptos components for web/mobile UI rendering |

Each sibling depends on this crate via a relative `path = "../markdoc"`
reference and pins its own additional deps.

## Differences from the JavaScript original

- **Type system**: Rust enums (`Scalar`, `Node`, `Value`) rather than untyped
  JS objects; `Result` where upstream throws, with upstream's error
  (`markdoc::thrown` gives JavaScript's `String(error)`).
- **Positions**: nodes and errors also carry the crate's own positions
  (1-based lines, UTF-16 columns and byte offsets).
- **Everything else** is a divergence listed, with its input and both
  behaviours, in [CONFORMANCE.md](CONFORMANCE.md) and the "Divergences from
  upstream" section of the module it belongs to.

## Conformance

`tests/marktest/` vendors Stripe's own conformance corpus
(`spec/marktest/tests.yaml`, 105 cases, unmodified, at the revision in
[`tests/marktest/UPSTREAM.md`](tests/marktest/UPSTREAM.md)), and
`tests/conformance.rs` runs every case through the crate's root paths and
grades it by the rule upstream's own runner applies (`index.ts`, vendored
beside it). **105 of 105 pass.**
[`tests/conformance-baseline.txt`](tests/conformance-baseline.txt) records the
verdict for every case.

The test is a ratchet, and it fails in both directions. A case the baseline
records as passing that now fails is a regression. A case it records as
failing that now passes is an improvement, recorded with

    MARKDOC_CONFORMANCE_BLESS=1 cargo test --test conformance

The corpus is never edited to fit the parser. The test checks its sha256 on
every run, and CI fetches every vendored file from upstream and compares it
byte for byte (`tests/marktest/verify-vendored.sh`). Moving to a newer
upstream revision is a wholesale replacement; the steps are in `UPSTREAM.md`.

Beyond that corpus, `tests/next/` compares the crate with upstream's own
published dist under node, output for output (`tests/next/README.md`).

## Examples

```sh
cargo run --example basic         # parse, validate, transform, render
cargo run --example control_flow  # switch / for, custom functions, variables
```

## License

MIT. Original Markdoc © Stripe, used here under the same licence.
