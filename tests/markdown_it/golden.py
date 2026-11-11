"""Golden token streams for the markdown-it port's node-free test, tests/markdown_it_golden.rs.

Runs every document of each input set (spec.jsonl, the CommonMark spec examples, and
regressions.jsonl, the cases the fuzzer found) through the node oracle, oracle_tokens.cjs,
in each configuration of fuzz.CONFIGS and in both modes (parse, parseInline), and writes
golden/<set>.txt. The cargo test runs the same requests through the Rust port and compares.
An input line is {id, src} and may carry `options`, which are merged over the options of
every configuration (the line's keys win), for cases that need an unusual option.

Usage (run python with -I, from the crate root):
  python3 -I tests/markdown_it/golden.py --markdoc DIST/index.js           # regenerate
  python3 -I tests/markdown_it/golden.py --markdoc DIST/index.js --check   # exit 1 if stale

DIST/index.js is upstream @markdoc/markdoc 0.5.11's dist/index.js.

Format of golden/<set>.txt (this paragraph is also written at the top of each file):
  # config NAME REQUEST_JSON      one per configuration: the request fields it adds
  === ID MODE NAME...             the output of document ID in MODE under the configurations
                                  named, which all agree; every (ID, MODE, NAME) appears once
  TOKEN LINE...                   one line per token, children below their parent, indented
                                  two spaces per level
  env JSON                        env after the parse, when it has keys
  error JSON                      instead of tokens: JS threw (the message); Rust must throw it
A token line is its type, then only the fields that are not empty/null/0/false, in this
order: tag=, attrs=, map=A,B, nesting=, level=, content=, markup=, info=, meta=, block,
hidden, errors=, position=, children=N (N children follow; present whenever children is an
array, even empty). type and tag are written bare when they are [A-Za-z0-9_]+, else as JSON;
all JSON is compact, with sorted keys and only ", \\ and control characters escaped.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True  # no __pycache__ in the crate
from fuzz import CONFIGS  # noqa: E402

INPUTS = ["spec", "regressions"]
MODES = ["parse", "parseInline"]
FORMAT = __doc__[__doc__.index("Format of"):].strip()


def js(v):
    return json.dumps(v, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def bare(s):
    return s if re.fullmatch(r"[A-Za-z0-9_]+", s) else js(s)


def token_lines(t, depth, out):
    f = [bare(t.get("type", ""))]
    if "tag" in t:
        f.append("tag=" + bare(t["tag"]))
    if "attrs" in t:
        f.append("attrs=" + js(t["attrs"]))
    if "map" in t:
        f.append("map=%d,%d" % tuple(t["map"]))
    for k in ("nesting", "level"):
        if t.get(k, 0):
            f.append("%s=%d" % (k, t[k]))
    for k in ("content", "markup", "info", "meta"):
        if k in t:
            f.append(k + "=" + js(t[k]))
    for k in ("block", "hidden"):
        if t.get(k):
            f.append(k)
    for k in ("errors", "position"):
        if k in t:
            f.append(k + "=" + js(t[k]))
    if "children" in t:
        f.append("children=%d" % len(t["children"]))
    out.append("  " * depth + " ".join(f))
    for c in t.get("children", []):
        token_lines(c, depth + 1, out)


def body(result):
    out = []
    if "error" in result:
        out.append("error " + js(result["error"]))
        return out
    for t in result["tokens"]:
        token_lines(t, 0, out)
    if "env" in result:
        out.append("env " + js(result["env"]))
    return out


def golden(markdoc, name):
    with open(os.path.join(HERE, name + ".jsonl"), encoding="utf-8") as f:
        docs = [json.loads(line) for line in f if line.strip()]
    reqs = []
    for doc in docs:
        for mode in MODES:
            for config, fields in CONFIGS.items():
                req = dict(fields)
                if "options" in doc:
                    req["options"] = {**fields.get("options", {}), **doc["options"]}
                req.update({"id": "\t".join((doc["id"], mode, config)), "src": doc["src"]})
                if mode != "parse":
                    req["mode"] = mode
                reqs.append(req)
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "requests.jsonl")
        with open(path, "w", encoding="utf-8") as f:
            for req in reqs:
                f.write(json.dumps(req, ensure_ascii=False) + "\n")
        with open(path, "rb") as f:
            proc = subprocess.run(["node", os.path.join(HERE, "oracle_tokens.cjs"), markdoc],
                                  stdin=f, capture_output=True, check=True)
    results = {}
    for line in proc.stdout.decode("utf-8").splitlines():
        r = json.loads(line)
        results[r["id"]] = r
    lines = ["# " + line for line in FORMAT.splitlines()]
    lines.append("# Generated by: python3 -I tests/markdown_it/golden.py --markdoc DIST/index.js")
    for config, fields in CONFIGS.items():
        lines.append(f"# config {config} {js(fields)}")
    for doc in docs:
        for mode in MODES:
            groups = {}
            for config in CONFIGS:
                b = "\n".join(body(results["\t".join((doc["id"], mode, config))]))
                groups.setdefault(b, []).append(config)
            for b, configs in groups.items():
                lines.append(f"=== {doc['id']} {mode} {' '.join(configs)}")
                if b:
                    lines.append(b)
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--markdoc", required=True, help="upstream @markdoc/markdoc 0.5.11 dist/index.js")
    ap.add_argument("--check", action="store_true", help="exit 1 if a golden file is stale")
    args = ap.parse_args()
    stale = 0
    os.makedirs(os.path.join(HERE, "golden"), exist_ok=True)
    for name in INPUTS:
        text = golden(args.markdoc, name)
        path = os.path.join(HERE, "golden", name + ".txt")
        old = open(path, encoding="utf-8").read() if os.path.exists(path) else None
        if args.check:
            if old != text:
                print(f"{path}: stale", file=sys.stderr)
                stale += 1
        elif old != text:
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            print(f"wrote {path} ({len(text.encode())} bytes)")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main())
