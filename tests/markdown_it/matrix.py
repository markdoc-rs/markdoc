"""The verification matrix: compare.py over input files x configurations, in parallel.

Runs every input file in `parse` mode, and the `--inline` files also in `parseInline` mode,
under each configuration of fuzz.CONFIGS (default, Markdoc's two, commonmark, zero,
typographer, html), and prints one tally line per run; exits 1 if any run differs.

Usage (run python with -I, from the crate root):
  python3 -I tests/markdown_it/matrix.py --markdoc DIST/index.js --rust-bin BIN
      INPUTS.jsonl... [--inline spec.jsonl,...] [--configs a,b] [--jobs N] [--show N]
"""

import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True  # no __pycache__ in the crate
from fuzz import CONFIGS, EXTRA_CONFIGS  # noqa: E402

TALLY = re.compile(r"inputs (\d+)\s+same (\d+)\s+differ (\d+)\s+both-error (\d+)\s+"
                   r"node-error-only (\d+)\s+rust-error-only (\d+)\s+missing (\d+)")


def run(args, inputs, config, mode):
    fields = dict(CONFIGS[config] if config in CONFIGS else EXTRA_CONFIGS[config])
    if mode == "parseInline":
        fields["mode"] = mode
    cmd = ["python3", "-I", os.path.join(HERE, "compare.py"), inputs, "--markdoc", args.markdoc,
           "--rust-bin", args.rust_bin, "--show", str(args.show), "--template",
           json.dumps(fields)]
    p = subprocess.run(cmd, capture_output=True, text=True)
    m = TALLY.search(p.stdout)
    name = f"{os.path.basename(inputs)} {config} {mode}"
    return name, (tuple(int(x) for x in m.groups()) if m else None), p.stdout + p.stderr


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("inputs", nargs="+")
    ap.add_argument("--markdoc", required=True)
    ap.add_argument("--rust-bin", required=True)
    ap.add_argument("--inline", default="", help="comma-separated input files also run in parseInline mode")
    ap.add_argument("--configs", default=",".join(CONFIGS))
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--show", type=int, default=3, help="differences printed per run")
    args = ap.parse_args()
    inline = [p for p in args.inline.split(",") if p]
    jobs = [(i, c, "parse") for c in args.configs.split(",") for i in args.inputs]
    jobs += [(i, c, "parseInline") for c in args.configs.split(",") for i in inline]
    bad = 0
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as ex:
        for name, counts, out in ex.map(lambda j: run(args, *j), jobs):
            if counts is None:
                bad += 1
                print(f"{name}: FAILED\n{out[-2000:]}")
                continue
            n, same, differ, both, node_only, rust_only, missing = counts
            flagged = differ + node_only + rust_only + missing
            bad += 1 if flagged else 0
            print(f"{name:45s} inputs {n:6d}  same {same:6d}  differ {differ}  both-error {both}"
                  f"  node-error-only {node_only}  rust-error-only {rust_only}  missing {missing}")
            if flagged:
                print("".join("    " + line + "\n" for line in out.splitlines()[:-2]))
    print(f"runs {len(jobs)}  with differences {bad}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
