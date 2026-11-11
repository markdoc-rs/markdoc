# Tag grammar port: verification harness

`src/grammar/` is a line-by-line port of upstream Markdoc 0.5.11's `src/grammar/tag.js`, the
parser PEG.js 0.11.0-master generated from `tag.pegjs` for the content of a tag (everything
between `{%` and `%}`). It is correct when, for every input, it returns what tag.js returns
when Markdoc calls it as `parse(content, {Variable, Function})`: the same result object, or a
SyntaxError with the same `message`, `expected`, `found` and `location`. This directory holds
the tools that check that.

| file | what it does |
| --- | --- |
| `oracle_grammar.cjs` | node: upstream's tag.js with the published package's `Ast.Variable` and `Ast.Function`; JSONL requests `{id, content}` in, `{id, result}` or `{id, error: {message, expected, found, location}}` out, in `JSON.stringify`'s text |
| `../../examples/grammar_parse.rs` | the same requests through the port, the same lines out (`--bench ROUNDS` measures throughput instead) |
| `compare.py` | runs both over an inputs file and requires every line to be identical, byte for byte; names the first differing field otherwise; exits 1 on any difference |
| `extract_tags.py` | the content of every tag in a directory of documents, in a marktest `tests.yaml`, or the `parse(...)` cases of upstream's `tag.test.ts`; a tag ends at the first `%}` outside a double-quoted string, as upstream's `findTagEnd` decides |
| `fuzz.py` | random contents built from the grammar's tokens and near misses, most of them mutated so they fail at every depth; `--mode numbers` writes number literals of every size, exact doubles and halfway cases |
| `dist_check.cjs` | checks the grammar bundled in the package's `dist/index.js` against tag.js, through the dist's own Tokenizer: `{% content %}` must give the token (or error token) tag.js's result (or SyntaxError) implies |
| `golden.py`, `golden.jsonl` | the node-free regression set of `../grammar_golden.rs`: upstream's cases, every tag of `tests.yaml`, probes, and generated cases covering each result shape and each expectation set the fuzzer reached |

You need node (22), upstream's `src/grammar/tag.js` (`$TAG_JS`, v0.5.11) and the published
package's `dist/index.js` (`$MARKDOC`, `@markdoc/markdoc` 0.5.11).

```sh
# from the crate root; use your own target dir
cargo build --release --example grammar_parse
RUST=$CARGO_TARGET_DIR/release/examples/grammar_parse
python3 -I tests/grammar/extract_tags.py --out tags.jsonl --prefix corpus DOCS_DIR
python3 -I tests/grammar/extract_tags.py --out marktest.jsonl --prefix marktest --yaml tests/marktest/tests.yaml
python3 -I tests/grammar/fuzz.py --seed 1 --count 100000 --out fuzz1.jsonl
python3 -I tests/grammar/fuzz.py --seed 11 --count 100000 --mode numbers --out numbers.jsonl
python3 -I tests/grammar/compare.py fuzz1.jsonl --tagjs $TAG_JS --markdoc $MARKDOC --rust-bin $RUST
node tests/grammar/dist_check.cjs $TAG_JS $MARKDOC < fuzz1.jsonl
```

Where tag.js throws something else than its SyntaxError (a `RangeError` once values nest
past V8's stack, or a hash literal has so many entries that `Object.assign(head, ...tail)`
exceeds V8's argument limit), the oracle prints `{id, crash}`; the port returns
`ParseError::Range` where a fresh node process's stack runs out (the values' `grammar::cost`
on `markdoc::depth`'s budget: 4,074 nested arrays, 2,401 hashes, 2,353 calls; past
`grammar::MAX_NESTING`, 300 levels, where the stack cannot grow) and the example prints
`{id, crash}` too, which `compare.py` counts as agreement. V8's limits depend on its stack
and JIT (a warmed-up process goes to 5,000 and more), so between the two the port and V8
disagree (see `grammar::cost`); the port does not model the hash-size limit.
