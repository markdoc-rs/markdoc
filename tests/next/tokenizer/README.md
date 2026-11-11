# The tokenizer port: verification

`src/next/tokenizer/` ports upstream Markdoc 0.5.11's `src/tokenizer/index.ts`,
`src/tokenizer/plugins/{annotations,frontmatter,comments,link}.ts` and `src/utils.ts` onto the
markdown-it port (`src/markdown_it/`) and the tag grammar port (`src/grammar/`). It is right
when, for every document and every Tokenizer configuration, its token stream is upstream's,
field by field, and it returns an error exactly where upstream's `tokenize` throws (with the
same name and message). These are the tools that check that; the tokens stage of
`../compare.py` (with `examples/next_compare`) checks the input sets.

| file | what it does |
| --- | --- |
| `fuzz.py` | generates documents biased to Markdoc (tags in every block and inline context, fences with tags and `process=false`, multi-line and unclosed tags, `%}` in strings, block-level variables and functions, annotations, comments everywhere, frontmatter shapes with CRLF and unterminated), mutates half of them, and compares both sides batch by batch, in parallel; writes `differences.jsonl` |
| `minimise.py` | delta-debugs each difference to the smallest document that still differs the same way (lines, then characters) |
| `oracle_tokens.cjs` | node: upstream's tokens for `{id, code, tokenizer}`, or `{threw: {name, message}}` |
| `golden.py` | writes `golden.jsonl` from `oracle_tokens.cjs`: a probe of each quirk of upstream's plugins, and the minimised fuzz documents in `minimised.jsonl` |
| `golden.jsonl`, `minimised.jsonl` | the regression set `tests/next_tokenizer.rs` checks without node (with the 105 conformance cases' tokens from `../golden/conformance.jsonl`) |

You need node (22) and `@markdoc/markdoc` 0.5.11's `dist/index.js`.

```sh
export CARGO_TARGET_DIR=/path/to/your/target
cargo build --release --example next_compare
BIN=$CARGO_TARGET_DIR/release/examples/next_compare
python3 -I tests/next/tokenizer/fuzz.py --markdoc $DIST --rust-bin $BIN --count 300000 \
    --jobs 6 --seed 1 --out fuzz1 --config '{"allowIndentation": true, "allowComments": true}'
python3 -I tests/next/tokenizer/minimise.py fuzz1/differences.jsonl --markdoc $DIST \
    --rust-bin $BIN --out minimised-new.jsonl
python3 -I tests/next/tokenizer/golden.py --markdoc $DIST --minimised tests/next/tokenizer/minimised.jsonl
cargo test --test next_tokenizer
```

Give `--config` every switch (`allowIndentation`, `allowComments`, ...): the oracle's own
default for those two is true, upstream's `new Tokenizer()` has both false.

## What fuzz.py counts apart

- **equal after U+FFFD**: upstream's grammar quotes the one UTF-16 code unit it failed at, so
  an error message can hold half of an emoji (`{% 😀 %}`). A Rust `String` cannot hold a lone
  surrogate; the port's message has U+FFFD there. Everything else in the document is equal.
- **nested past MAX_NESTING (node parsed)**: the port returns `RangeError: Maximum call stack
  size exceeded` where a fresh node process does (4,074 nested arrays, 2,401 hashes, 2,353
  calls: `grammar::cost`, `markdoc::depth`), and a warmed-up node goes deeper; where the
  stack cannot grow (the `stack-growth` feature off, wasm), past `grammar::MAX_NESTING` (300)
  levels. Counted only when the document nests at least 300 deep.
- **lone surrogate read back** (gate.py and the fuzzers built on it): the engine made a lone
  surrogate in a token (an `xn--` link text that punycode decodes to one, a linkify
  `maxLength` cut inside a pair) and smartquotes read it back; the port's token holds U+FFFD,
  which is punctuation where a lone surrogate is not, so a quote next to it is curled
  differently. Counted when upstream's tokens, computed with U+FFFD put in place of each lone
  surrogate just before smartquotes (`oracle_tokens.cjs`'s `lossy`), equal the port's. A
  documented divergence (`src/markdown_it/mod.rs`).
- **both threw**: with the same error. Upstream throws for a closing tag inside a link's text
  (`[{%/a%}]()`: `TypeError: Cannot read properties of undefined (reading 'length')`, or
  `'push'` when emphasis follows), for a comment that a link label scan reaches with
  `allowComments` (`see [1 <!-- note -->`: `Error: inline rule didn't increment state.pos`),
  and for values nested past V8's stack.

Every other difference is a bug.

## linkify, options, depth (the linkify branch)

`linkify: true` runs the port of linkify-it 6.1.0 (`src/markdown_it/linkify_it/`), the
version markdown-it 15.0.1 depends on and upstream's dist bundles.

| file | what it does |
| --- | --- |
| `gate.py` | the tokens stage over input sets (streamed in batches) and Tokenizer configurations (`--configs`, default the twelve of the gate), thrown errors compared by name and message (compare.py counts any two throws as agreeing); prints a line per (configuration, set) |
| `linkify_oracle.cjs` | node: upstream's linkify-it (`md.linkify`) answering `test`, `match` and `matchAtStart` for `{id, text, options?}` |
| `linkify_fuzz.py` | fuzzes the port's linkify-it directly (its ignored test `linkify_it::tests::answer_requests`) against `linkify_oracle.cjs`: URLs, e-mails, hosts, IPs, ports, paths, schemes in every case and look-alike letters, all of linkify-it's options |
| `linkify_tokens_fuzz.py` | fuzz.py's Markdoc documents with links put in them (also inside links, autolinks, code, HTML, tags), through `gate.py`'s comparison |
| `linkify_classes.cjs` | checks the case-insensitivity the port's hand-written linkify matchers assume of V8; with `--table`, writes `linkify_classes.json` |
| `linkify_classes.json` | V8's answers, from the dist's own regexes, for the character classes the matchers are built on (separators, Z/Cc, Z/P/Cc, the pseudo-letter, case-insensitive literal letters) for every BMP code unit, lone surrogates in the contexts where uc.micro's `Any` takes them, and every astral code point; `linkify_it::tests::the_character_classes_are_v8s_for_every_code_point` checks the port against it |
| `stack_probe.py` | measures the stack a nested input needs (debug lib test binary, `stack::probe`): how `markdown_it::MAX_DEPTH` (160), the limit where the stack cannot grow, was chosen |
| `node_depth.py`, `node_depth.cjs` | how deep node nests each construct before V8's RangeError (`maxNesting` 1e9), in a fresh process and after 3,000 documents (the costs in `markdown_it::cost` come from `findings/conformance/work/depth/node_shapes.cjs`, which measures every shape the same way) |

```sh
cargo test --release --lib --no-run            # BIN: the lib test binary it prints
python3 -I tests/next/tokenizer/linkify_fuzz.py --markdoc $DIST --rust-test-bin $BIN \
    --count 100000 --seed 2 --jobs 2 --options '{"fuzzyLink": true}' --out DIR
python3 -I tests/next/tokenizer/gate.py --markdoc $DIST --rust-bin $NEXT_COMPARE \
    --inputs $INPUTS/spec.jsonl $INPUTS/corpus.jsonl --conformance --out DIR
python3 -I tests/next/tokenizer/linkify_tokens_fuzz.py --markdoc $DIST --rust-bin $NEXT_COMPARE \
    --count 100000 --seed 11 --jobs 2 --out DIR
node tests/next/tokenizer/linkify_classes.cjs $DIST --table > tests/next/tokenizer/linkify_classes.json
python3 -I tests/next/tokenizer/node_depth.py $DIST
```

Both fuzzers take `--astral`: characters of every plane in hosts, TLDs, paths and around the
links (uc.micro's 228 astral punctuation characters most of all, read from
`linkify_classes.json`), and `xn--` labels that decode to surrogate code points; give the
tokens fuzzer a `typographer` config with astral `quotes` too. The robustness of the engine's
public functions (no argument makes one panic) is the lib test
`markdown_it::robustness::every_public_function_takes_any_argument`; `MDIT_ROBUSTNESS=N` runs
N rounds of it.

Run each under `bin/capped`.
