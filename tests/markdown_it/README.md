# markdown-it port: verification harness

`src/markdown_it/` is a line-by-line port of markdown-it 15.0.1 (with Markdoc's
`allowIndentation` patch). It is correct when, for every input and option combination, its
token stream is identical, field by field, to the one the JavaScript produces. This
directory holds the tools that check that.

| file | what it does |
| --- | --- |
| `oracle_tokens.cjs` | node: the markdown-it bundled in upstream Markdoc 0.5.11, engine only (no Markdoc plugins); JSONL requests in, token JSON out |
| `../../examples/mdit_tokens.rs` | the same requests through the Rust port, the same JSON out |
| `compare.py` | runs both over an inputs file and reports the first differing token path per document; exits 1 on any difference |
| `base.jsonl` | small inputs the skeleton alone must already agree on |
| `matrix.py` | the verification matrix: `compare.py` over input files x the seven configurations (parse, and parseInline for `--inline` files), in parallel, one tally line per run |
| `fuzz.py` | grammar-based differential fuzzer: generates documents, runs both sides, minimises differences (below) |
| `golden.py` | writes `golden/<set>.txt` from the oracle, for the node-free test `tests/markdown_it_golden.rs` (below) |
| `spec.jsonl`, `regressions.jsonl` | the inputs of the golden test (below) |
| `unit_test_values.cjs` | prints, from node and the bundle, the values the unit tests under `src/markdown_it/` assert |
| `inline_rule_values.cjs` | the same for the inline rules text ... entity, balance_pairs, fragments_join and `common/html_re` |
| `core_values.cjs`, `core_cases.json` | the same for mdurl, punycode, link normalization and the core rules; `core_cases.json` holds the generated cases' pieces and node's digests (`--write`, `--check`, `--dump FILE`, `--examples`) |
| `../../examples/mdit_bench.rs` | throughput: parses a JSONL file of documents a number of rounds |

You need node (22) and upstream Markdoc's package: `@markdoc/markdoc` 0.5.11
`dist/index.js` (below `$MARKDOC`).

## Requests and output

One JSON object per line:

```
{"id": "x", "src": "*hi*", "preset": "commonmark", "options": {"allowIndentation": true},
 "enable": ["..."], "disable": ["lheading", "code"], "mode": "parse" | "parseInline",
 "env": {"references": {"FOO": {"href": "/u", "title": ""}}}}
```

Everything but `id` and `src` is optional. Both sides build `new MarkdownIt(preset, options)`
(default preset when absent; options use markdown-it's camelCase names), then
`enable(enable)`, then `disable(disable)`, and call `parse` or `parseInline` with a fresh
`env` (or the request's `env`: its `references` let `parseInline` resolve reference links).
They print `{"id", "tokens", "env"?}` (tokens as `plain()`: fields in the order type,
tag, attrs, map, nesting, level, content, markup, info, meta, block, hidden, errors, position,
children; a field is left out when null, undefined or '', `hidden` also when false; `env`
only when the parse left keys in it), or `{"id", "error"}` where JS throws (a panic or an
`Err` on the Rust side). compare.py counts "both sides threw" as agreement.

## Running it

```sh
# from the crate root; use your own target dir
export CARGO_TARGET_DIR=/path/to/your/target
python3 -I tests/markdown_it/compare.py INPUTS.jsonl --markdoc $MARKDOC [config flags]
```

Config flags (all merged into one request template applied to every input; request keys
on an input line win over it):

- `--preset default|commonmark|zero`, `--options '{"allowIndentation": true}'`,
  `--mode parseInline`, `--disable lheading,code`, `--enable ...`, or a whole
  `--template '{"preset": "zero", "mode": "parseInline"}'`.
- `--disable-unported`: disables, on both sides, every rule whose Rust file is still a stub
  (its body has a `// PORT:` comment). Rule names are the file names, and `linkify`,
  `emphasis` and `strikethrough` each name their rule in every ruler they are in, as in JS.
  Disabling `paragraph` makes `parse` throw "none of the block rules matched" on any
  non-blank document, on both sides; compare such configurations in `parseInline` mode.
- `--rust-bin target/release/examples/mdit_tokens` skips `cargo run`;
  `--id-prefix`, `--limit`, `--show N` (differences printed, default 10), `--keep DIR`
  (keep requests and both outputs).

Examples:

```sh
# the skeleton: blank documents parse, plain text parses inline
python3 -I tests/markdown_it/compare.py tests/markdown_it/base.jsonl --markdoc $MARKDOC --disable-unported
python3 -I tests/markdown_it/compare.py tests/markdown_it/base.jsonl --markdoc $MARKDOC --disable-unported --mode parseInline

# a porter checking emphasis (with balance_pairs and fragments_join ported) on the spec
python3 -I tests/markdown_it/compare.py inputs/spec.jsonl --markdoc $MARKDOC --disable-unported --mode parseInline

# Markdoc's two configurations
python3 -I tests/markdown_it/compare.py inputs/corpus.jsonl --markdoc $MARKDOC \
    --disable lheading,code --options '{"allowIndentation": true}'
python3 -I tests/markdown_it/compare.py inputs/corpus.jsonl --markdoc $MARKDOC \
    --disable lheading,code --options '{"allowIndentation": false}'
```

The last line printed is the tally, e.g.
`inputs 1304  same 1290  differ 14  both-error 0  node-error-only 0  rust-error-only 0  missing 0`;
each difference before it shows the document id, its source, the first differing path
(`tokens[3].children[1].content`) and both values.

## What is ported

Every file of markdown-it 15.0.1 and of the mdurl, punycode, entities and uc.micro code it
uses; `grep -rn '// PORT:' src/markdown_it` (what `--disable-unported` disables) is empty.
linkify-it is not ported: with `linkify: true` the core and inline linkify rules panic
where JS would call it (upstream Markdoc never enables `linkify`). Strings in tokens are
Rust `String`s, so a lone surrogate (which a Rust `&str` source cannot contain) would
become U+FFFD; positions are UTF-16 code units, as in JS.

## The golden test (no node needed)

`cargo test --test markdown_it_golden` compares the port with token streams the oracle
produced, checked in under `golden/`: every document of `spec.jsonl` and `regressions.jsonl`,
in the seven configurations of the verification matrix (`fuzz.CONFIGS`: default, Markdoc's
two, commonmark, zero, typographer, html) and in both `parse` and `parseInline` mode.
Configurations that agree on a document share one section, so the files stay small; the
format is described at the top of each file. After changing an input file, regenerate:

```sh
python3 -I tests/markdown_it/golden.py --markdoc $MARKDOC          # rewrite golden/*.txt
python3 -I tests/markdown_it/golden.py --markdoc $MARKDOC --check  # exit 1 if stale
```

- `spec.jsonl`: the 652 examples of the CommonMark spec 0.31.2 (ids `cm31-NNN`, the example
  numbers) and the 17 examples of spec 0.30 whose input differs from every 0.31.2 one
  (`cm30-NNN`). The spec is by John MacFarlane, licensed CC BY-SA 4.0
  (https://spec.commonmark.org/); only the example inputs are kept here.
- `regressions.jsonl`: each line has a `why`. `mutant-*` cases are fuzzer differences
  minimised against builds of the port with one seeded porting slip each (an off-by-one,
  a dropped guard, a wrong character class); each one fails under its slip, so the test
  catches that kind of mistake. `options-*` cases carry an `options` field (merged over
  every configuration's) with values of unusual types (`quotes` null or an object,
  `maxNesting` a string, null, NaN, ...); `limits-*` cases sit at markdown-it's limits
  (maxNesting, run lengths, digit counts). `edge-*`, `links-*`, `changelog-*` and
  `probes-*` are the hand-written probes the port was investigated with. Any difference
  the fuzzer finds goes here too, minimised, as `fuzz-*`.

The corpus comparison stays with `compare.py`, as the corpus is not in this repository.

## Fuzzing

```sh
python3 -I tests/markdown_it/fuzz.py run --markdoc $MARKDOC --rust-bin $CARGO_TARGET_DIR/release/examples/mdit_tokens \
    --seed 1 --count 25000 --configs default,markdoc-ai,markdoc-noai --failures fail.jsonl
python3 -I tests/markdown_it/fuzz.py min --markdoc $MARKDOC --rust-bin ... fail.jsonl min.jsonl
python3 -I tests/markdown_it/fuzz.py gen SEED COUNT out.jsonl     # just the documents
```

The grammar mixes block starts at every indentation (spaces and tabs), nested blockquotes
and lists with lazy lines, fences, indented code, tables, reference definitions, the seven
kinds of HTML block, headings, rules and Markdoc `{% %}` lines; inline it mixes delimiter
runs, brackets and links (inline, full, collapsed and shortcut references, deep
parentheses), backticks, entities, escapes, inline HTML, autolinks and typography; it uses
CRLF and CR line ends, NUL, non-ASCII spaces, astral characters and very deep nesting, and
mutates some documents at random. `run` streams the requests through both programs, a
batch at a time (`FUZZ_TMP` is where the batch file goes), and appends every difference
to `--failures`; `--configs` takes the seven matrix names and the extra ones in
`fuzz.EXTRA_CONFIGS` (parseInline, maxNesting 4, the quotes option). `min` delta-debugs
each failure (lines, then characters) to a small source on which the two sides still
differ.

## The generated tables

`src/markdown_it/common/entities.rs` and `ucmicro.rs` are generated from the npm packages
markdown-it depends on (`entities` 8, `uc.micro` 3):

```sh
node tools/gen_mdit_tables.cjs NODE_MODULES_DIR                   # regenerate
node tools/gen_mdit_tables.cjs NODE_MODULES_DIR --check           # exit 1 if stale
node tools/gen_mdit_tables.cjs NODE_MODULES_DIR --check --oracle $MARKDOC   # and cross-check with the bundle
```

`cargo test --test markdown_it_tables` runs the `--check` when node and the packages are
found (`MDIT_NODE_MODULES`, else `node_modules` in the crate, from `npm install entities uc.micro`) and
says it skipped otherwise.
