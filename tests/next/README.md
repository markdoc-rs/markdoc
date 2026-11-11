# The next layer: comparison harness

`src/next/` ports upstream Markdoc 0.5.11 (tokenizer plugins, parser and table transform,
transformer, validator, schema, functions, tags, HTML renderer) onto the markdown-it port. It
is right when, for every input and every configuration, its tokens, AST, validation errors,
render tree and HTML are upstream's, and it throws where upstream throws, with upstream's
error name and message. These are the tools that check that.

| file | what it does |
| --- | --- |
| `oracle.cjs` | node: upstream's dist answers each request with any of `tokens`, `ast`, `errors`, `transformed`, `html`, `partials`, `formatted`, `reactStatic`, `react`, and what each step threw (its header has the request and answer format) |
| `../../examples/next_compare/main.rs` | the same requests through the Rust port (its header has the format) |
| `../../examples/next_compare/plugins.rs`, `parser.rs`, `semantics.rs`, `api.rs` | how each stage calls the port: `Tokenizer`, `next::index::parse_tokens`, `next::index::{transform, validate}`, the HTML renderer, and the api stages (`next::formatter::format`, `next::renderers::{react_static, react}` with a recording React) |
| `compare.py` | runs both over JSONL inputs and reports each document's first difference per stage, then totals; `--roundtrip` checks the converters; `--golden` writes the golden file |
| `gen_semantics.py` | generated configs (every schema feature) and tag-heavy documents for the transform, errors and html stages (its header has the usage) |
| `golden/conformance.jsonl` | upstream's outputs for the 105 conformance cases, read by the node-free `tests/next_compare.rs` and by the parser's and semantics' unit tests |
| `../../src/next/compare.rs` | the converters: Node, `validate()` entries, render tree and tokens to and from upstream's JSON, whole |
| `tokenizer/` | the tokens stage's own gate and fuzzers (its README) |
| `suites/` | upstream's own test suites as input sets (Markdoc's `src/**/*.test.ts`, markdown-it's and linkify-it's fixtures), and node's answers for `tests/next_suites.rs` (its README) |
| `realm/` | the config-lookup audit (`audit.jsonl`; `audit-ast.jsonl`, `ast` requests: run its stages errors, transform, html) and the built-in calls' fixture (`conversions.jsonl`, `tools/gen_realm_conversions.cjs`) |

You need node (22) and upstream's package: `@markdoc/markdoc` 0.5.11's `dist/index.js`
(`--markdoc PATH` or `$MARKDOC_DIST`). Run every process that can grow under
`findings/conformance/bin/capped`.

## Running it

```sh
export CARGO_TARGET_DIR=/path/to/your/target
cargo build --release --example next_compare
BIN=$CARGO_TARGET_DIR/release/examples/next_compare
# every stage, end to end, over input sets and the conformance cases:
python3 -I tests/next/compare.py --stage all inputs.jsonl --conformance --markdoc $DIST --rust-bin $BIN
# the same under several configurations (a JSON list, inline or a file):
python3 -I tests/next/compare.py --stage all inputs.jsonl --templates configs.json --markdoc $DIST --rust-bin $BIN
# one stage alone, from upstream's output for the stage before:
python3 -I tests/next/compare.py --stage ast --from-oracle --conformance --markdoc $DIST --rust-bin $BIN
python3 -I tests/next/compare.py --roundtrip inputs.jsonl --conformance --markdoc $DIST --rust-bin $BIN
```

- Stages: `tokens`, `ast`, `errors`, `transform`, `html`, or `all` (the five from one run of
  each side per document); the api stages `format` (`Markdoc.format` of the AST, with the
  line's `formatOptions`), `reactStatic` (`renderers.reactStatic` of the render tree) and
  `react` (`renderers.react`, with the line's `components`), or `api` (the three) and `every`
  (all eight). `react` records each `createElement(type, props, ...children)` call of a fake
  React, on both sides alike (`{calls, result}`: an element is the index of the call that made
  it, `Fragment` `{"$fragment": true}`, a value JSON cannot hold `{"$undefined": true}`,
  `{"$number": "NaN"}`, ...), so the calls and their order are compared. `reactStatic` runs
  with upstream's default `resolveTagName` under its source's name (the dist's bundler
  renamed it `tagName2`, which `reactStatic` rejects: see `CONFORMANCE.md`).
- **End to end** is the default: both sides start from the source, so the port's every
  output is made by its own tokenizer, parser and semantics. `--from-oracle` instead gives
  the port upstream's output for the stage before (its tokens, its AST and partials' ASTs,
  its render tree), so one stage can be checked alone.
- Inputs: JSONL, `{id, src or code}` plus any request key: `config`, `slots`, `file`,
  `partials` (`{name: source}`), `allowIndentation`, `allowComments` (both default true, the
  conformance runner's setting; upstream's own `new Tokenizer()` has both false), `tokenizer`
  (more Tokenizer options: `allowLinkValidation`, `linkValidationOptions`, markdown-it's
  `html`, `typographer`, ...), `parse` (more ParserArgs, e.g. `conditionalTags`),
  `formatOptions`, `components` (an object, or `"$function"` for `(name) => ({$component:
  name})`). A line may stand for what upstream's tests build by hand instead of a document:
  `tokens` (plain() tokens, parsed instead of tokenizing), `ast` (Node JSON, revived by
  `Ast.fromJSON` upstream: validated, transformed and formatted instead of parsing) or
  `renderTree` (any JSON: rendered by html, reactStatic and react instead of the document's).
  `html`, `typographer`, ...), `parse` (more ParserArgs, e.g. `conditionalTags`), `ast` (an
  AST in upstream's JSON to validate and transform instead of parsing: `Ast.fromJSON`).
  `--template JSON` is merged under every line; `--templates` runs the inputs once per
  template of a list; `--conformance` adds upstream's corpus.
- In `config`, types are upstream's strings (`"type": "String"`), a constructor or one of
  upstream's classes is `{"$type": name}` (`String` ... `Array`, `Class`, `Id`,
  `ConditionalAttributeType`, `PartialFile`), and a RegExp is `{"$regex": source, "flags": f}`;
  functions cannot be given (a configured function without a `transform` resolves to
  undefined, as upstream). Everything else is the JSON as `JSON.parse` makes it: a
  `"__proto__"` key is an own property on both sides (the oracle defines each key, as
  `JSON.parse` does, rather than assigning it).
- `tokens` (a hand-made token stream, `plain()` form) is parsed instead of `code` on both
  sides. The oracle revives it with `Ast.fromJSON` (a `$$mdtype` object becomes its class's
  instance; one `Ast.fromJSON` cannot construct throws at the tokens stage), and the port
  reads it the same way: the parser revives `meta` values, and the tokens it writes back show
  `meta` as revived.
- `--rust-bin PATH` skips `cargo run`; `--show N`, `--limit N`, `--id-prefix P`, `--keep DIR`
  (`differences.jsonl`: each difference with its stage, kind and the whole document).
- Documents go through both sides in batches (`--batch 400`, `--jobs 2` at once), so memory
  stays flat. A port process that dies on a document (an abort, a stack overflow, an
  allocation past `--port-memory`, `--timeout`) is restarted after it, and the document counts
  as `port crashed`; with `--stage all` its stages are then run one at a time. Node's heap is
  capped by `--oracle-memory`. Give `capped` room for `--jobs` times both caps (the defaults,
  1024 MB each, cover any input the sets hold with 2G and two jobs; a cgroup that runs out
  stops compare.py as a whole).
- Exit status 0 only when no document differs at any stage compared.

## What is compared

What `JSON.stringify` writes of upstream's value, whole, against the port's JSON of its own:
objects key by key **in order** (key order is output: upstream's attribute order shows in
validation errors, render trees and HTML, and every object's keys are in upstream's order),
numbers as numbers (JavaScript has one number type: `1` and `1.0` are one value). Nothing is
left out, filled in or reordered on either side; the one mapping is a lone surrogate's (see
"Strings" below: a host artifact, a Rust `String` cannot hold one):

| stage | the JSON |
| --- | --- |
| tokens | the token stream: markdown-it's tokens with all thirteen fields in the dist's order (`map`, `level`, `children`, `content`, `markup`, `info`, `block`, `hidden`, `type`, `tag`, `attrs`, `nesting`, `meta`) and `errors` where a plugin set them; the objects `parseTags` makes for a fence's `children` with their own fields alone (`{type, start, end, content}`, `{map, position, start, end, info, type, nesting?, meta}`: a field an object lacks is absent, not 0 or false) |
| ast | upstream's Node JSON with every field: `lines` and `location` (upstream's: 0-based line maps, see `src/next/parser.rs`) and each error whole (`{id, level, message, location?}`, or what a hand-made token gave) |
| errors | `validate()`'s entries whole: `[{type, lines, location?, error}]` |
| transform | the render tree |
| html | the string |

The document's `frontmatter` attribute is compared as it is: by default the port's is
upstream's raw text (parsing the YAML is the crate's switch, off by default). This crate's own
positions (`Node::span`, `ValidationError::span`: 1-based lines, UTF-16 columns, byte offsets)
are not upstream's and are in no JSON compared.

**Thrown errors**, at every stage: upstream's name and message (`TypeError: Cannot create
property 'b' on string 'a'`, JavaScript's `String(error)`) against what the port says it
throws (`next::index::thrown`, the tokenizer's `TokenizeError`). A stage's error is the first
one thrown on its way: tokens, ast, then partials and transformed (transform, html) or errors.
Two sides that threw the same agree; `threw differently`, `port threw` and `oracle threw` do
not, and neither does a port panic (`panic: ...`) or crash.

A render tree `JSON.stringify` cannot write (a value with a `toJSON` that throws) is
`unwritable` on either side (the transform did not throw; `examples/next_compare` says so as
the oracle does, and still renders the tree's HTML): both unwritable with the same error
agree (`both unwritable`); `unwritable differently` and `port unwritable` do not.

Counted apart and not compared: `oracle unwritable` (upstream made the value but
`JSON.stringify` cannot write it for V8's stack, e.g. an AST nested past what V8's stack lets
it write),
`skipped` (with `--from-oracle`, upstream threw before the stage's input), `harness` (a side
could not read the request).

Strings: a JS string can hold a lone UTF-16 surrogate, a Rust `String` cannot, and by the
user's decision tokens, the AST and messages stay Rust `String`s. So the one mapping is this
boundary, on both sides alike: in every field of an answer but the render tree (tokens, the
AST, `validate()`'s entries, the HTML, thrown messages; keys too), a lone surrogate becomes
U+FFFD before comparing, which is what node itself writes for one when it encodes UTF-8 and
what the port's `String`s hold. The render tree (`transformed`) is compared exactly: values
hold JS strings (`schema::value::JsString`), lone surrogates included. Inputs reach both
sides exactly: a request whose strings hold a lone surrogate is written as ASCII JSON, whose
`\udxxx` escapes node's `JSON.parse` reads exactly and `next_compare` reads exactly into the
config's `variables` (`Value::from_json_text`; serde_json, which reads the rest of the
request, cannot hold them, so elsewhere in a config they are U+FFFD on the port's side).
What the boundary leaves, and nothing else: a lone surrogate the engine makes (astral
typographer `quotes`, an `xn--` host decoding to a surrogate, linkify-it's `maxLength`
cutting a pair) is U+FFFD in the port's token and AST strings, so where upstream's render
tree carries such an AST string it holds the code unit and the port's U+FFFD, and two halves
in two text nodes that upstream's HTML renderer joins into one character are two U+FFFD in
the port's HTML. The totals say how many answers held a lone surrogate; no document is left
out over one. Every walk over a value in `compare.py` is iterative, so any depth either side
can write is compared.

## The Rust side

`examples/next_compare` answers `{id, stage or stages, code, ...}` with each output and, for
what threw, `failed: {step: "Name: message"}` (the oracle's step names) and `threw`, the
first of them. For tests that run without node, `markdoc::next::compare` reads
`golden/conformance.jsonl`'s values (`node_from_json`, `tokens_from_json`,
`render_from_json`) and writes the port's (`node_json`, `tokens_json`, `render_json`,
`errors_json`) as upstream's JSON, whole, and `first_difference` compares them, key order
included. The transform stage's answer is `JSON.stringify` of the port's values
(`schema::value::json_stringify`, written into the answer as it is), as the oracle writes
upstream's tree, with the same depth limit.
