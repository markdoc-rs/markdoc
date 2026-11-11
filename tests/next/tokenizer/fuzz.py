"""Fuzz the tokenizer port against upstream Markdoc 0.5.11, with documents biased to Markdoc.

Generates documents with tags in every block and inline context (paragraph starts and ends,
list items, blockquotes, tables, headings, link text and destinations, code spans), fences
with tags in their content and info string (process=false and other falsy values),
multi-line tags, `%}` inside strings, unclosed tags, $variables and functions at block
level, annotations, comments in every position, and frontmatter shapes (CRLF,
unterminated, spaced), then mutates some of them. Each batch goes through
tests/next/oracle.cjs (node) and examples/next_compare (the port) at the tokens stage,
and every difference is written out with its document.

Usage (python3 -I; run from anywhere):
  python3 -I tests/next/tokenizer/fuzz.py --markdoc DIST/index.js --rust-bin NEXT_COMPARE \
      --config '{"allowIndentation": true, "allowComments": true}' --count 300000 \
      --seed 1 --jobs 6 --out DIR

Writes DIR/differences.jsonl ({id, code, tokenizer, where, oracle, port}) and prints the
tally: equal, different, both threw, and how many documents compare equal only after a
lone surrogate in upstream's strings is read as U+FFFD (a Rust string cannot hold one; the
grammar quotes the one code unit it fails at, which can be half of an emoji).
`--print N` prints N generated documents instead (no comparison).
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
ORACLE = os.path.join(os.path.dirname(HERE), "oracle.cjs")

# --- generating -------------------------------------------------------------------------------

NAMES = ["a", "note", "if", "else", "table", "partial", "slot", "x-y", "tag_1", "callout",
         "process", "b2", "Tab"]
WORDS = ["foo", "bar", "baz", "Hello", "world", "x", "1.", "2)", "a_b", "*", "_", "#", "+",
         "-", ">", "|", "`", "~", "&amp;", "&copy", "\\*", "\\{%", "%}", "{%", "{", "}", "%",
         "é", "😀", " ", "\t", "https://x.org", "www.y.com", "<b>", "</b>", "<!-", "->",
         "---", "***", "$x", "@a", "!", "[", "]", "(", ")", "\"", "'", ":", "=", "\\"]
URLS = ["/u", "https://example.com/p", "http://a.b/{% x %}", "https://{% $v %}",
        "vscode://{% $v.w %}", "<a b>", "u \"t\"", "#h", "ftp://f/{% y /%}", "{% a %}",
        "https://x/{% a %}y{% /a %}", ""]


class Gen:
    def __init__(self, seed):
        self.r = random.Random(seed)

    def pick(self, xs):
        return self.r.choice(xs)

    def chance(self, p):
        return self.r.random() < p

    # values and tag contents

    def value(self, depth=0):
        r = self.r
        k = r.randrange(18 if depth < 3 else 11)
        if k == 0:
            return '"' + self.pick(["v", "%}", "a \\\" %} b", "", "😀", "{% x %}", "\\n", "é\\u0041",
                                    "a\\\\", "multi\nline"]) + '"'
        if k == 1:
            return self.pick(["1", "-2.5", "0", "-0", "3.14", "0.000000059604644775390625", "10",
                              "123456789012345678901234567890", ".5", "1.", "007"])
        if k == 2:
            return self.pick(["true", "false", "null"])
        if k == 3:
            return "$" + self.pick(["v", "v.w", "v[0]", "v.w[\"k\"]", "a_b.c", "x[1][2]"])
        if k == 4:
            return "@" + self.pick(["a", "a.b", "a.b.c"])
        if k in (5, 6):
            return self.pick(["1e5", "0x10", "'single'", "bare", "=", "\"open", "[1,", "{a:",
                              "fn(", "$", "😀", "-", "+1", "NaN"])
        if k in (7, 8, 9, 10):
            return self.pick(["x", "\"y\"", "2", "true", "$z"])
        if k in (11, 12):
            items = [self.value(depth + 1) for _ in range(self.r.randrange(0, 4))]
            return "[" + ", ".join(items) + self.pick(["]", "]", "]", ",]"])
        if k in (13, 14):
            items = [self.pick(["a", "b", "\"c d\"", "__proto__", "0", "$$mdtype", "1"]) + ": " +
                     self.value(depth + 1) for _ in range(self.r.randrange(0, 4))]
            return "{" + ", ".join(items) + self.pick(["}", "}", "}", ",}"])
        args = []
        for _ in range(self.r.randrange(0, 4)):
            args.append(self.pick(["", "", "k="]) + self.value(depth + 1))
        return self.pick(["fn", "default", "equals", "and", "f_1"]) + "(" + ", ".join(args) + ")"

    def deep(self):
        """Values nested deep: within the port's limit (300), or far past V8's."""
        levels = self.pick([5, 50, 150, 250, 299, 20000])
        o, c = self.pick([("[", "]"), ("{a: ", "}"), ("f(1, ", ")"), ("f(", ")")])
        d = levels - 1
        return o * d + "1" + c * d

    def attrs(self):
        out = []
        for _ in range(self.r.randrange(0, 4)):
            k = self.r.randrange(7)
            if k == 0:
                out.append("#" + self.pick(["id", "a-b", "x_1", "😀"]))
            elif k == 1:
                out.append("." + self.pick(["cls", "a-b", "c1"]))
            elif k == 2:
                out.append(self.pick(["process", "process"]) + "=" +
                           self.pick(["false", "true", "0", "\"\"", "null", "1", "$p", "\"no\""]))
            else:
                out.append(self.pick(["k", "id", "class", "src", "a-b", "primary", "x"]) + "=" +
                           self.value())
        return out

    def content(self):
        """The inside of a tag."""
        k = self.r.randrange(16)
        name = self.pick(NAMES)
        if k <= 3:
            parts = [name]
            if self.chance(0.3):
                parts.append(self.value())
            parts += self.attrs()
            return " ".join(parts)
        if k <= 5:
            return "/" + name
        if k <= 7:
            return " ".join([name] + self.attrs()) + self.pick([" /", "/", " / "])
        if k == 8:
            return " ".join(self.attrs() or ["#i"])
        if k == 9:
            return "$" + self.pick(["v", "v.w", "v[0]", "x.y[\"z\"]"])
        if k == 10:
            return self.pick(["fn()", "f(1, $x)", "default($a, 1)", "x(k=1)"])
        if k == 11:
            return self.pick(["", " ", "1", "=", "😀", "a b c", "a =", "/", "a /% b", "\"s\"",
                              "@a.b", "#", ".", "a #😀", "a\tb", "a b"])
        if k == 12:
            return name + " x=" + (self.deep() if self.chance(0.1) else self.value())
        if k == 13:
            return name + "\n  " + "\n  ".join(self.attrs() or ["k=1"]) + self.pick(["", "\n", " /"])
        if k == 14:
            return name + " a=\"%}\" b=\"{%\""
        return name + " " + self.value()

    def tag(self, inner=None):
        c = self.content() if inner is None else inner
        k = self.r.randrange(20)
        if k == 0:
            return "{%" + c  # unclosed
        if k == 1:
            return "{% " + c + " %"  # unclosed
        if k == 2:
            return "{%" + c + "%}"
        if k == 3:
            return "{%\n" + c + "\n%}"
        if k == 4:
            return self.pick(["{%}", "{%%}", "{% %}", "{%  %}", "{% \" %}", "{% \"a\\\"%}\" %}"])
        if k == 5:
            return "{%  " + c + "   %}"
        return "{% " + c + " %}"

    def inline(self, n=None, label=False):
        """Inline content; in a link's label a comment is rare (with allowComments it makes
        upstream throw, as markdown-it's skipToken rejects the comment rule's answer)."""
        n = self.r.randrange(1, 9) if n is None else n
        out = []
        for _ in range(n):
            k = self.r.randrange(24)
            if label and k in (16, 21) and not self.chance(0.1):
                k = 0
            if k <= 5:
                out.append(self.pick(WORDS))
            elif k <= 9:
                out.append(self.tag())
            elif k == 10:
                out.append("*" + self.inline(2, label) + "*")
            elif k == 11:
                out.append("**" + self.inline(2, label) + "**")
            elif k == 12:
                out.append("`" + self.pick(["x", self.tag(), "a {% b %} c"]) + "`")
            elif k == 13:
                out.append("[" + self.inline(2, True) + "](" + self.pick(URLS) + ")")
            elif k == 14:
                out.append("![" + self.inline(2, True) + "](" + self.pick(URLS) + ")")
            elif k == 15:
                out.append("<" + self.pick(["https://x.y/{% a %}", "https://a.b", "mailto:a@b.c",
                                            "a@b.c", "x {% y %}"]) + ">")
            elif k == 16:
                out.append("<!--" + self.pick([" c ", "", "-", " {% a %} ", "\nmulti\n",
                                               " unclosed"]) + self.pick(["-->", "-->", "->", ""]))
            elif k == 17:
                out.append(self.tag(" ".join(self.attrs() or [".c"])))
            elif k == 18:
                out.append("~~" + self.inline(1, label) + "~~")
            elif k == 19:
                out.append(self.pick(["  \n", "\\\n", "\n", "\t", " ", "\r"]))
            elif k == 20:
                out.append("[" + self.inline(2, True) + "][" + self.pick(["r", "a", ""]) + "]")
            elif k == 21:
                out.append("[" + self.pick(["<!-- c -->", "{% a %}", "x"]) + "](" +
                           self.pick(URLS) + ")")
            else:
                out.append(self.pick(["_e_", "__s__", "a*b*c", "&#123;", "\\{% a %}", "{{x}}"]))
        sep = self.pick([" ", " ", " ", "", "\n"])
        return sep.join(out)

    # blocks

    def para(self):
        k = self.r.randrange(5)
        if k == 0:
            return self.tag() + " " + self.inline()
        if k == 1:
            return self.inline() + " " + self.tag()
        if k == 2:
            return self.inline() + "\n" + self.tag() + "\n" + self.inline()
        return self.inline()

    def heading(self):
        text = self.inline(self.r.randrange(0, 4))
        if self.chance(0.5):
            text += " " + self.tag(" ".join(self.attrs() or ["#h"]))
        return "#" * self.r.randrange(1, 8) + self.pick([" ", " ", "", "\t"]) + text + \
            self.pick(["", "", " ##", " #"])

    def list_block(self, depth):
        bullet = self.pick(["-", "*", "+", "1.", "2)", "10."])
        items = []
        for _ in range(self.r.randrange(1, 4)):
            body = self.blocks(depth + 1, self.r.randrange(1, 3))
            pad = " " * (len(bullet) + 1)
            lines = body.split("\n")
            lazy = self.chance(0.2)
            text = bullet + " " + lines[0] + "".join(
                "\n" + ("" if lazy or not l else pad) + l for l in lines[1:])
            items.append(text)
        return self.pick(["\n", "\n", "\n\n"]).join(items)

    def blockquote(self, depth):
        body = self.blocks(depth + 1, self.r.randrange(1, 3))
        lazy = self.chance(0.2)
        return "\n".join(("> " if (not lazy or i == 0) else "") + l if l or not lazy else ">"
                         for i, l in enumerate(body.split("\n")))

    def fence(self):
        marker = self.pick(["```", "~~~", "````", "``"])
        info = self.pick(["", "js", "js " + self.tag(), self.tag(),
                          "{% process=false %}", "x {% a process=false %}",
                          "{% a process=0 %} y", "{% $v %}", "{% #i .c %}", "{% /a %}",
                          "{% a b=\"%}\" %}", "{% unclosed",
                          "{% x=" + (self.deep() if self.chance(0.1) else "[1]") + " %}",
                          "{%}", "`` {% a %}", "{% a b=😀 %}"])
        lines = []
        for _ in range(self.r.randrange(0, 5)):
            k = self.r.randrange(6)
            if k <= 1:
                lines.append(self.pick(["", "  ", "\t"]) + self.tag())
            elif k == 2:
                lines.append(self.inline())
            elif k == 3:
                lines.append(self.tag() + " text " + self.tag())
            elif k == 4:
                lines.append(self.pick(["{% a\nb=1 %}", "x {%\ny %}", "{%", "%}", "\"{% a %}\""]))
            else:
                lines.append(self.pick(["code", "  indented", "", "{{ x }}"]))
        indent = self.pick(["", "", "", " ", "    ", "  "])
        close = self.pick([marker, marker, marker, "", marker + "`", "~~~"])
        text = marker + info + "\n" + "\n".join(lines) + ("\n" + close if close else "")
        return "\n".join(indent + l for l in text.split("\n"))

    def table(self):
        cols = self.r.randrange(1, 4)
        def row():
            return "| " + " | ".join(self.pick([self.tag(), self.inline(2), "x", ""])
                                     for _ in range(cols)) + self.pick([" |", " |", ""])
        head = row()
        delim = "|" + "|".join(self.pick(["---", ":--", "--:", ":-:", "-"]) for _ in range(cols)) + "|"
        body = [row() for _ in range(self.r.randrange(0, 4))]
        return "\n".join([head, delim] + body)

    def tag_block(self, depth):
        k = self.r.randrange(6)
        name = self.pick(NAMES)
        if k <= 2:
            inner = self.blocks(depth + 1, self.r.randrange(0, 3))
            indent = self.pick(["", "", "  ", "    "])
            inner = "\n".join(indent + l if l else l for l in inner.split("\n"))
            sep = self.pick(["\n", "\n\n"])
            close = self.pick(["{% /" + name + " %}", "{% /" + name + " %}", "", "{% /other %}"])
            return self.tag(" ".join([name] + self.attrs())) + sep + inner + sep + close
        if k == 3:
            return self.tag(" ".join([name] + self.attrs()) + " /")
        if k == 4:
            return self.pick(["{% $v %}", "{% fn() %}", "{% $a.b[0] %}", "{% default($x, 1) %}",
                              "  {% $v %}", "{% @a %}"])
        return self.tag() + self.pick(["", " trailing", "  ", "\t"])

    def comment(self):
        k = self.r.randrange(6)
        if k == 0:
            return "<!-- " + self.inline(2) + " -->"
        if k == 1:
            return "<!--\n" + self.inline(2) + "\n-->"
        if k == 2:
            return "<!-- c --> after"
        if k == 3:
            return "<!-- unclosed\n" + self.inline(1)
        if k == 4:
            return self.pick(["<!---->", "<!-->", "<!--->", "  <!-- c -->", "<!-- {% a %} -->"])
        return "<!--\n\n" + self.inline(1) + "\n\n-->"

    def block(self, depth):
        k = self.r.randrange(22 if depth < 3 else 15)
        if k <= 3:
            return self.para()
        if k <= 6:
            return self.tag_block(depth)
        if k == 7:
            return self.heading()
        if k == 8:
            return self.fence()
        if k == 9:
            return self.table()
        if k == 10:
            return self.comment()
        if k == 11:
            return self.pick(["---", "***", "- - -", "___", "--- ", "----"])
        if k == 12:
            return "[" + self.pick(["r", "a", "{% x %}"]) + "]: " + self.pick(URLS[:6]) + \
                self.pick(["", " \"t\"", " 'u {% a %}'"])
        if k == 13:
            return self.pick(["    indented {% a %}", "\tcode", "<div>\n" + self.inline(1) + "\n</div>",
                              "Setext\n===", "Setext\n---", self.inline(1) + "\n    " + self.tag()])
        if k == 14:
            return self.pick(["", " ", "\t"])
        if k <= 17:
            return self.list_block(depth)
        if k <= 20:
            return self.blockquote(depth)
        return self.tag_block(depth)

    def blocks(self, depth, n):
        out = []
        for _ in range(n):
            out.append(self.block(depth))
            out.append(self.pick(["\n", "\n\n", "\n\n", "\n\n\n"]))
        return "".join(out).rstrip("\n")

    def frontmatter(self):
        body = "\n".join(self.pick(["title: x", "a: 1", "tags: [a, b]", "{% a %}", "", "  b: c",
                                    "---x", "k: \"v\""]) for _ in range(self.r.randrange(0, 4)))
        open_ = self.pick(["---", "---", "--- ", "  ---", "----", "---\t"])
        close = self.pick(["---", "---", " --- ", "...", "", "----"])
        fm = open_ + "\n" + body + ("\n" + close if close else "")
        return fm

    def document(self):
        parts = []
        if self.chance(0.3):
            parts.append(self.frontmatter() + self.pick(["\n", "\n\n", ""]))
        if self.chance(0.08):
            parts.insert(0, self.pick(["\n", "\n\n\n\n\n", "   ", "\n \n"]))
        parts.append(self.blocks(0, self.r.randrange(1, 7)))
        doc = "".join(parts)
        if self.chance(0.5):
            doc = self.mutate(doc)
        if self.chance(0.1):
            doc = doc.replace("\n", "\r\n")
        if self.chance(0.7):
            doc += "\n"
        return doc

    def mutate(self, doc):
        snippets = ["{%", "%}", "{% a %}", "\"", "\\", "\n", " ", "  ", "\t", ">", "> ", "-", "- ",
                    "|", "`", "```", "<!--", "-->", "---", "*", "#", "[", "](", ")", "$", "😀",
                    " ", "\r", "\u0000", "/%}", "{% /a %}", "\n\n", "    "]
        for _ in range(self.r.randrange(1, 4)):
            if not doc:
                doc = self.pick(snippets)
                continue
            i = self.r.randrange(len(doc))
            k = self.r.randrange(4)
            if k == 0:
                doc = doc[:i] + doc[i + 1:]
            elif k == 1:
                doc = doc[:i] + self.pick(snippets) + doc[i:]
            elif k == 2:
                j = min(len(doc), i + self.r.randrange(1, 20))
                doc = doc[:i] + doc[i:j] + doc[i:]
            else:
                lines = doc.split("\n")
                a, b = self.r.randrange(len(lines)), self.r.randrange(len(lines))
                lines[a], lines[b] = lines[b], lines[a]
                doc = "\n".join(lines)
        return doc


# --- comparing --------------------------------------------------------------------------------

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
    """Upstream's strings as a Rust string holds them: a lone surrogate as U+FFFD."""
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
    # Split on "\n" only: JSON.stringify leaves U+2028 and U+2029 unescaped, and
    # str.splitlines() would split there.
    answers = [json.loads(l) for l in done.stdout.decode("utf-8", "surrogatepass").split("\n")
               if l.strip()]
    if len(answers) != len(requests):
        raise RuntimeError(f"{command[0]}: {len(requests)} requests, {len(answers)} answers")
    return answers


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


RANGE_ERROR = "RangeError: Maximum call stack size exceeded"
TALLY = ("equal", "different", "both threw", "equal after U+FFFD",
         "nested past MAX_NESTING (node parsed)")


def compare(docs, markdoc, rust_bin, node="node"):
    """Both sides over docs ([{id, code, tokenizer}]): (tally, differences).

    Where both throw, the port's error must be upstream's: `Name: message` (TokenizeError's
    Display) ending in node's message; a panic is a difference. Where only the port throws
    and it is the RangeError of values nested past grammar::MAX_NESTING (300) while V8's
    stack let node parse them, the document is counted apart, after checking it does
    nest that deep."""
    oracle = run([node, ORACLE, markdoc],
                 [{"id": d["id"], "code": d["code"], "tokenizer": d["tokenizer"],
                   "outputs": ["tokens"]} for d in docs])
    port = run([rust_bin], [{"id": d["id"], "stage": "tokens", "code": d["code"],
                             "tokenizer": d["tokenizer"]} for d in docs])
    tally = {k: 0 for k in TALLY}
    diffs = []
    for d, o, p in zip(docs, oracle, port):
        o_has, p_has = "tokens" in o, "tokens" in p
        if p.get("threw") == RANGE_ERROR and o.get("threw") != RANGE_ERROR.split(": ")[1] \
                and bracket_depth(d["code"]) >= 300:
            # node parsed what the port refused, and went on: to tokens, or to a later throw
            tally["nested past MAX_NESTING (node parsed)"] += 1
            continue
        if not o_has and not p_has:
            if str(p.get("threw", "")).endswith(": " + str(o.get("threw"))) and \
                    not str(p.get("threw", "")).startswith("panic"):
                tally["both threw"] += 1
            else:
                tally["different"] += 1
                diffs.append({**d, "where": "threw", "oracle": o.get("threw"),
                              "port": p.get("threw")})
            continue
        if o_has != p_has:
            tally["different"] += 1
            diffs.append({**d, "where": "threw", "oracle": o.get("threw"), "port": p.get("threw")})
            continue
        a, b = o["tokens"], p["tokens"]
        diff = first_difference(a, b)
        if diff and not first_difference(lossy(a), b):
            tally["equal after U+FFFD"] += 1
            continue
        if diff:
            tally["different"] += 1
            diffs.append({**d, "where": diff[0], "oracle": diff[1], "port": diff[2]})
        else:
            tally["equal"] += 1
    return tally, diffs


def batch(args):
    seed, start, count, config, markdoc, rust_bin = args
    gen = Gen(f"{seed}:{start}")
    docs = [{"id": f"s{seed}-{start + i}", "code": gen.document(), "tokenizer": config}
            for i in range(count)]
    return compare(docs, markdoc, rust_bin)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"))
    ap.add_argument("--rust-bin")
    ap.add_argument("--config", default='{"allowIndentation": true, "allowComments": true}')
    ap.add_argument("--count", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--batch", type=int, default=5000)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--out")
    ap.add_argument("--print", type=int)
    args = ap.parse_args()
    if args.print:
        gen = Gen(f"{args.seed}:0")
        for _ in range(args.print):
            print(json.dumps(gen.document(), ensure_ascii=False))
        return
    config = json.loads(args.config)
    tasks = [(args.seed, s, min(args.batch, args.count - s), config, args.markdoc, args.rust_bin)
             for s in range(0, args.count, args.batch)]
    total = {k: 0 for k in TALLY}
    diffs = []
    with multiprocessing.Pool(args.jobs) as pool:
        for tally, d in pool.imap_unordered(batch, tasks):
            for k, v in tally.items():
                total[k] += v
            diffs += d
    if args.out:
        os.makedirs(args.out, exist_ok=True)
        with open(os.path.join(args.out, "differences.jsonl"), "w", encoding="utf-8",
                  errors="surrogatepass") as f:
            for d in diffs:
                f.write(json.dumps(d, ensure_ascii=False) + "\n")
    print(f"{json.dumps(config)} seed {args.seed}: {args.count} documents: "
          + ", ".join(f"{v} {k}" for k, v in total.items()))
    for d in diffs[:10]:
        print(f"  {d['id']}: {d['where']}: oracle {json.dumps(d['oracle'])[:150]} | "
              f"port {json.dumps(d['port'])[:150]}")
        print(f"    code {json.dumps(d['code'])[:300]}")
    sys.exit(1 if total["different"] else 0)


if __name__ == "__main__":
    main()
