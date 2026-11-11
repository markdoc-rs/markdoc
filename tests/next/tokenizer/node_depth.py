"""node's depth limits per construct, cold (each probe a fresh node process, one tokenize)
and warm (node_depth.cjs warm: one process, after 3,000 warm-up documents).

  python3 -I node_depth.py DIST
"""
import subprocess
import sys

DIST = sys.argv[1]
HERE = __file__.rsplit("/", 1)[0]
CONSTRUCTS = ["blockquote", "list", "ordered", "quoted-list", "image", "link-image", "bracket"]


def fits(name, n):
    out = subprocess.run(["node", f"{HERE}/node_depth.cjs", DIST, "probe", name, str(n)],
                         capture_output=True, text=True, check=True).stdout.strip()
    return out == "ok"


def main():
    warm = subprocess.run(["node", f"{HERE}/node_depth.cjs", DIST, "warm"], capture_output=True,
                          text=True, check=True).stdout
    print("warm (one process, after 3,000 documents):")
    print(warm, end="")
    print("cold (a fresh process per probe):")
    for name in CONSTRUCTS:
        lo, hi = 1, 40000
        while hi - lo > 1:
            mid = (lo + hi) // 2
            if fits(name, mid):
                lo = mid
            else:
                hi = mid
        print(name, lo, flush=True)


if __name__ == "__main__":
    main()
