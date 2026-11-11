# regress, vendored

This directory is [regress](https://github.com/ridiculousfish/regress) 0.12.0, the
ECMAScript regular-expression engine `markdoc::next` matches a schema's `matches` RegExp
with, carried with the fixes below until upstream accepts them.

- Source: the crates.io package `regress-0.12.0` (checksum
  `32eef8b209c3c1c15dbad02c1f30f9539f00dc7253e0cbcdaae442a50a09d7c1`), which is the
  repository at commit `aae35dee1596cc9a9caaad2425b088081c09e5d5` (its `src/` and `tests/`
  are identical). Kept: `src/`, `tests/`, `Cargo.toml` (the package's normalised manifest,
  changed for publishing as below), `Cargo.toml.orig` (the repository's), `LICENSE-APACHE`, `LICENSE-MIT`, `README.md`,
  `perf.md`, `rustfmt.toml`, `.gitignore`, and, from the repository at that commit,
  `gen-unicode/` (the generator of `src/unicodetables.rs`; nothing builds it here, and its
  dependencies are not in this workspace's registry). Left out: `.github/`, `regress-tool/`,
  the package's `Cargo.lock` and the registry's bookkeeping files. regress is licensed MIT OR
  Apache-2.0; both licence files are kept unchanged. Paths here are the repository's, so the
  patch files apply to either.
- Published to crates.io as `markdoc-regress`, because the published `markdoc` gets its
  dependencies from there (packaging drops `path`, and `[patch]` does not travel). Its
  `Cargo.toml` differs from the package's in `name`, `description`, `readme` (this file) and
  `repository`, and in `exclude = ["Cargo.toml.orig"]` (a name `cargo package` reserves).
  The library is still `regress`.
- `markdoc`'s `Cargo.toml` takes it as `regress = { package = "markdoc-regress", version =
  "0.12.0", path = "vendor/regress", features = ["utf16"] }`. It is published before
  `markdoc`: `cargo publish --manifest-path vendor/regress/Cargo.toml`. A change here is
  published as a new version of `markdoc-regress`, which `markdoc` must then require.
- Its own test suite runs on its own:
  `cargo test --manifest-path vendor/regress/Cargo.toml --features utf16` (use a separate
  `CARGO_TARGET_DIR`).

## Changes

Each change is one commit touching only this directory, with tests in regress's own style
(`tests/tests.rs`, `src/*` unit tests). The ones marked *for upstream* are spec bugs: each has
a patch file against regress 0.12.0 and a pull-request text in the markdoc workspace
(`findings/conformance/work/regex/patches/000N-*.patch`, `UPSTREAM-PR.md`). The ones marked
*not for upstream* are V8's departures from ECMAScript, which markdoc must match; they change
nothing unless asked for (`Flags::v8_compat`, `Regex::try_find_from_utf16`).

For upstream (ECMAScript, as V8 answers too):

1. **Canonicalize without `u` or `v`** (0001): `TO_UPPERCASE` holds ECMAScript's
   Canonicalize (no non-ASCII character mapped to ASCII, no character whose full uppercase
   is longer than one), regenerated from `gen-unicode`'s patched rules; brackets without the
   unicode flag close over it. `/\u017F/i` no longer matches `s`, nor `/[a-z]/i` U+212A.
2. **`v` alone is a unicode mode** (0002): `/k/iv` matches U+212A, `/\a/v` is refused.
3. **`v` with `i` folds each class operand before the set operations** (0003): `/[\P{Lu}]/iv`
   no longer matches `a`; `\q{}` strings and properties of strings fold.
4. **`\W` in a bracket with `iu`** (0004) excludes U+017F and U+212A.
5. **A UTF-16 backreference does not end inside a surrogate pair** (0005).
6. **Empty `\q{}` strings** (0006) are kept, and tried after the single characters.
7. **One-character `\q{}` strings are characters; MayContainStrings** (0007): a negated class
   complements them, and is valid or not by MayContainStrings.
8. **Properties of strings: one-code-point members are characters** (0008): matched as a
   set instead of one alternative each (`\p{RGI_Emoji}` has about 3,800); same answers,
   much faster.

Not for upstream (V8 12.4, node 22, where it departs from the specification):

9. **`Flags::v8_compat`: code-unit search steps.** V8's search tries a match at every code
   unit, also between the halves of a surrogate pair, where it reads no character in either
   direction but assertions see the halves: only an empty match can start there, and does
   (`/\B/gu` on `a\u{1F600}xab` at 2; ECMAScript, 4). `Utf16Input::with_v8_steps`.
10. **`Flags::v8_compat`: `v` classes with `i`.** V8 does not fold the property `ASCII`
   (`/\p{ASCII}/iv` does not match U+017F, `/\P{ASCII}/iv` does) and makes a one-character
   `\q{}` string its simple case folding alone (`/[\q{K}]/iv` matches `k`, not `K`).
11. **`Regex::try_find_from_utf16`: a backtrack limit, and sticky matching.** V8's backtrack
    stack holds 64 MiB and a match past it throws `RangeError`; regress's grows for as long
    as memory lasts. This search stops with `Err` past a given number of entries (counted as
    loops iterate, lookarounds included), and with `sticky` tries the start position alone.
12. **`Flags::v8_compat`: case-insensitive backreferences with `u` or `v` in a subject with a
    code unit past U+00FF** compare as V8 does there, by ICU's `caseCompare` (full case
    folding, one level, only where the units differ): `/(\u{FB00}f)\1/iu` matches
    `\u{FB00}ff\u{FB00}`. `src/v8compat/` ports ICU's `_cmpFold`; its full-folding table is
    generated from Unicode 17.0's `CaseFolding.txt` by
    `findings/conformance/work/regex/tables/gen_full_folding.py`.
13. **`Flags::v8_compat`: the first operand of a `v`-mode intersection or subtraction with
    `i`.** V8 leaves a single character in that position unfolded: `/[a&&a]/iv` does not
    match `A`, while `/[[a]&&a]/iv` does (95547fd; test 93e3519).

The Unicode tables (`src/unicodetables.rs`, Unicode 17.0 as published) are checked against
node 22 (ICU 78.2, Unicode 17.0) for every code point by
`findings/conformance/work/regex/tables/gen_scans.cjs` through markdoc's front end.
