"""Write the node-free regression sets of the linkify port from upstream's dist:

- linkify_cases.jsonl: `{text, options, answer}`, linkify-it's answers (`test`, `match`,
  `matchAtStart`, as linkify_oracle.cjs gives them) for hand-written edge cases and
  generated texts (linkify_fuzz.py's generator), read by src/markdown_it/linkify_it/tests.rs;
- linkify_golden.jsonl: `{id, code, tokenizer, tokens | threw}`, the tokenizer's output with
  `linkify: true` (oracle_tokens.cjs), read by src/next/tokenizer/tests.rs.

Usage (python3 -I, under bin/capped): linkify_golden.py --markdoc DIST/index.js
"""

import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True  # no __pycache__ beside the scripts
sys.path.insert(0, HERE)

import linkify_fuzz  # noqa: E402
import linkify_tokens_fuzz  # noqa: E402

TEXTS = [
    "see https://example.com/a?b#c. and x@y.org",
    "HTTP://X.ORG hTTps://x.org/(a) ftp://f.io:21/x",
    "//localhost:8080/p a://b.c x///y.com ://a.b",
    "mailto:a@b.co MAILTO:A@B.CO a@b.co a.b@c.d.e a..b@c.d .a@b.c a.@b.c",
    "user+tag@sub.example.co.uk " + "x" * 64 + "@a.bc " + "y" * 65 + "@a.bc",
    "http://[::1]:80/x http://[2001:db8::1]/ http://[1:2:3:4:5:6:7:8:9]/ a@[IPv6:::1] "
    "a@[ipv6:2001:db8::1] a@[IPV6:x]",
    "http://a.b.c.d.e.f.g.h.i.j.k/ http://a.b.c.d.e.f.g.h.i.j.k.l/",
    "http://xn--80ak6aa92e.com/ http://пример.рф/ http://日本.jp http://😀.com",
    "http://example.com:65535 http://e.com:65536 http://e.com:0 http://e.com:00000 http://e.com:6",
    "http://a.b/x... http://a.b/x..y http://a.b/x..:z http://a.b/x!!! http://a.b/x!!!y",
    "http://a.b/x--y http://a.b/x---y http://a.b/'quoted' http://a.b/I'm_king http://a.b/\"q\"",
    "http://a.b/[a[b[c[d]]]] http://a.b/[a[b[c[d[e]]]]] http://a.b/(a)(b) http://a.b/{c}",
    "httpſ://x.org maılto:a@b.co http://x.\u212aR/ İhttp://x.org ｈｔｔｐ://x.org",
    "http://example.com. http://example.com.. http://example.com.- http://example.com_x",
    "http://a-.com http://-a.com http://a_b.com http://a..b.com http://a.b-",
    "<http://a.b> (http://a.b) [http://a.b] \"http://a.b\" 'http://a.b' http://a.b,",
    "http://a.b/x?y=1&z=2#f http://a.b/%20 http://a.b/@x http://a.b/~u http://a.b/*x*",
    "http://a.b/\u00a0x http://a.b/\u3000x http://a.b/\u2028x http://a.b/x\u200b",
    "xhttp://a.b 1http://a.b _http://a.b -http://a.b >http://a.b ｜http://a.b",
    "www.example.com example.com 1.2.3.4 256.1.1.1 a---b.com x.co.uk test.museum foo.xxx",
    "a@b a@b.c a@b.c-d a@1.2.3.4 \"a\"@b.co (a@b.co) <a@b.co> a@b.co.",
    "",
    "http://",
    "mailto:",
    "@",
    # every astral character is a pseudo-letter, uc.micro's astral punctuation included
    "http://a\U00010100b.com x@a\U00010100b.com http://a.com/x'\U00010100 http://\U00010100.com "
    "http://a.\U00010100",
    "//a\U00010100.b\U000E0001c http://a\U0001F600b.com/\U00010100x http://a.b/I'\U0001F600 "
    "x@\U00010100.\U00010100 \U00011047http://x.y \U00011047a@b.co a\U00011047@b.co",
    "http://\U0001BC9F.\U0001E95E/\U00012470 http://a-\U00010100-b.c x@a.b\U0001DA87 "
    "http://a.b/(\U00010101) http://a.b/'\U000F0000 http://a.b/\U000E0100",
]
OPTION_SETS = [{}, {"fuzzyLink": True}, {"fuzzyLink": True, "fuzzyIP": True},
               {"fuzzyLink": True, "---": True}, {"urlAuth": True}, {"maxLength": 5},
               {"fuzzyEmail": False}]

DOCS = [
    "see https://example.com/a?b#c. and x@y.org",
    "[http://a.b](http://c.d) <http://e.f> `http://g.h` **http://i.j** _k@l.mn_",
    "http\\://example.com/ http:\\//example.com/ \\http://a.b",
    "<a href=\"x\">http://a.b</a> http://c.d <a>e@f.gh</a>",
    "{% tag %}http://a.b{% /tag %} https://x/{% a %} {% a x=\"http://b.c\" %}",
    "> http://quote.org\n\n- http://list.org\n- a@b.co\n\n# http://heading.org",
    "| http://a.b | c@d.ef |\n|---|---|\n| x | y |",
    "https://a.b/***a*** **https://a.b/x** *http://a.b*",
    "\"http://a.b\" 'mailto:a@b.co' -- http://a.b/x--- (c) http://a.b/(c)",
    "http://a.b/" + "x" * 10010,
    "line http://a.b\nhttp://c.d  \nnext\\\nhttp://e.f",
    "[x]: http://ref.org\n\n[x] http://y.z [x][]",
    "![http://img.org](http://src.org) [![x](http://a.b)](http://c.d)",
    "HTTP://X.ORG hTtPs://Y.ORG/(x) ftp://z.org:21 //w.org/p mailto:Q@R.ST",
]
# after the generated documents, so that adding them changed no case before them
ASTRAL_DOCS = [
    "http://a\U00010100b.com and x@a\U00010100b.com, http://a.com/x'\U00010100 "
    "\"http://\U00010100.com\" (x@\U00011047.\U00010100)",
    "- http://\U0001BC9F.\U0001E95E/\U00012470 <http://a\U00010100b.com>\n- "
    "[x](http://a\U00010100b.com) **x@a\U00010100b.com**",
]
CONFIGS = [
    {"allowIndentation": True, "allowComments": True, "linkify": True},
    {"allowIndentation": False, "allowComments": False, "linkify": True},
    {"allowIndentation": True, "allowComments": True, "linkify": True, "typographer": True,
     "html": True, "allowLinkValidation": True},
]


def whole(line):
    """Whether the line holds no lone surrogate (a Rust string cannot; those cases are
    counted apart by the fuzzers)."""
    try:
        line.encode("utf-8")
        return True
    except UnicodeEncodeError:
        return False


def run(command, requests):
    text = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in requests)
    done = subprocess.run(command, input=text.encode("utf-8"), capture_output=True, check=True)
    return [json.loads(l) for l in done.stdout.decode("utf-8", "surrogatepass").split("\n")
            if l.strip()]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"), required=False)
    args = ap.parse_args()

    requests = [{"id": f"t{i}-{j}", "text": t, "options": o}
                for i, t in enumerate(TEXTS) for j, o in enumerate(OPTION_SETS)]
    gen = linkify_fuzz.Gen("golden")
    for i in range(300):
        requests.append({"id": f"g{i}", "text": gen.text(),
                         "options": gen.options() if i % 3 == 0 else {}})
    answers = run(["node", os.path.join(HERE, "linkify_oracle.cjs"), args.markdoc], requests)
    with open(os.path.join(HERE, "linkify_cases.jsonl"), "w", encoding="utf-8") as f:
        for r, a in zip(requests, answers):
            assert a["id"] == r["id"]
            a = {k: v for k, v in a.items() if k != "id"}
            line = json.dumps({"text": r["text"], "options": r["options"], "answer": a},
                              ensure_ascii=False, separators=(",", ":"))
            if whole(line):
                f.write(line + "\n")

    cases = [{"id": f"d{i}-{j}", "code": d, "tokenizer": c}
             for i, d in enumerate(DOCS) for j, c in enumerate(CONFIGS)
             if j == 0 or len(d) < 1000]
    gen = linkify_tokens_fuzz.Gen("golden")
    first = len(cases)
    while len(cases) < first + 80:
        doc = gen.document()
        if len(doc) <= 600:
            cases.append({"id": f"f{len(cases)}", "code": doc,
                          "tokenizer": CONFIGS[len(cases) % len(CONFIGS)]})
    cases += [{"id": f"a{i}-{j}", "code": d, "tokenizer": c}
              for i, d in enumerate(ASTRAL_DOCS) for j, c in enumerate(CONFIGS)]
    answers = run(["node", os.path.join(HERE, "oracle_tokens.cjs"), args.markdoc], cases)
    with open(os.path.join(HERE, "linkify_golden.jsonl"), "w", encoding="utf-8") as f:
        for c, a in zip(cases, answers):
            assert a["id"] == c["id"]
            line = json.dumps({**c, **{k: v for k, v in a.items() if k != "id"}},
                              ensure_ascii=False, separators=(",", ":"))
            if whole(line):
                f.write(line + "\n")
    print(f"wrote {len(requests)} linkify-it cases and {len(cases)} token cases")


if __name__ == "__main__":
    main()
