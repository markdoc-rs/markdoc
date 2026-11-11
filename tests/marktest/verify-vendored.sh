#!/usr/bin/env bash
# Prove the vendored files are upstream's: fetch each at the pinned revision
# and compare it with the copy here, byte for byte.
#
# Before fetching anything, the table in UPSTREAM.md is held to its shape: a
# file row that does not parse fails rather than being skipped, the rows name
# exactly the vendored files present (no more, no fewer), and the corpus row's
# hash is the one tests/conformance.rs pins. Otherwise a row could be bent
# out of the parser's sight and the script would pass having checked nothing.
#
# Usage: tests/marktest/verify-vendored.sh   (from anywhere)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
doc="$here/UPSTREAM.md"
fail() { echo "verify-vendored: $*" >&2; exit 1; }

rev="$(sed -n 's/^revision: \([0-9a-f]\{40\}\) .*/\1/p' "$doc")"
[ "$(printf '%s\n' "$rev" | grep -c .)" = 1 ] || fail "UPSTREAM.md needs exactly one 'revision: <40-hex> ...' line"

row='^| `\([^`]*\)` | `\([^`]*\)` | `\([0-9a-f]\{64\}\)` |$'
rows="$(grep '^| `' "$doc" | tr -d '\r')"
[ -n "$rows" ] || fail "UPSTREAM.md lists no vendored files"
bad="$(printf '%s\n' "$rows" | grep -v "$row" || true)"
[ -z "$bad" ] || fail "malformed file row(s) in UPSTREAM.md:"$'\n'"$bad"
table="$(printf '%s\n' "$rows" | sed "s/$row/\1 \2 \3/")"

listed="$(printf '%s\n' "$table" | cut -d' ' -f1 | sort)"
[ -z "$(printf '%s\n' "$listed" | uniq -d)" ] || fail "UPSTREAM.md lists a file twice"
present="$(cd "$here" && ls -A | grep -v -x -e UPSTREAM.md -e verify-vendored.sh | sort)"
[ "$listed" = "$present" ] || fail "UPSTREAM.md lists [$(echo $listed)] but tests/marktest holds [$(echo $present)]"

pinned="$(sed -n 's/^const CORPUS_SHA256: &str = "\([0-9a-f]\{64\}\)";$/\1/p' "$here/../conformance.rs")"
[ -n "$pinned" ] || fail "no CORPUS_SHA256 in tests/conformance.rs"
corpus="$(printf '%s\n' "$table" | awk '$1 == "tests.yaml" {print $3}')"
[ "$corpus" = "$pinned" ] || fail "UPSTREAM.md records tests.yaml as $corpus; tests/conformance.rs pins $pinned"

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
status=0 checked=0
while read -r local upstream sha; do
  url="https://raw.githubusercontent.com/markdoc/markdoc/$rev/$upstream"
  curl -fsSL "$url" -o "$tmp/upstream"
  have="$(sha256sum "$here/$local" | cut -d' ' -f1)"
  if ! cmp -s "$tmp/upstream" "$here/$local"; then
    echo "DIFFERS  $local  from  $url" >&2; status=1
  elif [ "$have" != "$sha" ]; then
    echo "HASH     $local  is upstream's, but UPSTREAM.md records $sha, not $have" >&2; status=1
  else
    echo "ok       $local"
  fi
  checked=$((checked + 1))
done <<< "$table"
[ "$checked" -eq "$(printf '%s\n' "$present" | grep -c .)" ] || fail "checked $checked file(s), expected one per vendored file"
exit "$status"
