"""Compare the markdown-it port's token streams with the JavaScript oracle's.

Runs tests/markdown_it/oracle_tokens.cjs (node, upstream Markdoc's bundled markdown-it) and
examples/mdit_tokens.rs (the Rust port) over the same requests, then reports, per document,
the first token path where they differ, with counts. Exits 1 on any difference.

Each input line is {id, src, ...}; the request sent to both sides is the template plus the
line's id and src (request keys present on the line itself win over the template).

Usage (run python with -I):
  python3 -I tests/markdown_it/compare.py INPUTS.jsonl --markdoc DIST/index.js
      [--template JSON] [--preset NAME] [--mode parse|parseInline] [--options JSON]
      [--disable a,b] [--enable a,b] [--disable-unported]
      [--rust-bin PATH] [--node PATH] [--id-prefix P] [--limit N] [--show N] [--keep DIR]

--disable-unported adds every rule whose Rust file is still a stub (a `// PORT:` body under
src/markdown_it/rules_*) to the disable list of both sides.
Without --rust-bin, the example is built and run with `cargo run --release` (honours
CARGO_TARGET_DIR).
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(os.path.dirname(HERE))
REQUEST_KEYS = ("preset", "options", "enable", "disable", "mode", "env")
FIELDS = ("type", "tag", "attrs", "map", "nesting", "level", "content", "markup", "info",
          "meta", "block", "hidden", "errors", "position", "children")


def unported_rules():
    """Rule names whose src/markdown_it/rules_*/<name>.rs body is still a `// PORT:` stub."""
    names = []
    for chain in ("rules_core", "rules_block", "rules_inline"):
        folder = os.path.join(CRATE, "src", "markdown_it", chain)
        for file in sorted(os.listdir(folder)):
            stem = file[:-3]
            if not file.endswith(".rs") or stem in ("mod",) or stem.startswith("state_"):
                continue
            with open(os.path.join(folder, file), encoding="utf-8") as f:
                if "// PORT:" in f.read() and stem not in names:
                    names.append(stem)
    return names


def first_difference(a, b, path):
    """The first path where two plain() values differ, with both values, or None."""
    if isinstance(a, dict) and isinstance(b, dict):
        keys = [k for k in FIELDS if k in a or k in b] + sorted((set(a) | set(b)) - set(FIELDS))
        for k in keys:
            if k not in a or k not in b:
                return (f"{path}.{k}", a.get(k, "<absent>"), b.get(k, "<absent>"))
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
            longer = a if len(a) > len(b) else b
            extra = longer[min(len(a), len(b))]
            what = extra.get("type", extra) if isinstance(extra, dict) else extra
            return (f"{path}.length", f"{len(a)}", f"{len(b)} (first extra: {json.dumps(what)})")
        return None
    if same_scalar(a, b):
        return None
    return (path, a, b)


def same_scalar(a, b):
    """JSON scalar equality: 1 == 1.0, but true != 1 and "1" != 1."""
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return a == b
    return type(a) is type(b) and a == b


def run(cmd, stdin_path, stdout_path, cwd=None):
    with open(stdin_path, "rb") as fin, open(stdout_path, "wb") as fout:
        proc = subprocess.run(cmd, stdin=fin, stdout=fout, cwd=cwd)
    if proc.returncode != 0:
        sys.exit(f"{' '.join(cmd)} exited with {proc.returncode}")


def load(path):
    out = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.strip():
                r = json.loads(line)
                out[r["id"]] = r
    return out


def short(v, n=160):
    s = json.dumps(v, ensure_ascii=False)
    return s if len(s) <= n else s[:n] + "..."


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("inputs")
    ap.add_argument("--markdoc", required=True, help="upstream @markdoc/markdoc 0.5.11 dist/index.js")
    ap.add_argument("--template", default="{}", help="JSON request fields for every input")
    ap.add_argument("--preset")
    ap.add_argument("--mode", choices=("parse", "parseInline"))
    ap.add_argument("--options", help="JSON markdown-it options, merged into the template's")
    ap.add_argument("--disable", help="comma-separated rule names, added to the template's")
    ap.add_argument("--enable", help="comma-separated rule names, added to the template's")
    ap.add_argument("--disable-unported", action="store_true")
    ap.add_argument("--rust-bin", help="a built mdit_tokens example (default: cargo run --release)")
    ap.add_argument("--node", default="node")
    ap.add_argument("--id-prefix", default="")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--show", type=int, default=10, help="differences to print")
    ap.add_argument("--keep", help="directory to keep requests and both outputs in")
    args = ap.parse_args()

    template = json.loads(args.template)
    if args.preset:
        template["preset"] = args.preset
    if args.mode:
        template["mode"] = args.mode
    if args.options:
        template["options"] = {**template.get("options", {}), **json.loads(args.options)}
    for key, extra in (("disable", args.disable), ("enable", args.enable)):
        if extra:
            template[key] = template.get(key, []) + [n for n in extra.split(",") if n]
    if args.disable_unported:
        template["disable"] = template.get("disable", []) + [n for n in unported_rules() if n not in template.get("disable", [])]

    work = args.keep or tempfile.mkdtemp(prefix="mdit-compare-")
    os.makedirs(work, exist_ok=True)
    req_path = os.path.join(work, "requests.jsonl")
    order, sources = [], {}
    with open(args.inputs, encoding="utf-8") as fin, open(req_path, "w", encoding="utf-8") as fout:
        for line in fin:
            if not line.strip():
                continue
            doc = json.loads(line)
            if not str(doc["id"]).startswith(args.id_prefix):
                continue
            req = dict(template)
            req.update({k: doc[k] for k in REQUEST_KEYS if k in doc})
            req["id"], req["src"] = doc["id"], doc["src"]
            fout.write(json.dumps(req, ensure_ascii=False) + "\n")
            order.append(doc["id"])
            sources[doc["id"]] = doc["src"]
            if args.limit and len(order) >= args.limit:
                break

    node_path, rust_path = os.path.join(work, "node.jsonl"), os.path.join(work, "rust.jsonl")
    run([args.node, os.path.join(HERE, "oracle_tokens.cjs"), args.markdoc], req_path, node_path)
    if args.rust_bin:
        run([args.rust_bin], req_path, rust_path)
    else:
        run(["cargo", "run", "--quiet", "--release", "--example", "mdit_tokens"], req_path, rust_path, cwd=CRATE)
    node, rust = load(node_path), load(rust_path)

    counts = {"same": 0, "differ": 0, "both-error": 0, "node-error-only": 0, "rust-error-only": 0, "missing": 0}
    shown = 0
    for doc_id in order:
        a, b = node.get(doc_id), rust.get(doc_id)
        if a is None or b is None:
            counts["missing"] += 1
            diff = ("output", "<missing>" if a is None else "present", "<missing>" if b is None else "present")
        elif "error" in a or "error" in b:
            if "error" in a and "error" in b:
                counts["both-error"] += 1
                continue
            counts["node-error-only" if "error" in a else "rust-error-only"] += 1
            diff = ("error", a.get("error", "<tokens>"), b.get("error", "<tokens>"))
        else:
            diff = first_difference({"tokens": a["tokens"], "env": a.get("env")},
                                    {"tokens": b["tokens"], "env": b.get("env")}, "")
            if diff is None:
                counts["same"] += 1
                continue
            counts["differ"] += 1
        if shown < args.show:
            shown += 1
            print(f"--- {doc_id}  src={short(sources[doc_id], 120)}")
            print(f"    at {diff[0].lstrip('.')}\n    node: {short(diff[1])}\n    rust: {short(diff[2])}")

    config = {k: template[k] for k in REQUEST_KEYS if k in template}
    print(f"config {json.dumps(config)}")
    print(f"inputs {len(order)}  " + "  ".join(f"{k} {v}" for k, v in counts.items()))
    if not args.keep:
        for p in (req_path, node_path, rust_path):
            os.remove(p)
        os.rmdir(work)
    bad = counts["differ"] + counts["node-error-only"] + counts["rust-error-only"] + counts["missing"]
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
