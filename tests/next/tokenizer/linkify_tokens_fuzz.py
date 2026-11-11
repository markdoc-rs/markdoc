"""Fuzz the tokenizer with linkify on: Markdoc documents (fuzz.py's generator) with links
put into them (linkify_fuzz.py's URLs, e-mails, hosts, IPs, ports and paths), also inside
markdown links, autolinks, code, emphasis, HTML and tags, compared with upstream by gate.py
(tokens field by field; throws by name and message).

Usage (python3 -I; run under bin/capped):
  python3 -I tests/next/tokenizer/linkify_tokens_fuzz.py --markdoc DIST/index.js \
      --rust-bin NEXT_COMPARE --config '{"allowIndentation": true, "allowComments": true,
      "linkify": true}' --count 100000 --seed 1 --jobs 2 --out DIR [--astral]

`--astral` makes the links astral-heavy (linkify_fuzz.py's `--astral`: every plane, astral
punctuation, `xn--` labels decoding to surrogates) and puts astral characters into the rest of
the document too; give it a `typographer` config with astral `quotes` as well.
"""

import argparse
import json
import multiprocessing
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True  # no __pycache__ beside the scripts
sys.path.insert(0, HERE)

import fuzz  # noqa: E402  (this directory's own modules)
import gate  # noqa: E402
import linkify_fuzz  # noqa: E402

WRAPS = ["{}", "{}", "{}", "[{}](u)", "[x]({})", "<{}>", "`{}`", "**{}**", "_{}_", "~~{}~~",
         "<a href=\"x\">{}</a>", "<a>{}</a>", "\\{}", "[{}]", "![{}](i)", "{{% a %}}{}",
         "{}{{% b /%}}", "{{% a x=\"{}\" %}}", "\"{}\"", "'{}'", "({})", "&lt;{}&gt;",
         "{}\n", "> {}", "- {}", "# {}", "| {} |", "    {}", "\n{}"]


class Gen(fuzz.Gen):
    def __init__(self, seed, astral=False):
        super().__init__(seed)
        self.links = linkify_fuzz.Gen(f"links:{seed}", astral)
        self.astral = astral

    def link(self):
        frag = self.links.url() if self.chance(0.8) else self.links.text()
        return self.pick(WRAPS).format(frag)

    def document(self):
        doc = super().document()
        k = self.r.randrange(4)
        if k == 0:
            doc = self.link() + self.pick([" ", "\n", "", "\n\n"]) + doc
        else:
            parts = doc.split(" ")
            for _ in range(self.r.randrange(1, 5)):
                parts.insert(self.r.randrange(len(parts) + 1), self.link())
            doc = " ".join(parts)
        if self.astral:
            # astral characters beside quotes, brackets and the links' edges as well
            for _ in range(self.r.randrange(0, 4)):
                i = self.r.randrange(len(doc) + 1)
                doc = doc[:i] + self.pick(['"', "'", "", "(", "[", "*"]) + \
                    self.links.astral_run() + self.pick(['"', "'", "", ")", "]", "*"]) + doc[i:]
        return doc


def batch(args):
    seed, start, count, config, markdoc, rust_bin, keep, astral = args
    gen = Gen(f"{seed}:{start}", astral)
    docs = [{"id": f"s{seed}-{start + i}", "code": gen.document()} for i in range(count)]
    return gate.compare_batch((docs, config, markdoc, rust_bin, keep))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"))
    ap.add_argument("--rust-bin", required=True)
    ap.add_argument("--config", default='{"allowIndentation": true, "allowComments": true, '
                                        '"linkify": true}')
    ap.add_argument("--count", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--batch", type=int, default=2000)
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--keep", type=int, default=20)
    ap.add_argument("--out")
    ap.add_argument("--print", type=int)
    ap.add_argument("--astral", action="store_true")
    args = ap.parse_args()
    if args.print:
        gen = Gen(f"{args.seed}:0", args.astral)
        for _ in range(args.print):
            print(json.dumps(gen.document(), ensure_ascii=False))
        return
    config = json.loads(args.config)
    tasks = [(args.seed, s, min(args.batch, args.count - s), config, args.markdoc,
              args.rust_bin, args.keep, args.astral) for s in range(0, args.count, args.batch)]
    total = {k: 0 for k in gate.TALLY}
    diffs = []
    with multiprocessing.Pool(args.jobs) as pool:
        for tally, d in pool.imap_unordered(batch, tasks):
            for k, v in tally.items():
                total[k] += v
            diffs += d[: max(0, args.keep - len(diffs))]
    if args.out:
        os.makedirs(args.out, exist_ok=True)
        with open(os.path.join(args.out, "differences.jsonl"), "w", encoding="utf-8",
                  errors="surrogatepass") as f:
            for d in diffs:
                f.write(json.dumps(d, ensure_ascii=False) + "\n")
    print(f"{json.dumps(config)}{' astral' if args.astral else ''} seed {args.seed}: "
          f"{args.count} documents: "
          + ", ".join(f"{v} {k}" for k, v in total.items()), flush=True)
    for d in diffs[:5]:
        print(f"  {d['id']}: {d['where']}: oracle {json.dumps(d['oracle'])[:200]} | "
              f"port {json.dumps(d['port'])[:200]}")
        print(f"    code {json.dumps(d['code'])[:300]}")
    sys.exit(1 if total["different"] else 0)


if __name__ == "__main__":
    main()
