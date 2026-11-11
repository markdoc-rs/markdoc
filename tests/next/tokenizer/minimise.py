"""Minimise the documents fuzz.py found differing: the smallest document that still differs.

Delta debugging over the document's lines, then its characters: each round sends every
candidate (one chunk removed) through both sides at once (tests/next/oracle.cjs and the
port, as fuzz.py compares them) and keeps the first that still differs, halving the chunk
size when none does. Writes one {id, code, tokenizer, where, oracle, port} line per input
difference, deduplicated by minimised code.

Usage (python3 -I):
  python3 -I tests/next/tokenizer/minimise.py DIFFERENCES.jsonl --markdoc DIST/index.js \
      --rust-bin NEXT_COMPARE --out MINIMISED.jsonl
"""

import argparse
import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ beside the scripts
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fuzz  # noqa: E402


def differs(docs, args):
    """For each doc, its difference (as fuzz.compare reports it), or None."""
    _, diffs = fuzz.compare(docs, args.markdoc, args.rust_bin)
    by_id = {d["id"]: d for d in diffs}
    return [by_id.get(d["id"]) for d in docs]


def signature(diff):
    """What kind of difference: a smaller document must keep it (the same message thrown by
    each side, or a difference in the tokens), so one bug is not traded for another."""
    if diff["where"] == "threw":
        port = str(diff["port"])
        return ("threw", diff["oracle"], port.split(":")[0])
    return ("tokens",)


def shrink(case, args, units):
    """Remove chunks of `units` (lines or characters) while the document still differs."""
    parts = units(case["code"])
    n = max(1, len(parts) // 2)
    while n >= 1:
        candidates = []
        for start in range(0, len(parts), n):
            rest = parts[:start] + parts[start + n:]
            candidates.append(rest)
        docs = [{"id": f"c{i}", "code": "".join(c), "tokenizer": case["tokenizer"]}
                for i, c in enumerate(candidates)]
        found = None
        for c, d in zip(candidates, differs(docs, args)):
            if d is not None and signature(d) == signature(case):
                found = (c, d)
                break
        if found:
            parts = found[0]
            case = {**case, "code": "".join(parts), "where": found[1]["where"],
                    "oracle": found[1]["oracle"], "port": found[1]["port"]}
            n = min(n, max(1, len(parts) // 2))
        else:
            n //= 2
    return case


def lines(code):
    return code.splitlines(keepends=True)


def chars(code):
    return list(code)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("differences")
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"))
    ap.add_argument("--rust-bin", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--limit", type=int, default=50)
    args = ap.parse_args()
    with open(args.differences, encoding="utf-8", errors="surrogatepass") as f:
        cases = [json.loads(l) for l in f if l.strip()][: args.limit]
    seen = {}
    for case in cases:
        case = {k: case[k] for k in ("id", "code", "tokenizer", "where", "oracle", "port")}
        if differs([case], args)[0] is None:
            print(f"{case['id']}: no longer differs")
            continue
        case = shrink(case, args, lines)
        case = shrink(case, args, chars)
        print(f"{case['id']}: {json.dumps(case['code'])} -> {case['where']}: "
              f"oracle {json.dumps(case['oracle'])[:100]} | port {json.dumps(case['port'])[:100]}")
        seen.setdefault(case["code"], case)
    with open(args.out, "w", encoding="utf-8") as f:
        for case in seen.values():
            f.write(json.dumps(case, ensure_ascii=False) + "\n")
    print(f"{len(cases)} differences, {len(seen)} distinct minimised documents -> {args.out}")


if __name__ == "__main__":
    main()
