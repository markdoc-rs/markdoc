"""Write tests/grammar/golden.jsonl, the node-free regression set of tests/grammar_golden.rs.

Each line is {"id", "content", "line"}: a request and the exact line the oracle
(oracle_grammar.cjs: upstream's tag.js) prints for it. The requests are upstream's own
cases (tag.test.ts, and every tag in tests/marktest/tests.yaml), hand-written probes of the
grammar's corners, and generated cases (fuzz.py) chosen so that each distinct shape of result
and each distinct set of expectations in an error appears at least once.

Usage (run python with -I):
  python3 -I tests/grammar/golden.py --tagjs TAG_JS --markdoc DIST/index.js --tag-test TAG_TEST_TS
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True

import extract_tags  # noqa: E402
import fuzz  # noqa: E402

PROBES = [
    "foo", "foo /", "/foo", "foo/", "/", "/ foo", "", " ", "\n", "foo ", " foo", "foo\n\n$",
    "foo 0", "foo -0", "foo \"\"", "foo false", "foo null", "foo 1 x=2", "foo \"x\"  y=1",
    "foo x=nullx", "foo x=null()", "foo bar", "foo bar(", "foo x=f(a=1, 2)", "foo x=f(1, 0=5)",
    "foo x=f(, 1)", "foo x=f(1,)", "foo x=f( 1 , b = 2 )", "foo x=f(g(h()))",
    "foo x={b: 1, 2: 2, a: 3, 1: 4}", "foo x={4294967294: 1, 4294967295: 2, a: 3, 01: 4, \"-0\": 5, 0: 6}",
    "foo x={a: 1, __proto__: 2}", "foo x={__proto__: 1, a: 2}", "foo x={__proto__: 1, a: 2, __proto__: 3}",
    "foo x={a: 1, __proto__: null, __proto__: 5}", "foo x={x: 1, __proto__: {a: 1, __proto__: null}, __proto__: 3}",
    "foo x={x: 1, __proto__: {__proto__: 1}, __proto__: 3}", "foo x={a: 1, __proto__: [1,2], length: 3}",
    "foo x=f(__proto__=1)", "foo x=f(__proto__=null, __proto__=1)", "foo x={\"$$mdtype\": 1, a: 2}",
    "foo x={$$mdtype: 1}", "foo x={\"\": 1}", "foo x={a:1,}", "foo x={,}", "foo x=[1,2,]", "foo x=[,]",
    "foo x=[ ]", "foo x={ }", "foo x=1e5", "foo x=.5", "foo x=1.", "foo x=-", "foo x=007",
    "foo x=0.000001", "foo x=0.0000001", "foo x=1" + "0" * 400, "foo x=9041039839299.8125",
    "foo x=1152921504606846976", "foo x=123456789012345678901", "foo x=1000000000000000000000",
    "$x", "$x.y[0][\"z\"]", "@x.y[0][\"z\"]", "$x[asdf]", "$x[-0]", "$x[1.5]", "$x.", "$x[1", "$",
    "foo x=\"a\\u0041\"", "foo x=\"\\\"\\\\\\n\\r\\t\"", "foo x=\"\x00\"", "foo \x01", "foo\x7f",
    "foo 😀", "foo x=\"é😀\"", "é", "foo\r\nbar", "foo x=1", "foo\tx=1\ny=2",
    "#id .class key=value", "#id.class", ".a .b", "#a #b", "#", ".", "#-", "if $a.b[0][\"c d\"] .x #y z=fn(1, a=$b, \"q\")",
    "partial file=\"x.md\" variables={a: 1} /", "table .numbered #t1", "%}", "foo \"%}\"",
]


def oracle(requests, tagjs, markdoc):
    with tempfile.TemporaryDirectory(prefix="grammar-golden-") as work:
        req = os.path.join(work, "requests.jsonl")
        with open(req, "w", encoding="utf-8") as f:
            for r in requests:
                f.write(json.dumps(r) + "\n")
        out = subprocess.run(["node", os.path.join(HERE, "oracle_grammar.cjs"), tagjs, markdoc],
                             stdin=open(req, "rb"), capture_output=True, check=True).stdout
    return [line for line in out.decode("utf-8").split("\n") if line]


def signature(line):
    r = json.loads(line)
    if "error" in r:
        e = r["error"]
        return ("error", tuple(sorted({json.dumps(x, sort_keys=True) for x in e["expected"]})),
                e["found"] is None)

    def shape(v):
        if isinstance(v, dict):
            if "$$mdtype" in v:
                return v["$$mdtype"]
            return "{" + ",".join(sorted({shape(x) for x in v.values()})) + "}"
        if isinstance(v, list):
            return "[" + ",".join(sorted({shape(x) for x in v})) + "]"
        return type(v).__name__
    res = r["result"]
    attrs = res["meta"].get("attributes") or []
    return ("result", res["type"], tuple(sorted({a["type"] + ":" + shape(a["value"]) for a in attrs})),
            shape(res["meta"].get("variable")))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tagjs", required=True)
    ap.add_argument("--markdoc", required=True)
    ap.add_argument("--tag-test", required=True, help="upstream's src/grammar/tag.test.ts")
    ap.add_argument("--fuzz-count", type=int, default=40000)
    ap.add_argument("--max-generated", type=int, default=400)
    args = ap.parse_args()

    requests = []
    with open(args.tag_test, encoding="utf-8") as f:
        requests += [{"id": f"tag.test.ts-{i}", "content": c}
                     for i, c in enumerate(extract_tags.tag_test_literals(f.read()))]
    import yaml  # PyYAML
    with open(os.path.join(CRATE, "tests", "marktest", "tests.yaml"), encoding="utf-8") as f:
        cases = yaml.safe_load(f)
    seen = set()
    for n, case in enumerate(cases):
        for s in extract_tags.strings_in(case):
            for t in extract_tags.tags(s):
                for c in (extract_tags.js_trim(t), t):
                    if c not in seen:
                        seen.add(c)
                        requests.append({"id": f"marktest-{len(seen) - 1}", "content": c})
    requests += [{"id": f"probe-{i}", "content": c} for i, c in enumerate(PROBES)]

    import random
    g = fuzz.Gen(random.Random(20261109))
    generated = [{"id": f"fuzz-{i}", "content": g.item()} for i in range(args.fuzz_count)]
    numbers = [{"id": f"number-{i}", "content": g.number_item()} for i in range(2000)]
    lines = oracle(generated + numbers, args.tagjs, args.markdoc)
    picked, signatures = [], {}
    for r, line in zip(generated + numbers, lines):
        sig = signature(line) if r["id"].startswith("fuzz") else ("number", len(picked) % 7 == 0)
        if r["id"].startswith("number"):
            if sum(1 for p in picked if p["id"].startswith("number")) < 40:
                picked.append(r)
            continue
        if signatures.get(sig, 0) < 1 and len(signatures) < args.max_generated:
            signatures[sig] = signatures.get(sig, 0) + 1
            picked.append(r)
    requests += picked

    lines = oracle(requests, args.tagjs, args.markdoc)
    out_path = os.path.join(HERE, "golden.jsonl")
    with open(out_path, "w", encoding="utf-8") as f:
        for r, line in zip(requests, lines):
            assert json.loads(line)["id"] == r["id"]
            f.write(json.dumps({"id": r["id"], "content": r["content"], "line": line}, ensure_ascii=False) + "\n")
    print(f"{out_path}: {len(requests)} cases ({len(picked)} generated)", file=sys.stderr)


if __name__ == "__main__":
    main()
