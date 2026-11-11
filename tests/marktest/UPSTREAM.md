# Vendored from upstream Markdoc

The five files in the table below are copied unmodified from Stripe's
Markdoc, the JavaScript original this crate is a port of, at one pinned
revision. None of them is executed. `tests.yaml` is the corpus
`../conformance.rs` reads and grades; the other four are the reference that
grading cites. This file and `verify-vendored.sh` are this crate's own.

revision: 9440a1d45670d367090d66446c5e387028ea9236 (v0.5.11)
repository: https://github.com/markdoc/markdoc

| here | upstream path | sha256 |
|---|---|---|
| `tests.yaml` | `spec/marktest/tests.yaml` | `549762cc080c9723c0d2495290de60ede053a0cfe7f46686b385bd1c1cd70b7b` |
| `index.ts` | `spec/marktest/index.ts` | `7ee653280cbfa856224ba59fcc6614e0d53d78a2120f55820a528f9c9a3ff949` |
| `react-shim.ts` | `spec/marktest/react-shim.ts` | `09aba6bb0e1ee5cbe4c7d3b267a09a225f0935f9485fd338f50b5305a6e1a309` |
| `react.ts` | `src/renderers/react/react.ts` | `043b9c24972e0c1b4d9644a401d608c71146fa7dd8ef0c54432b98d7241ee73f` |
| `LICENSE` | `LICENSE` | `22d695fb4d9a771b4883f2fc5e90dab07dbc32024869d7463339a88faba6d648` |

`tests.yaml` is the corpus: 105 cases, each a Markdoc snippet and what
upstream renders from it. `index.ts` is upstream's runner, and it is the
grading rule: `../conformance.rs` mirrors it and cites its line numbers.
`react.ts` and `react-shim.ts` are the renderer and shim the runner puts a
tree through, which `../conformance/react.rs` reproduces. `LICENSE` is
Stripe's MIT licence, which has to travel with any copy of these files.

## Never edited

Changing the corpus to match the implementation destroys the only measure
the port has, so these files are never edited, not even to fix a typo
upstream would also have. Two checks hold that:

- `../conformance.rs`, on every `cargo test` and with no network, checks
  every file against its hash above, that the table lists exactly the files
  here, and that the `tests.yaml` row carries the hash it pins itself;
- `verify-vendored.sh` holds the table to the same shape, then fetches every
  file at the revision above and compares it with the copy here, byte for
  byte. CI runs it before the tests, so the hashes above cannot be edited to
  match an edited file either.

`.gitattributes` keeps git from converting their line endings on checkout,
which would change the bytes on Windows and fail the first check.

## Refreshing to a newer revision

A refresh is a wholesale replacement, never a partial one:

1. Copy all five files from the new revision, unmodified.
2. Update the revision line and every hash above, and `REVISION` and
   `CORPUS_SHA256` in `../conformance.rs`; `verify-vendored.sh` must pass.
3. Run `MARKDOC_CONFORMANCE_BLESS=rebuild cargo test --test conformance`.
   That writes a fresh baseline. A failing case keeps its reason if the same
   case — same position, same name — failed before; every other failing
   case is `UNDECLARED`, and the test fails until each one says why. Check
   the kept reasons still describe the new revision's cases. A difference
   from upstream is declared, never discovered.

`rebuild` refuses while the baseline already records verdicts for the
vendored corpus, whatever its revision line says, so it cannot be used to
wave a regression through.
