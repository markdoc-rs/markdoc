"""Compare the tag grammar port with upstream's generated parser, request by request.

Runs tests/grammar/oracle_grammar.cjs (node: upstream Markdoc 0.5.11's src/grammar/tag.js
with the published package's Ast.Variable and Ast.Function) and examples/grammar_parse.rs
(the Rust port, markdoc::grammar) over the same JSONL requests {id, content}, and compares
their output lines. Both sides print JSON.stringify's text, so the lines must be identical,
byte for byte; a line that differs is parsed to name the first field that differs (result,
or the error's message, expected, found, location). Where tag.js throws something else than
its SyntaxError (RangeError when values nest past V8's stack) and the port panics, both print
{id, crash} with their own messages, which counts as agreement. Exits 1 on any difference.

Usage (run python with -I):
  python3 -I tests/grammar/compare.py INPUTS.jsonl --tagjs TAG_JS --markdoc DIST/index.js
      [--rust-bin PATH] [--node PATH] [--show N] [--keep DIR] [--label NAME]

Without --rust-bin the example is built and run with `cargo run --release` (honours
CARGO_TARGET_DIR).
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(os.path.dirname(HERE))


def run(cmd, stdin_path, stdout_path, cwd=None):
    start = time.monotonic()
    with open(stdin_path, "rb") as fin, open(stdout_path, "wb") as fout:
        proc = subprocess.run(cmd, stdin=fin, stdout=fout, cwd=cwd)
    if proc.returncode != 0:
        sys.exit(f"{' '.join(cmd)} exited with {proc.returncode}")
    return time.monotonic() - start


def load(path):
    """id -> raw output line (without the newline)."""
    out = {}
    with open(path, encoding="utf-8", errors="surrogatepass") as f:
        for line in f:
            line = line.rstrip("\n")
            if line:
                out[json.loads(line)["id"]] = line
    return out


def first_difference(a, b, path):
    """The first path where two JSON values differ (types included), or None."""
    if isinstance(a, dict) and isinstance(b, dict):
        if list(a) != list(b):
            return (f"{path} keys", list(a), list(b))
        for k in a:
            d = first_difference(a[k], b[k], f"{path}.{k}")
            if d:
                return d
        return None
    if isinstance(a, list) and isinstance(b, list):
        for i, (x, y) in enumerate(zip(a, b)):
            d = first_difference(x, y, f"{path}[{i}]")
            if d:
                return d
        if len(a) != len(b):
            return (f"{path}.length", len(a), len(b))
        return None
    if type(a) is type(b) and a == b:
        return None
    return (path, a, b)


def classify(js_line, rs_line):
    """(category, detail) for two differing lines."""
    js, rs = json.loads(js_line), json.loads(rs_line)
    kind_js = "result" if "result" in js else "error" if "error" in js else "crash"
    kind_rs = "result" if "result" in rs else "error" if "error" in rs else "crash"
    if kind_js != kind_rs:
        return f"{kind_js} vs {kind_rs}", None
    if kind_js == "result":
        d = first_difference(js["result"], rs["result"], "result")
        return "result", d or ("text only", js_line, rs_line)
    if kind_js == "error":
        for field in ("message", "expected", "found", "location"):
            d = first_difference(js["error"].get(field), rs["error"].get(field), f"error.{field}")
            if d:
                return f"error.{field}", d
        d = first_difference(js["error"], rs["error"], "error")
        return "error", d or ("text only", js_line, rs_line)
    return "crash", (js.get("crash"), rs.get("crash"))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("inputs")
    ap.add_argument("--tagjs", required=True, help="upstream Markdoc 0.5.11 src/grammar/tag.js")
    ap.add_argument("--markdoc", required=True, help="upstream @markdoc/markdoc 0.5.11 dist/index.js")
    ap.add_argument("--rust-bin", help="a built grammar_parse example (default: cargo run --release)")
    ap.add_argument("--node", default="node")
    ap.add_argument("--show", type=int, default=10, help="differences to print")
    ap.add_argument("--keep", help="directory to keep both outputs in")
    ap.add_argument("--label", help="name of the input set in the tally line")
    args = ap.parse_args()

    work = args.keep or tempfile.mkdtemp(prefix="grammar-compare-")
    os.makedirs(work, exist_ok=True)
    js_out = os.path.join(work, "node.jsonl")
    rs_out = os.path.join(work, "rust.jsonl")

    t_js = run([args.node, os.path.join(HERE, "oracle_grammar.cjs"), args.tagjs, args.markdoc],
               args.inputs, js_out)
    if args.rust_bin:
        rust_cmd = [args.rust_bin]
    else:
        rust_cmd = ["cargo", "run", "--quiet", "--release", "--example", "grammar_parse"]
    t_rs = run(rust_cmd, args.inputs, rs_out, cwd=CRATE)

    ids = []
    with open(args.inputs, encoding="utf-8") as f:
        for line in f:
            if line.strip():
                ids.append(json.loads(line)["id"])
    js, rs = load(js_out), load(rs_out)

    tally = {"result": 0, "error": 0, "crash": 0}
    categories = {}
    shown = 0
    missing = 0
    for i in ids:
        a, b = js.get(i), rs.get(i)
        if a is None or b is None:
            missing += 1
            continue
        r = json.loads(a)
        tally["result" if "result" in r else "error" if "error" in r else "crash"] += 1
        if a == b:
            continue
        if "crash" in r and "crash" in json.loads(b):
            continue  # both threw something else than a SyntaxError: agreement
        cat, detail = classify(a, b)
        categories[cat] = categories.get(cat, 0) + 1
        if shown < args.show:
            shown += 1
            print(f"--- {i}: {cat}")
            print(f"    node: {a[:400]}")
            print(f"    rust: {b[:400]}")
            if detail:
                print(f"    first difference: {detail!r}"[:600])

    differ = sum(categories.values())
    label = args.label or os.path.basename(args.inputs)
    print(f"{label}: inputs {len(ids)}  results {tally['result']}  errors {tally['error']}"
          f"  crashes {tally['crash']}  missing {missing}  differ {differ}"
          + ("".join(f"  [{k}: {v}]" for k, v in sorted(categories.items())))
          + f"  (node {t_js:.1f}s, rust {t_rs:.1f}s)")
    if not args.keep:
        for p in (js_out, rs_out):
            os.remove(p)
        os.rmdir(work)
    sys.exit(1 if differ or missing or len(js) != len(ids) or len(rs) != len(ids) else 0)


if __name__ == "__main__":
    main()
