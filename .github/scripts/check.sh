#!/usr/bin/env bash
#
# Everything that has to be true before markdoc is published.
#
# ONE COPY: ci.yml runs it on every push and pull request, release.yml runs
# it before anything is published. A second list of steps drifts, and the
# half that drifts is always the one nobody watches.
#
# Inputs:
#   $1          the markdoc checkout                   default: $PWD
#   STRICT      1 to fail on clippy warnings           default: 1
set -euo pipefail

ROOT="${1:-$PWD}"
cd "$ROOT"
ROOT="$PWD"
STRICT="${STRICT:-1}"

group() { echo "::group::$*"; }
endgroup() { echo "::endgroup::"; }
die() { echo "::error::$*" >&2; exit 1; }

[ -f Cargo.toml ] || die "no Cargo.toml in $ROOT — point this at the markdoc checkout"

failed=""
step() {
  local name="$1"; shift
  group "$name"
  if "$@"; then
    endgroup
  else
    endgroup
    echo "::error::$name failed"
    failed="$failed $name"
  fi
}

step "cargo fmt" cargo fmt --all --check

# Cargo.lock is committed, and has to agree with Cargo.toml: the release
# publishes with `cargo publish --locked`, which refuses one that does not,
# and finding that out here costs a push rather than a release.
lock_is_current() { cargo metadata --locked --format-version 1 >/dev/null; }
step "Cargo.lock is current" lock_is_current

if [ "$STRICT" = "1" ]; then
  step "cargo clippy" cargo clippy --all-targets -- -D warnings
else
  step "cargo clippy" cargo clippy --all-targets
fi

# Before the tests, so an edit to the corpus that also edits the hash the
# conformance test checks it against fails here, by name.
step "the vendored corpus is upstream's" tests/marktest/verify-vendored.sh

# Includes tests/conformance.rs: upstream Markdoc's own corpus, held as a
# ratchet against tests/conformance-baseline.txt.
step "cargo test" cargo test --all-targets

# Doctests are not --all-targets.
step "cargo test --doc" cargo test --doc

doc_is_clean() { RUSTDOCFLAGS="-D warnings" cargo doc --no-deps; }
step "cargo doc" doc_is_clean

# The release profile is what a consumer builds against, and it differs from
# debug in more than speed: an overflow that panics in debug wraps in release.
step "cargo build --release" cargo build --release

for s in .github/scripts/*.sh; do
  step "bash -n $(basename "$s")" bash -n "$s"
done

if [ -n "$failed" ]; then
  die "failed:$failed"
fi
echo "==> all checks passed"
