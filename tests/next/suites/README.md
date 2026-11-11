# Upstream's own test suites, as inputs

Every input upstream's and its dependencies' own test suites give, so that the harness and
the node-free tests run them against the port. The scripts that make these files are in the
workspace's `findings/conformance/work/api/suites/` (`SUITES.md` beside them says, test file
by test file, what was taken and how, and what could not be and why).

## Input sets of `tests/next/compare.py` (this directory's `*.jsonl`)

Each line carries the options the test used (`allowIndentation`, `allowComments`,
`tokenizer`, `parse`, `config`, `formatOptions`, `components`), and stands for a document
(`code`), a token stream (`tokens`), an AST (`ast`, revived by `Ast.fromJSON` upstream), or a
render tree (`renderTree`), as the test made it. Run them with every stage:

```sh
python3 -I tests/next/compare.py --stage every tests/next/suites/*.jsonl --markdoc $DIST --rust-bin $BIN
```

| file | what |
| --- | --- |
| `upstream-<path>.jsonl` | upstream Markdoc 0.5.11's `src/<path>.test.ts`, run against its published dist by `run_test.cjs` (which records every document, token stream, AST, config, format option and render tree the tests hand to Markdoc's API) and sorted by `make_suites.py`; a config that holds a function cannot be written as JSON (those calls are Rust tests, see `golden/`) |
| `markdown-it-fixtures.jsonl` | markdown-it 15.0.1's `test/fixtures/markdown-it/*.txt` (212 fixtures), with its runner's options (`html`, `langPrefix: ''`, `typographer`, `linkify`) as Markdoc's `Tokenizer` options, `allowIndentation` and `allowComments` off |
| `commonmark-good.jsonl` | markdown-it 15.0.1's `test/fixtures/commonmark/good.txt` (652; `bad.txt` is empty, and `spec.txt`, which its runner reads, holds the same 652 examples); Markdoc's `Tokenizer` cannot select markdown-it's `commonmark` preset, so these go through upstream's default tokenizer (`engine/` has them with the preset) |
| `format-node-objects.jsonl` | hand-made ASTs whose text holds `Node` objects with fields of every type (`findings/conformance/work/api/fuzz_api.py nodes`, seeds 1-3, 100 each, and three repros): what `Ast.fromJSON` revives inside a value, formatted as upstream's `formatNode` reads it |
| `api-edges.jsonl` | the api stages' edge cases (`make_api_edges.py`): attribute values an annotation can give a node the formatter reads, every node type in every container, the formatter's options, render trees the React renderers read as upstream does |

## `engine/`: inputs of `tests/markdown_it/compare.py`

The markdown-it fixtures with exactly the preset and options markdown-it's own runners use
(`markdownit({html: true, langPrefix: '', typographer: true, linkify: true})` for
`test/fixtures/markdown-it/`, `markdownit('commonmark')` for `good.txt`), compared token for
token against the markdown-it upstream Markdoc bundles:

```sh
python3 -I tests/markdown_it/compare.py tests/next/suites/engine/markdown-it-fixtures.jsonl --markdoc $DIST
```

## `golden/`: node's answers, for `tests/next_suites.rs`

| file | what |
| --- | --- |
| `upstream-direct.jsonl` | the calls upstream's tests make to something other than a document's pipeline (`Node.walk`, `Variable.resolve`, `Function.resolve`, `Tag.isTag`, `findTagEnd`, `parseTags`, the tag grammar, the renderers with a `resolveTagName` of the test's own, `validate`/`transform` with function configs), with node's result |
| `upstream-format.jsonl` | `formatter.test.ts`'s `format(ast, options)` calls, with node's result |
| `linkify-fixtures.jsonl` | linkify-it 6.1.0's `test/fixtures/links.txt` and `not_links.txt` (from its repository at tag 6.1.0, read as data), run as its own `test/api.test.mjs` runs them through the linkify-it upstream Markdoc bundles (`linkify_fixtures.cjs`) |

The fixtures are markdown-it's (MIT, Copyright (c) 2014 Vitaly Puzrin, Alex Kocharin) and
linkify-it's (MIT, Copyright (c) 2015 Vitaly Puzrin); the test inputs, Markdoc's (MIT,
Copyright (c) 2021- Stripe, Inc.).
