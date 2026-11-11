"""Fuzz the port's linkify-it (src/markdown_it/linkify_it/) against linkify-it 6.1.0 as
upstream Markdoc 0.5.11 bundles it, calling test, match and matchAtStart directly.

Texts are biased to links: schemes in every case (and look-alikes: long s, dotless i,
Kelvin sign, dotted I, fullwidth letters), hosts (labels with unicode, punycode, hyphens,
digits, astral letters; many labels; long labels; localhost; IPv4 and IPv6, in brackets and
as e-mail hosts), ports, paths with brackets, quotes, dots, dashes, bangs and trailing
punctuation, e-mails with every allowed name character, links inside links, and Markdoc
tags beside them; then mutated. Each batch goes through linkify_oracle.cjs (node) and the
port's `answer_requests` test (src/markdown_it/linkify_it/tests.rs), and every answer is
compared.

Usage (python3 -I; run under bin/capped):
  cargo test --release --lib --no-run      # prints the test binary, BIN below
  python3 -I tests/next/tokenizer/linkify_fuzz.py --markdoc DIST/index.js --rust-test-bin BIN \
      --count 200000 --seed 1 --jobs 2 --out DIR [--options '{"fuzzyLink": true}']

`--options` fixes linkify-it's options for every text; without it a tenth of the texts get
random ones (fuzzyLink, fuzzyIP, '---', urlAuth, maxLength). `--astral` makes the texts
astral-heavy: characters of every plane in labels, hosts, TLDs, paths and around links,
uc.micro's 228 astral punctuation characters (from linkify_classes.json) most of all, and
`xn--` labels that punycode decodes to surrogate code points. Writes DIR/differences.jsonl
(at most --keep of them) and prints the tally.
"""

import argparse
import json
import multiprocessing
import os
import random
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
ORACLE = os.path.join(HERE, "linkify_oracle.cjs")

SCHEMES = ["http", "https", "ftp", "mailto", "HTTP", "Https", "hTtPs", "FTP", "MailTo", "MAILTO",
           "httpſ", "maılto", "fTp", "ｈｔｔｐ", "htTp", "file", "javascript", "xhttp", "1http",
           "httpK", "İhttp", "data", "vbscript", "skype", "a.b+c-d", ""]
SEPS = ["://", "://", "://", ":/", ":", "//", "///", ":\\\\", ":///", ": //"]
LETTERS = list("abcdefghijklmnopqrstuvwxyzABCXYZ0123456789") + [
    "é", "ü", "ß", "ſ", "ı", "İ", "K", "ｇ", "日本", "рф", "РФ", "😀", "𝒜", "́",
    "$", "+", "=", "^", "`", "|", "~", "_", "-", "--", " ", " ", "️"]
TLDS = ["com", "org", "net", "io", "xn--p1ai", "рф", "РФ", "c", "co", "uk", "museum", "COM",
        "Org", "xn--", "xn---", "localhost", "123", "a1", "ſhop", "ınfo", "xxx", "w3b"]
PATH_PIECES = ["/", "/a", "/a/b", "?q=1&r=2", "#frag", "/(a)", "/[x]", "/{y}", "/((a))",
               "/[[[[a]]]]", "/[[[[[a]]]]]", "/(", "/)", "/[", "]", "/{", "}", "/\"a\"", "/'b'",
               "/'", "/I'm_king", "/'-x", ".", ",", ";", "!", "?", "!!!", "...", "..:a",
               "...a", "....", "---", "--", "-", "/~u", "/%20", "/@x", "/a*", "**", "*",
               "/é", "/😀", "/日本", " ", " ", "　", "<", ">", "｜", "\"", "'", "/a.b.",
               "/a..b", "/a,b", "/a;b", "/a!b", "/a?b", "/!!", "?!", "?a?", "#a#b", ")", "(",
               "/\\x", "/:x", "/a=b", "/[ ]", "/\" \"", "/'''", "/(a)(b)", "/{{}}"]
PORTS = [":80", ":8080", ":65535", ":65536", ":0", ":00000", ":123456", ":", ":6", ":60000",
         ":70000", ":99999", ":1a", ":012"]
IPV6 = ["[::1]", "[2001:db8::1]", "[::ffff:1.2.3.4]", "[1:2:3:4:5:6:7:8]", "[1:2:3:4:5:6:7:8:9]",
        "[::]", "[:::]", "[g::1]", "[::1", "[1::2::3]", "[fe80::1%25eth0]", "[::256.1.1.1]",
        "[ABCD:EF::]", "[1:2:3:4:5:6:1.2.3.4]"]
IPV4 = ["192.168.0.1", "256.1.1.1", "01.2.3.4", "1.2.3", "1.2.3.4.5", "255.255.255.255",
        "0.0.0.0", "10.0.0.1:80", "1.2.3.04", "100.200.250.255"]
MAIL_NAMES = ["user", "a.b", "a..b", ".a", "a.", "a+b!#$%&'*/=?^_`{|}~-", "x" * 64, "y" * 65,
              "é", "A_B", "1", "\"q\"", "a b", "(a", "<a", ">a", "｜a", "a@b"]
CONTEXT = ["", " ", "see ", "(", ")", "<", ">", "\"", "'", "[", "]", "*", "_", "`", "&lt;",
           "{% a %}", "{% /a %}", "[link](", "](", "<a href=", "x", "é", "😀", "\n", "\t", ".",
           ",", ":", "/", "@", "-", "$", "+", "=", "^", "|", "~", "｜", " ", "　",
           "​", " ", "http://a.b/", "mailto:"]


def astral_punctuation():
    """uc.micro's astral P, as V8 matches it: linkify_classes.json's astral rows with ZPCc 2."""
    with open(os.path.join(HERE, "linkify_classes.json"), encoding="utf-8") as f:
        table = json.load(f)
    return [chr(cp) for start, end, zpcc, *_ in table["astral"] if zpcc == 2
            for cp in range(start, end + 1)]


# astral characters of every plane besides P: letters, symbols (S), marks, format
# characters, private use, unassigned code points and noncharacters
ASTRAL_OTHER = [chr(cp) for cp in (
    0x10000, 0x10348, 0x1D49C, 0x1D7D8, 0x1F600, 0x1F3F3, 0x1F130, 0x1F44D, 0x1F3FD, 0x1F1E6,
    0x16FE0, 0x1E900, 0x1D173, 0x1D16E, 0x20000, 0x2A6D6, 0x2F800, 0x30000, 0x31350, 0x40000,
    0x5FFFF, 0xE0001, 0xE0041, 0xE0100, 0xF0000, 0xFFFFD, 0x100000, 0x10FFFD, 0x1FFFE,
    0x10FFFF)]


def punycode(code_points):
    """RFC 3492 Punycode of a list of code points, surrogates included (punycode.js's
    `encode` would pair two surrogate halves first): an `xn--` label that markdown-it's
    normalizeLinkText decodes to exactly these code units."""
    base, tmin, tmax, skew, damp = 36, 1, 26, 38, 700
    out = [chr(c) for c in code_points if c < 0x80]
    b = h = len(out)
    if b:
        out.append("-")
    n, delta, bias = 128, 0, 72

    def adapt(delta, num, first):
        delta = delta // damp if first else delta >> 1
        delta += delta // num
        k = 0
        while delta > ((base - tmin) * tmax) >> 1:
            delta //= base - tmin
            k += base
        return k + (base - tmin + 1) * delta // (delta + skew)

    while h < len(code_points):
        m = min(c for c in code_points if c >= n)
        delta += (m - n) * (h + 1)
        n = m
        for c in code_points:
            if c < n:
                delta += 1
            if c == n:
                q, k = delta, base
                while True:
                    t = tmin if k <= bias else tmax if k >= bias + tmax else k - bias
                    if q < t:
                        break
                    d = t + (q - t) % (base - t)
                    out.append(chr(d + 22 + 75 * (d < 26)))
                    q = (q - t) // (base - t)
                    k += base
                out.append(chr(q + 22 + 75 * (q < 26)))
                bias = adapt(delta, h + 1, h == b)
                delta = 0
                h += 1
        delta += 1
        n += 1
    return "".join(out)


class Gen:
    def __init__(self, seed, astral=False):
        self.r = random.Random(seed)
        self.astral = astral
        if astral:
            self.astral_p = astral_punctuation()

    def pick(self, xs):
        return self.r.choice(xs)

    def astral_char(self):
        return self.pick(self.astral_p) if self.r.random() < 0.6 else self.pick(ASTRAL_OTHER)

    def astral_run(self):
        return "".join(self.astral_char() for _ in range(self.r.randrange(1, 4)))

    def surrogate_label(self):
        """An `xn--` label decoding to surrogate code points (lone, or two halves that pair
        again), maybe with letters around them."""
        r = self.r
        cps = []
        for _ in range(r.randrange(1, 4)):
            k = r.randrange(4)
            if k == 0:
                cps += [0xD83D, 0xDE00]
            elif k == 1:
                cps.append(r.randrange(0xD800, 0xE000))
            else:
                cps.append(ord(r.choice("abcz019")))
        return "xn--" + punycode(cps)

    def sprinkle(self, s):
        """`s` with astral characters put in, in astral mode."""
        if not self.astral or not s or self.r.random() < 0.4:
            return s
        for _ in range(self.r.randrange(1, 3)):
            i = self.r.randrange(len(s) + 1)
            s = s[:i] + self.astral_run() + s[i:]
        return s

    def label(self):
        r = self.r
        if self.astral and r.random() < 0.5:
            k = r.randrange(4)
            if k == 0:
                return self.astral_run()
            if k == 1:
                return self.surrogate_label()
            return self.sprinkle("".join(self.pick(LETTERS) for _ in range(r.randrange(1, 6))))
        k = r.randrange(10)
        if k == 0:
            return "xn--" + "".join(self.pick("abc019-") for _ in range(r.randrange(0, 8)))
        if k == 1:
            return "a" * self.pick([1, 62, 63, 64, 70])
        if k == 2:
            return self.pick(["localhost", "LocalHost", "localhoſt", "www", "WWW", "例え", "😀"])
        n = r.randrange(1, 8)
        return "".join(self.pick(LETTERS) for _ in range(n))

    def host(self):
        r = self.r
        k = r.randrange(12)
        if k == 0:
            return self.pick(IPV6)
        if k == 1:
            return self.pick(IPV4)
        labels = [self.label() for _ in range(self.pick([0, 1, 1, 2, 2, 3, 4, 9, 10, 11, 12]))]
        if self.astral and r.random() < 0.2:
            labels.append(self.astral_run())
        else:
            labels.append(self.pick(TLDS))
        host = ".".join(labels)
        if r.random() < 0.1:
            host = host.replace(".", self.pick(["..", ".-", "-.", "_.", ". "]), 1)
        if r.random() < 0.1:
            host += self.pick([".", "-", "_", ":", "..", "-x"])
        return host

    def path(self):
        pieces = [self.pick(PATH_PIECES) for _ in range(self.r.randrange(0, 5))]
        if self.astral:
            pieces = [self.sprinkle(p) if self.r.random() < 0.5 else
                      self.pick(["/", "'", ".", "!", "?", "-", ",", "/'", "/("]) + self.astral_run()
                      for p in pieces]
        return "".join(pieces)

    def url(self):
        r = self.r
        k = r.randrange(10)
        if k <= 5:
            s = self.pick(SCHEMES) + self.pick(SEPS)
            if r.random() < 0.1:
                s += self.pick(["user:pass@", "u@", "a@b@", "@", "x" * 51 + "@"])
            s += self.host()
            if r.random() < 0.3:
                s += self.pick(PORTS)
            return s + self.path()
        if k <= 7:
            name = self.pick(MAIL_NAMES)
            if r.random() < 0.3:
                name = self.pick(SCHEMES[3:6]) + ":" + name
            host = self.pick(["[IPv6:::1]", "[ipv6:2001:db8::1]", "[IPV6:x]"]) if r.random() < 0.1 \
                else self.host()
            return name + "@" + host + (self.path() if r.random() < 0.2 else "")
        if k == 8:
            return "//" + self.host() + (self.pick(PORTS) if r.random() < 0.3 else "") + self.path()
        return self.host() + self.path()

    def text(self):
        r = self.r
        parts = []
        for _ in range(r.randrange(1, 5)):
            parts.append(self.sprinkle(self.pick(CONTEXT)))
            parts.append(self.url())
        parts.append(self.sprinkle(self.pick(CONTEXT)))
        s = "".join(parts)
        if r.random() < 0.4:
            s = self.mutate(s)
        return s

    def mutate(self, s):
        r = self.r
        for _ in range(r.randrange(1, 4)):
            if not s:
                break
            i = r.randrange(len(s))
            k = r.randrange(5)
            if k == 0:
                s = s[:i] + s[i + 1:]
            elif k == 1:
                extra = self.astral_run() if self.astral and r.random() < 0.5 else \
                    self.pick(LETTERS + CONTEXT)
                s = s[:i] + extra + s[i:]
            elif k == 2:
                s = s[:i] + s[i].swapcase() + s[i + 1:]
            elif k == 3:
                j = r.randrange(len(s))
                s = s[:i] + s[j:] + s[:j]
            else:
                s = s[:i] + s[i:] * 2
        return s

    def options(self):
        r = self.r
        o = {}
        for key, p in (("fuzzyLink", 0.5), ("fuzzyIP", 0.3), ("---", 0.3), ("urlAuth", 0.3),
                       ("fuzzyEmail", 0.2)):
            if r.random() < p:
                o[key] = key != "fuzzyEmail"
        if r.random() < 0.2:
            o["maxLength"] = self.pick([1, 2, 5, 9, 20, 100])
        return o


def run(command, requests, env=None):
    text = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in requests)
    with tempfile.TemporaryFile("w+b") as stdin:
        stdin.write(text.encode("utf-8"))
        stdin.seek(0)
        done = subprocess.run(command, stdin=stdin, capture_output=True, cwd=CRATE, env=env)
    if done.returncode != 0:
        raise RuntimeError(f"{command[0]} exited {done.returncode}: "
                           + done.stderr.decode("utf-8", "replace")[-2000:])
    return done.stdout.decode("utf-8")


def rust_answers(rust_bin, requests):
    with tempfile.TemporaryDirectory() as d:
        req, ans = os.path.join(d, "requests.jsonl"), os.path.join(d, "answers.jsonl")
        with open(req, "w", encoding="utf-8") as f:
            for r in requests:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        env = dict(os.environ, LINKIFY_REQUESTS=req, LINKIFY_ANSWERS=ans)
        done = subprocess.run([rust_bin, "--ignored", "--exact",
                               "markdown_it::linkify_it::tests::answer_requests", "--quiet"],
                              capture_output=True, cwd=CRATE, env=env)
        if done.returncode != 0:
            raise RuntimeError("port exited %d: %s" % (done.returncode,
                                                         done.stderr.decode()[-2000:]))
        with open(ans, encoding="utf-8") as f:
            return f.read()


def lossy(value):
    """Upstream's strings as a Rust string holds them: a lone surrogate as U+FFFD."""
    if isinstance(value, str):
        return value.encode("utf-16", "surrogatepass").decode("utf-16", "replace")
    if isinstance(value, list):
        return [lossy(v) for v in value]
    if isinstance(value, dict):
        return {lossy(k): lossy(v) for k, v in value.items()}
    return value


def batch(args):
    seed, start, count, options, markdoc, rust_bin, keep, astral = args
    gen = Gen(f"{seed}:{start}", astral)
    requests = []
    for i in range(count):
        o = options if options is not None else (gen.options() if gen.r.random() < 0.1 else {})
        requests.append({"id": f"s{seed}-{start + i}", "text": gen.text(), "options": o})
    oracle = [json.loads(l) for l in run(["node", ORACLE, markdoc], requests).split("\n")
              if l.strip()]
    port = [json.loads(l) for l in rust_answers(rust_bin, requests).split("\n") if l.strip()]
    if len(oracle) != len(requests) or len(port) != len(requests):
        raise RuntimeError(f"{len(requests)} requests, {len(oracle)} oracle, {len(port)} port")
    tally = {"equal": 0, "different": 0, "equal after U+FFFD": 0, "with links": 0}
    diffs = []
    for r, o, p in zip(requests, oracle, port):
        if o.get("match") or o.get("matchAtStart"):
            tally["with links"] += 1
        if o == p:
            tally["equal"] += 1
        elif lossy(o) == p:
            # a match boundary inside a surrogate pair: equal but for the lone half
            tally["equal after U+FFFD"] += 1
        else:
            tally["different"] += 1
            if len(diffs) < keep:
                diffs.append({"request": r, "oracle": o, "port": p})
    return tally, diffs


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"))
    ap.add_argument("--rust-test-bin", required=True)
    ap.add_argument("--options")
    ap.add_argument("--count", type=int, default=20000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--batch", type=int, default=10000)
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--keep", type=int, default=50)
    ap.add_argument("--out")
    ap.add_argument("--print", type=int)
    ap.add_argument("--astral", action="store_true")
    args = ap.parse_args()
    if args.print:
        gen = Gen(f"{args.seed}:0", args.astral)
        for _ in range(args.print):
            print(json.dumps(gen.text(), ensure_ascii=False))
        return
    options = json.loads(args.options) if args.options else None
    tasks = [(args.seed, s, min(args.batch, args.count - s), options, args.markdoc,
              args.rust_test_bin, args.keep, args.astral)
             for s in range(0, args.count, args.batch)]
    total = {"equal": 0, "different": 0, "equal after U+FFFD": 0, "with links": 0}
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
    print(f"options {args.options or 'mixed'}{' astral' if args.astral else ''} seed "
          f"{args.seed}: {args.count} texts: "
          + ", ".join(f"{v} {k}" for k, v in total.items()))
    for d in diffs[:5]:
        print("  text", json.dumps(d["request"]["text"])[:300], json.dumps(d["request"]["options"]))
        print("    oracle", json.dumps(d["oracle"])[:500])
        print("    port  ", json.dumps(d["port"])[:500])
    sys.exit(1 if total["different"] else 0)


if __name__ == "__main__":
    main()
