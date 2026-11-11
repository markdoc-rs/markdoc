"""Binary-search the smallest thread stack on which the tokenizer handles a nested input:
how `markdown_it::MAX_DEPTH` was chosen (see src/next/tokenizer/stack.rs).

Runs the lib test binary's ignored `next::tokenizer::stack::probe` once per stack size (an
overflow aborts the process), 4 KiB steps. Build it in debug (`cargo test --lib --no-run`
prints its path). Usage (python3 -I, under bin/capped):
  stack_probe.py TEST_BIN CONSTRUCT DEPTH TAG [CONSTRUCT DEPTH TAG ...]
CONSTRUCT is one of `stack::nested`'s (blockquote, list, ordered, quoted-list, image,
link-image, bracket), TAG 1 to put the grammar's deepest tag inside, else 0.
Prints CONSTRUCT DEPTH TAG MIN_STACK_KIB OUTCOME.
"""
import os
import subprocess
import sys


def fits(bin_, construct, depth, tag, stack):
    env = dict(os.environ, PROBE_CONSTRUCT=construct, PROBE_DEPTH=str(depth),
               PROBE_STACK=str(stack), PROBE_TAG=tag, RUST_BACKTRACE="0")
    done = subprocess.run([bin_, "--ignored", "--exact", "next::tokenizer::stack::probe",
                           "--nocapture", "--test-threads", "1"],
                          capture_output=True, env=env, timeout=600)
    out = done.stdout.decode("utf-8", "replace")
    line = next((l[l.index("PROBE "):] for l in out.splitlines() if "PROBE " in l), None)
    return done.returncode == 0 and line is not None, line


def search(bin_, construct, depth, tag):
    lo, hi = 16 * 1024, 64 * 1024 * 1024
    ok, line = fits(bin_, construct, depth, tag, hi)
    if not ok:
        return None, line
    best = line
    while hi - lo > 4 * 1024:
        mid = (lo + hi) // 2
        ok, line = fits(bin_, construct, depth, tag, mid)
        if ok:
            hi, best = mid, line
        else:
            lo = mid
    return hi, best


def main():
    bin_ = sys.argv[1]
    args = sys.argv[2:]
    for i in range(0, len(args), 3):
        construct, depth, tag = args[i], int(args[i + 1]), args[i + 2]
        stack, line = search(bin_, construct, depth, tag)
        outcome = line.split(" ", 4)[4] if line else "?"
        print(construct, depth, tag, stack // 1024 if stack else None, outcome[:80], flush=True)


if __name__ == "__main__":
    main()
