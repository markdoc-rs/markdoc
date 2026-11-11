"""The tokens stage over input sets and Tokenizer configurations, thrown errors compared by
name and message (compare.py counts any two throws as agreeing).

Streams each input set in batches through oracle_tokens.cjs (node, upstream's dist: a
throw is {name, message}) and examples/next_compare (the port: a throw is "Name: message")
and compares every document: tokens as upstream's JSON of them, whole (as compare.py does),
throws by name and message. Keeps counts and the first differences only, so memory stays
flat whatever the sets.

Usage (python3 -I, under bin/capped):
  python3 -I tests/next/tokenizer/gate.py --markdoc DIST/index.js --rust-bin NEXT_COMPARE \
      --inputs DIR/spec.jsonl DIR/corpus.jsonl ... [--conformance] [--configs JSON] \
      [--batch 400] [--jobs 2] [--out DIR]

--configs is a JSON list of Tokenizer configs (default: the gate's twelve, see CONFIGS).
Prints one line per (config, set) and the totals; exits 1 on any difference. "equal after
U+FFFD" counts documents equal once a lone surrogate in upstream's strings is read as U+FFFD
(a Rust string cannot hold one). "lone surrogate read back" counts documents that differ
only because smartquotes read a lone surrogate the engine made (an `xn--` link text, a
linkify `maxLength` cut) where the port's token holds U+FFFD: equal to upstream's tokens
computed with those U+FFFDs in place before smartquotes (oracle_tokens.cjs `lossy`). Both
are documented divergences (src/markdown_it/mod.rs).
"""

import argparse
import json
import multiprocessing
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
ORACLE = os.path.join(HERE, "oracle_tokens.cjs")

BOTH = {"allowIndentation": True, "allowComments": True}
CONFIGS = [
    {"allowIndentation": True, "allowComments": True},
    {"allowIndentation": True, "allowComments": False},
    {"allowIndentation": False, "allowComments": True},
    {"allowIndentation": False, "allowComments": False},
    {**BOTH, "allowLinkValidation": True},
    {**BOTH, "linkify": True},
    {"allowIndentation": False, "allowComments": False, "linkify": True},
    {**BOTH, "linkify": True, "typographer": True, "html": True, "allowLinkValidation": True},
    {**BOTH, "typographer": True, "quotes": "«»‹›"},
    {**BOTH, "html": True},
    {**BOTH, "breaks": True},
    {**BOTH, "maxNesting": 3},
]


def same_scalar(a, b):
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return float(a) == float(b)
    return type(a) is type(b) and a == b


def first_difference(a, b, path=""):
    if isinstance(a, dict) and isinstance(b, dict):
        if list(a) != list(b):
            return (path + "{keys}", list(a), list(b))
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
            return (path + ".length", len(a), len(b))
        return None
    return None if same_scalar(a, b) else (path or "(value)", a, b)


def lossy(value):
    if isinstance(value, str):
        return value.encode("utf-16", "surrogatepass").decode("utf-16", "replace")
    if isinstance(value, list):
        return [lossy(v) for v in value]
    if isinstance(value, dict):
        return {lossy(k): lossy(v) for k, v in value.items()}
    return value


def run(command, requests):
    text = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in requests)
    with tempfile.TemporaryFile("w+b") as stdin:
        stdin.write(text.encode("utf-8", "surrogatepass"))
        stdin.seek(0)
        done = subprocess.run(command, stdin=stdin, capture_output=True, cwd=CRATE)
    if done.returncode != 0:
        raise RuntimeError(f"{command[0]} exited {done.returncode}: "
                           + done.stderr.decode("utf-8", "replace")[-2000:])
    answers = [json.loads(l) for l in done.stdout.decode("utf-8", "surrogatepass").split("\n")
               if l.strip()]
    if len(answers) != len(requests):
        raise RuntimeError(f"{command[0]}: {len(requests)} requests, {len(answers)} answers")
    return answers


TALLY = ("equal", "both threw", "equal after U+FFFD", "nested past MAX_NESTING (node parsed)",
         "lone surrogate read back", "different")
RANGE_ERROR = "RangeError: Maximum call stack size exceeded"


def bracket_depth(code):
    """The deepest nesting of brackets in the text (strings not skipped: an upper bound)."""
    depth = best = 0
    for c in code:
        if c in "[{(":
            depth += 1
            best = max(best, depth)
        elif c in "]})":
            depth = max(0, depth - 1)
    return best


def compare_batch(args):
    docs, config, markdoc, rust_bin, keep = args
    oracle = run(["node", ORACLE, markdoc],
                 [{"id": d["id"], "code": d["code"], "tokenizer": config} for d in docs])
    port = run([rust_bin], [{"id": d["id"], "stage": "tokens", "code": d["code"],
                             "tokenizer": config} for d in docs])
    tally = {k: 0 for k in TALLY}
    diffs = []
    differing = []
    for d, o, p in zip(docs, oracle, port):
        if o.get("id") != d["id"] or p.get("id") != d["id"]:
            raise RuntimeError(f"answer order: {d['id']} {o.get('id')} {p.get('id')}")
        where = None
        oracle_threw = o.get("threw") and f"{o['threw']['name']}: {o['threw']['message']}"
        if p.get("threw") == RANGE_ERROR and oracle_threw != RANGE_ERROR \
                and bracket_depth(d["code"]) >= 300:
            # the grammar's MAX_NESTING (300): node parsed deeper (to tokens, or to a later
            # error), as fuzz.py counts it apart; a known, documented divergence
            tally["nested past MAX_NESTING (node parsed)"] += 1
        elif "threw" in o or "threw" in p:
            if "threw" in o and "threw" in p:
                want = f"{o['threw']['name']}: {o['threw']['message']}"
                if p["threw"] == want:
                    tally["both threw"] += 1
                elif p["threw"] == lossy(want):
                    tally["equal after U+FFFD"] += 1
                else:
                    where = ("threw", want, p["threw"])
            else:
                where = ("threw", o.get("threw"), p.get("threw", p.get("unported")))
        else:
            a, b = o["tokens"], p["tokens"]
            diff = first_difference(a, b)
            if not diff:
                tally["equal"] += 1
            elif not first_difference(lossy(a), b):
                tally["equal after U+FFFD"] += 1
            else:
                where = diff
        if where:
            tally["different"] += 1
            if len(diffs) < keep:
                diffs.append({"id": d["id"], "code": d["code"], "tokenizer": config,
                              "where": where[0], "oracle": where[1], "port": where[2]})
            differing.append((d, p))
    # A difference that goes away once node's tokens hold U+FFFD for each lone surrogate the
    # engine made, before smartquotes reads them (what the port's Rust strings hold), is the
    # documented divergence "a lone surrogate read back from a token" (src/markdown_it/mod.rs)
    if differing:
        again = run(["node", ORACLE, markdoc],
                    [{"id": d["id"], "code": d["code"], "tokenizer": config, "lossy": True}
                     for d, _ in differing])
        explained = set()
        for (d, p), o in zip(differing, again):
            if "tokens" in o and "tokens" in p and not first_difference(
                    lossy(o["tokens"]), p["tokens"]):
                explained.add(d["id"])
        tally["different"] -= len(explained)
        tally["lone surrogate read back"] += len(explained)
        diffs = [x for x in diffs if x["id"] not in explained]
    return tally, diffs


def load(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.strip():
                doc = json.loads(line)
                yield {"id": doc["id"], "code": doc.get("code", doc.get("src", ""))}


def conformance(rust_bin):
    done = subprocess.run([rust_bin, "--conformance"], capture_output=True, cwd=CRATE,
                          stdin=subprocess.DEVNULL, check=True)
    for line in done.stdout.decode("utf-8").split("\n"):
        if line.strip():
            doc = json.loads(line)
            yield {"id": doc["id"], "code": doc.get("code", "")}


def batches(docs, size):
    batch = []
    for d in docs:
        batch.append(d)
        if len(batch) == size:
            yield batch
            batch = []
    if batch:
        yield batch


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"), required=False)
    ap.add_argument("--rust-bin", required=True)
    ap.add_argument("--inputs", nargs="*", default=[])
    ap.add_argument("--conformance", action="store_true")
    ap.add_argument("--configs")
    ap.add_argument("--batch", type=int, default=400)
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--keep", type=int, default=20)
    ap.add_argument("--out")
    args = ap.parse_args()
    configs = json.loads(args.configs) if args.configs else CONFIGS
    sets = [(os.path.basename(p).removesuffix(".jsonl"), p) for p in args.inputs]
    if args.conformance:
        sets.append(("conformance", None))
    grand = {k: 0 for k in TALLY}
    all_diffs = []
    if args.out:
        os.makedirs(args.out, exist_ok=True)
    out = open(os.path.join(args.out, "differences.jsonl"), "w", encoding="utf-8",
               errors="surrogatepass") if args.out else None
    with multiprocessing.Pool(args.jobs) as pool:
        for config in configs:
            for name, path in sets:
                docs = conformance(args.rust_bin) if path is None else load(path)
                tasks = ((b, config, args.markdoc, args.rust_bin, args.keep)
                         for b in batches(docs, args.batch))
                total = {k: 0 for k in TALLY}
                for tally, diffs in pool.imap(compare_batch, tasks):
                    for k, v in tally.items():
                        total[k] += v
                    for d in diffs:
                        if len(all_diffs) < args.keep:
                            all_diffs.append(d)
                        if out:
                            out.write(json.dumps(d, ensure_ascii=False) + "\n")
                for k, v in total.items():
                    grand[k] += v
                print(f"{json.dumps(config)} {name}: {sum(total.values())} documents: "
                      + ", ".join(f"{v} {k}" for k, v in total.items()), flush=True)
    if out:
        out.close()
    print("total: " + ", ".join(f"{v} {k}" for k, v in grand.items()))
    for d in all_diffs[:10]:
        print(f"  {d['id']} {json.dumps(d['tokenizer'])}: {d['where']}: "
              f"oracle {json.dumps(d['oracle'])[:200]} | port {json.dumps(d['port'])[:200]}")
        print(f"    code {json.dumps(d['code'])[:300]}")
    sys.exit(1 if grand["different"] else 0)


if __name__ == "__main__":
    main()
