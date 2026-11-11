"""Grammar-based differential fuzzer for the markdown-it port.

Generates Markdown documents from a small grammar (block starts at every indentation, with
spaces and tabs, nested blockquotes and lists, fences, tables, reference definitions, HTML
blocks, headings and rules; inline delimiter runs, brackets, links and images, backticks,
entities, escapes, inline HTML, autolinks, Markdoc-like {% %} text; CRLF/CR line ends, NUL,
non-ASCII spaces, astral characters), lightly mutates some of them, runs them through both
the node oracle (oracle_tokens.cjs) and the Rust port (examples/mdit_tokens.rs) and reports
every document whose output differs. `min` shrinks a difference to a small case.

Usage (run python with -I, from the crate root):
  python3 -I tests/markdown_it/fuzz.py gen SEED COUNT OUT.jsonl
  python3 -I tests/markdown_it/fuzz.py run --markdoc DIST/index.js --rust-bin BIN
      [--seed S] [--count N] [--batch B] [--configs default,markdoc-ai,markdoc-noai]
      [--failures OUT.jsonl]
  python3 -I tests/markdown_it/fuzz.py min --markdoc DIST/index.js --rust-bin BIN
      FAILURES.jsonl OUT.jsonl

`run` streams: nothing but the requests of one batch is written to disk. A failure line is
{config, id, src, path, node, rust}; `min` keeps, for each, the smallest source on which the
two sides still differ (line- then character-level delta debugging, each round one batch
through both programs) and writes {id, src, config} lines: once the difference is fixed,
add a `why` and append the case to regressions.jsonl (then regenerate with golden.py).
"""

import argparse
import json
import os
import random
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True  # no __pycache__ in the crate
from compare import first_difference  # noqa: E402  (same comparison as compare.py)

# The configurations of the verification matrix, as request fields.
CONFIGS = {
    "default": {},
    "markdoc-ai": {"disable": ["lheading", "code"], "options": {"allowIndentation": True}},
    "markdoc-noai": {"disable": ["lheading", "code"], "options": {"allowIndentation": False}},
    "commonmark": {"preset": "commonmark"},
    "zero": {"preset": "zero"},
    "typographer": {"options": {"typographer": True}},
    "html": {"options": {"html": True}},
}
# Extra configurations `run` (and matrix.py) also accept, not in the golden files: inline
# mode, Markdoc's configuration with html or typographer, a low nesting limit, and the
# quotes option in its string and array forms.
EXTRA_CONFIGS = {
    "markdoc-html": {"disable": ["lheading", "code"], "options": {"allowIndentation": True, "html": True}},
    "markdoc-typographer": {"disable": ["lheading", "code"], "options": {"allowIndentation": True, "typographer": True}},
    "inline": {"mode": "parseInline"},
    "inline-typographer": {"mode": "parseInline", "options": {"typographer": True}},
    "nest4": {"options": {"maxNesting": 4}},
    "markdoc-nest4": {"disable": ["lheading", "code"], "options": {"allowIndentation": True, "maxNesting": 4}},
    "quotes": {"options": {"typographer": True, "quotes": "«»„“"}},
    "quotes-array": {"options": {"typographer": True,
                                 "quotes": ["«\u00a0", "\u00a0»", "‹\u00a0", "\u00a0›"]}},
}

# ---------------------------------------------------------------------------------------
# The grammar

WORDS = ["foo", "bar", "baz", "a", "b", "x1", "Foo", "αβγ", "日本語", "é", "naïve", "ὐ",
         "İ", "ǅ", "ß", "ẞ", "ﬀ", "Σσς", "ΑΣ", "Ⓐ", "word", "q", "ab_cd", "snake_case_x",
         "2*3*4", "\U0001F600", "\U0001D400x", "a\u0301", "\u200bz", "\U00020000"]
PUNCT = list("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~") + [
    "¡", "§", "«", "»", "¿", "‐", "–", "—", "‘", "’", "“", "”", "…", "、", "。", "「",
    "！", "（", "）", "°", "€", "©", "™", "←", "∑", "☃", "\U00010100", "\U0001F4A9",
    "\U0001F10D", "\u0300", "¦", "^", "\U0001D7CE"]
SPACES = [" ", " ", " ", "  ", "\t", "\u00a0", "\u3000", "\u2000", "\u2028", "\u202f",
          "\u205f", "\ufeff", "\u1680", "\u000b", "\u000c", "\u0085", "\u200a"]
LABELS = ["foo", "Foo", "FOO", "bar baz", "bar  baz", "ẞ", "ss", "SS", "ß", "Σ", "ς",
          "a\\]b", " x ", "x\ny", "\U0001F600", "ǅ", "1", "*em*", "[n]", "`c`", "ΑΣ",
          "\u00a0nb", "a\tb", "İ", "i\u0307"]
DESTS = ["", "/url", "/föö", "<>", "<a b>", "<a\\>b>", "a(b)c", "a(b", "(x)", "/a%20b",
         "/a%zz", "%E0%A4%A", "http://example.com/a?b=1&c=2#d", "http://bücher.de/x",
         "http://xn--bcher-kva.de", "HTTP://EXAMPLE.COM", "mailto:a@b.c", "javascript:x()",
         "JAVAscript:x", "data:image/png;base64,AA", "data:text/html,x", "vbscript:x",
         "file:///etc", "#frag", "?q", "\\(x\\)", "a\\ b", "&amp;x", "&#x41;", "u\U0001F600",
         "http://[::1]:80/", "http://a:b@c.d/", "//host/p", "<\U0001F600>", "a\"b", "x'y",
         "http://例え.テスト/", "http://xn--9-.com", "http://xn--a.b", "ftp://x.y/%",
         "http://-a-.b", "/\u00a0", "<a\nb>", "a]b", "<a<b>", "\\", "<", ">x",
         "(" * 31 + "x" + ")" * 31, "(" * 32 + "x" + ")" * 32, "(" * 33 + "x" + ")" * 33,
         "((a)\\)", "a\\(b(c)"]
TITLES = ["", " \"t\"", " 't'", " (t)", " \"a\\\"b\"", " 'a\nb'", " \"unclosed", " (a(b)",
          " (a\\)b)", "\n\"next line\"", " \"&amp; \U0001F600\"", "\"tight\"", " \"\"",
          " \"x\" junk", "\t'tab'"]
ENTITIES = ["&amp;", "&AMP;", "&Amp;", "&nbsp;", "&copy", "&copy;", "&#35;", "&#x23;", "&#X23;",
            "&#0;", "&#1114111;", "&#1114112;", "&#xD800;", "&#xdfff;", "&#x1F600;",
            "&#99999999;", "&#123456789;", "&#xFFFFFF;", "&bogus;", "&;", "&#;", "&#x;",
            "&ThickSpace;", "&ngE;", "&lt", "&amp", "&quot;", "&apos;", "&#65;", "&#x0;",
            "&#xFFFE;", "&#xFDD0;", "&#8;", "&#127;", "&#128;", "&#159;", "&fjlig;",
            "&CounterClockwiseContourIntegral;", "&nbsp", "&#00065;", "&#x0000041;"]
INLINE_HTML = ["<a>", "<a href=\"x\">", "</a>", "<span class=\"x\" id='y' data-z=w>", "</span>",
               "<br/>", "<br />", "<!-- c -->", "<!-->", "<!--->", "<!-- a -- b -->",
               "<!-- x ->", "<?php echo 1 ?>", "<!DOCTYPE html>", "<![CDATA[x]]>",
               "<x y=\">\">", "<1>", "< a>", "<a b='c>", "<a\nb=c>", "<A-1 b=c/>", "</a b>",
               "<a b=c\u00a0d>", "<a\u3000b>", "<x:y>", "<img src=x onerror=y>", "<b>", "</b>",
               "<?x", "<!x", "<![CDATA[", "<!--"]
AUTOLINKS = ["<http://a.b>", "<HTTPS://x/y?z>", "<mailto:foo@bar.com>", "<foo@bar.example.com>",
             "<a+b@c.d>", "<irc://foo.bar:2233/baz>", "<made-up-scheme://foo,bar>", "<a:b>",
             "<ab:c>", "<x@y>", "<http://x y>", "<http://bücher.de>", "<http://xn--bcher-kva.de>",
             "<http://[::1]/>", "<ftp://a%20b>", "<http://a.b/%zz>", "<javascript:alert(1)>",
             "<http://a.b/\U0001F600>", "<HTTP://Ex.COM/É>", "<a.b-c_d@e-f.g>", "<-a@b.c>",
             "<a@-b.c>", "<a@b-.c>", "<" + "x" * 33 + ":y>", "<" + "x" * 32 + ":y>",
             "<a@" + "b" * 63 + ".c>", "<a@" + "b" * 64 + ".c>", "<http://a\\b>",
             "<dat\ta:x>", "<data:x>", "<http://a.b/[c]>", "<http://a.b/`c`>"]
MARKDOC = ["{% tag %}", "{% /tag %}", "{% tag a=\"b\" /%}", "{% if $x %}", "{% else /%}",
           "{% /if %}", "{{ $x }}", "{% $var %}", "{%", "%}", "{% table %}", "{% /table %}",
           "{% partial file=\"x.md\" /%}", "{% callout type=\"note\" %}", "{% #id .cls %}",
           "{% x | y %}", "{% a b=[1,2] c={d: 1} %}", "{%- x -%}", "{% '*a*' %}"]
TYPOGRAPHY = ["\"quoted\"", "'single'", "don't", "(c)", "(C)", "(tm)", "(TM)", "(r)", "(R)",
              "(p)", "+-", "...", "?....", "!!!!!", ",,", "--", "---", "a--b", "5'10\"",
              "\"'nested'\"", "'\"x\"'", "a...b", "-- -", "x---y", "1990's", "'90s"]


class Gen:
    def __init__(self, rng):
        self.r = rng
        self.labels = []

    # helpers
    def pick(self, seq):
        return seq[self.r.randrange(len(seq))]

    def chance(self, p):
        return self.r.random() < p

    def n(self, a, b):
        return self.r.randint(a, b)

    def space(self):
        return self.pick(SPACES) if self.chance(0.12) else " "

    def label(self):
        if self.labels and self.chance(0.6):
            return self.pick(self.labels)
        return self.pick(LABELS) if self.chance(0.8) else self.text(1)

    # inline grammar
    def word(self):
        r = self.r.random()
        if r < 0.75:
            return self.pick(WORDS)
        if r < 0.9:
            return self.pick(PUNCT)
        return self.pick(PUNCT) + self.pick(WORDS) + self.pick(PUNCT)

    def text(self, k):
        return self.space().join(self.word() for _ in range(k))

    def delims(self, inner):
        d = self.pick(["*", "**", "***", "_", "__", "___", "~~", "~", "~~~", "****", "*_", "_*",
                       "**_", "~~*"])
        close = d[::-1] if self.chance(0.7) else self.pick(["*", "**", "_", "~~", ""])
        lpad = self.pick(["", "", "", " ", self.pick(PUNCT), self.pick(WORDS)])
        rpad = self.pick(["", "", "", " ", self.pick(PUNCT), self.pick(WORDS)])
        return lpad + d + inner + close + rpad

    def code_span(self):
        k = self.n(1, 4)
        inner = self.pick(["a", " a ", "  ", " ", "`", "``", "a`b", " `` ", "\n", "x\ny", "|",
                           "\\", "*a*", "<b>", "&amp;", "\U0001F600", "a  b", "\t"])
        close = k if self.chance(0.8) else self.n(1, 4)
        return "`" * k + inner + "`" * close

    def link(self, depth):
        text = self.inline(depth + 1, self.n(0, 3))
        bang = "!" if self.chance(0.3) else ""
        form = self.r.random()
        if form < 0.45:
            ws1 = self.pick(["", "", " ", "\n", "\t", "  "])
            ws2 = self.pick(["", "", " ", "\n"])
            close = ")" if self.chance(0.9) else self.pick(["", "]", " )"])
            return f"{bang}[{text}]({ws1}{self.pick(DESTS)}{self.pick(TITLES)}{ws2}{close}"
        if form < 0.65:
            return f"{bang}[{text}][{self.label()}]"
        if form < 0.75:
            return f"{bang}[{text}][]"
        if form < 0.9:
            return f"{bang}[{self.label()}]"
        return self.pick(["[", "]", "![", "[]", "[[", "]]", "][", "]("]) + text

    def inline_atom(self, depth):
        r = self.r.random()
        if depth > 3:
            r = r * 0.45
        if r < 0.35:
            return self.text(self.n(1, 3))
        if r < 0.40:
            return self.pick(ENTITIES)
        if r < 0.45:
            return "\\" + self.pick(PUNCT + ["a", " ", "\n", "\\", "\t", "é"])
        if r < 0.55:
            return self.delims(self.inline(depth + 1, self.n(0, 2)))
        if r < 0.60:
            return self.code_span()
        if r < 0.70:
            return self.link(depth)
        if r < 0.74:
            return self.pick(AUTOLINKS)
        if r < 0.79:
            return self.pick(INLINE_HTML)
        if r < 0.83:
            return self.pick(MARKDOC)
        if r < 0.87:
            return self.pick(TYPOGRAPHY)
        if r < 0.91:
            return self.pick(["  \n", "\\\n", "\n", "   \n", "\t\n", "\u0000", "\r", " \n "])
        if r < 0.95:
            return self.pick(["*" * self.n(3, 40), "_" * self.n(3, 12), "[" * self.n(3, 30),
                              "]" * self.n(2, 8), "`" * self.n(3, 12), "~" * self.n(3, 9),
                              "![" * self.n(2, 12), "](" * self.n(2, 6), "<" * self.n(2, 6),
                              "&" * self.n(2, 4), "\\" * self.n(2, 7)])
        return self.pick(SPACES)

    def inline(self, depth=0, k=None):
        k = self.n(1, 6) if k is None else k
        parts = []
        for _ in range(k):
            parts.append(self.inline_atom(depth))
            if self.chance(0.6):
                parts.append(self.space())
        return "".join(parts)

    def one_line(self, depth=0):
        return self.inline(depth).replace("\n", " ").replace("\r", " ")

    # block grammar: each block is a list of lines (no line ends)
    def indent(self):
        r = self.r.random()
        if r < 0.65:
            return ""
        if r < 0.85:
            return " " * self.n(1, 3)
        return self.pick(["    ", "     ", "\t", " \t", "  \t", "   \t", "\t\t", "\t ", "      ",
                          "        ", "\u00a0", "\u3000"])

    def paragraph(self, depth):
        return [self.indent() + self.one_line(depth) for _ in range(self.n(1, 3))]

    def atx(self, depth):
        hashes = "#" * self.pick([1, 1, 2, 3, 4, 5, 6, 7])
        sep = self.pick([" ", " ", "  ", "\t", "", " \t"])
        close = self.pick(["", "", "", " #", " ###", "#", " \\#", " # ", " #x", "  ##  "])
        body = self.one_line(depth) if self.chance(0.85) else ""
        return [self.indent() + hashes + sep + body + close]

    def setext(self, depth):
        under = self.pick(["===", "---", "=", "-", "  ==  ", "= =", "---   ", "-----x", "\t===",
                           "    ---", "==\t"])
        return self.paragraph(depth) + [self.indent() + under]

    def hr(self, depth):
        return [self.indent() + self.pick(["***", "---", "___", " - - -", "*\t*\t*", "_ _ _ _",
                                           "**", "--- a", "-\t-\t-", "* * * * *", "___x",
                                           "+++", "= = ="])]

    def fence(self, depth):
        ch = self.pick(["`", "~"])
        k = self.n(3, 5)
        ind = self.pick(["", "", " ", "  ", "   ", "    ", "\t"])
        info = self.pick(["", "", "js", " js ", "a`b", "a~b", "{% x %}", "x y z", "&amp;",
                          "\\`", "\U0001F600", " \t ", "js{.cls}"])
        body = []
        for _ in range(self.n(0, 4)):
            body.append(self.pick(["", "  code", "\tx", ch * self.n(2, 6), " " + ch * k,
                                   "    " + ch * k, self.one_line(depth), "<div>", "# h",
                                   "- li", "> q", ch * k + " x", "\u0000"]))
        lines = [ind + ch * k + info] + body
        if self.chance(0.75):
            lines.append(self.pick(["", " ", "  ", "   ", "    "]) + ch * self.pick([k, k, k + 1, k - 1]) +
                         self.pick(["", "", " ", "\t", "x"]))
        return lines

    def code(self, depth):
        lines = []
        for _ in range(self.n(1, 4)):
            lines.append(self.pick(["    ", "\t", "     ", "  \t", "\t\t", "      "]) +
                         (self.one_line(depth) if self.chance(0.8) else ""))
        return lines

    def table(self, depth):
        cols = self.n(1, 4)
        lead = self.chance(0.6)
        trail = self.chance(0.6)

        def row(cells):
            s = " | ".join(cells)
            sep = self.pick(["|", "| ", " |"])
            return self.indent() + (sep if lead else "") + s + (sep[::-1] if trail else "")

        def cell():
            return self.pick([self.text(1), self.one_line(3), "", "\\|", "`a|b`", "a \\| b",
                              " ", "**x**", "[a](b)", "\u00a0", "&#124;", "\\\\|"])

        aligns = [self.pick(["---", ":--", "--:", ":-:", "-", ":", " --- ", "--- x", "-\t-",
                             "::", ":-", "---|"]) for _ in range(cols + self.pick([0, 0, 0, 1, -1]))]
        lines = [row([cell() for _ in range(cols)]), row(aligns)]
        for _ in range(self.n(0, 4)):
            lines.append(row([cell() for _ in range(cols + self.pick([0, 0, 1, -1]))]))
        if self.chance(0.2):
            lines.append(self.pick(["", "> q", "- li", "# h", "    code", "|", "x"]))
        return lines

    def refdef(self, depth):
        lab = self.label()
        self.labels.append(lab)
        dest = self.pick(DESTS)
        sep1 = self.pick([" ", " ", "\t", "\n", "  ", ""])
        title = self.pick(TITLES)
        lines = (self.indent() + f"[{lab}]:" + sep1 + dest + title +
                 self.pick(["", "", " ", " junk", "\t"])).split("\n")
        if self.chance(0.2):
            lines.append(self.pick(["\"title on next line\"", "'t'", "(t)", "junk", ""]))
        return lines

    def html_block(self, depth):
        r = self.r.random()
        if r < 0.25:
            tag = self.pick(["script", "pre", "style", "textarea", "SCRIPT", "Pre"])
            open_ = f"<{tag}" + self.pick([">", " a=b>", "", "\t>", "/>"])
            body = [self.one_line(depth) for _ in range(self.n(0, 3))]
            close = self.pick([f"</{tag}>", f"x </{tag}> y", "", f"</{tag.upper()}>", "</pre"])
            return [self.indent() + open_] + body + ([close] if close else [])
        if r < 0.4:
            return [self.indent() + self.pick(["<!--", "<!-- x", "<!---->", "<!-- a -->", "<!-->"])] + \
                [self.one_line(depth) for _ in range(self.n(0, 2))] + \
                [self.pick(["-->", "x -->", "", "--->", "- ->"])]
        if r < 0.5:
            return [self.indent() + self.pick(["<?", "<?php", "<!DOCTYPE html>", "<!doctype x",
                                               "<![CDATA[", "<!X"]),
                    self.one_line(depth), self.pick(["?>", "]]>", ">", ""])]
        if r < 0.8:
            tag = self.pick(["div", "DIV", "table", "p", "h1", "ul", "li", "section", "td",
                             "colgroup", "details", "summary", "iframe", "/div", "/table",
                             "address", "nav", "source", "track"])
            lines = [self.indent() + f"<{tag}" + self.pick([">", " class=\"x\">", "", " ", "/>",
                                                            "\t", " x", ">text"])]
            lines += [self.one_line(depth) for _ in range(self.n(0, 3))]
            return lines
        lines = [self.indent() + self.pick(["<a href='x'>", "</custom>", "<x-y z>", "<a b=c d>",
                                            "<span>", "</em>", "<a/>", "<x y='>'>", "<a>  ",
                                            "<a> x", "<b\nc>", "<a b=c\u00a0d>"])]
        lines += [self.one_line(depth) for _ in range(self.n(0, 2))]
        return lines

    def markdoc(self, depth):
        lines = [self.indent() + self.pick(MARKDOC)]
        if self.chance(0.5):
            lines += self.blocks(depth + 1, self.n(1, 2))
            lines.append(self.indent() + self.pick(MARKDOC))
        return lines

    def blank(self, depth):
        return [self.pick(["", "", " ", "\t", "  ", "    ", "\u00a0"])]

    def junk(self, depth):
        return ["".join(self.pick(PUNCT + SPACES + WORDS + ["\u0000", "\r"]) for _ in range(self.n(1, 12)))]

    def blockquote(self, depth):
        inner = self.blocks(depth + 1, self.n(1, 4))
        lines = []
        for i, line in enumerate(inner):
            if i > 0 and line and self.chance(0.15):
                lines.append(line)  # lazy continuation
            else:
                lines.append(self.pick(["", "", " ", "  ", "   ", "    "]) +
                             self.pick([">", "> ", "> ", ">  ", ">\t", "> \t", ">>", "> > "]) + line)
        return lines

    def list_(self, depth):
        ordered = self.chance(0.4)
        if ordered:
            start = self.pick([0, 1, 1, 1, 2, 9, 10, 99, 123456789, 999999999, 1234567890])
            delim = self.pick([".", ".", ")"])
        else:
            bullet = self.pick(["-", "*", "+"])
        lines = []
        ind = self.pick(["", "", "", " ", "  ", "   ", "    ", "\t"])
        for i in range(self.n(1, 4)):
            marker = (str(start + i if self.chance(0.8) else start) + delim) if ordered else \
                (bullet if self.chance(0.85) else self.pick(["-", "*", "+"]))
            pad = self.pick([" ", " ", " ", "  ", "   ", "    ", "     ", "\t", " \t", "\t\t"])
            content = self.blocks(depth + 1, self.n(0, 3))
            width = len(ind) + len(marker) + len(pad)
            if not content:
                lines.append(ind + marker + self.pick(["", " ", "   "]))
            else:
                lines.append(ind + marker + pad + content[0])
                for line in content[1:]:
                    r = self.r.random()
                    if not line:
                        lines.append(line)
                    elif r < 0.6:
                        lines.append(" " * width + line)
                    elif r < 0.7:
                        lines.append("\t" * max(1, (width + 3) // 4) + line)
                    elif r < 0.8:
                        lines.append(" " * max(0, width - 1) + line)
                    elif r < 0.9:
                        lines.append(" " * (width + 1) + line)
                    else:
                        lines.append(line)  # lazy
            if self.chance(0.25):
                lines.append("")
        return lines

    def deep(self, depth):
        k = self.n(18, 110)
        r = self.r.random()
        if r < 0.3:
            return [">" * k + " " + self.one_line(3)]
        if r < 0.5:
            return ["".join(self.pick(["- ", "* ", "1. ", "> "]) for _ in range(k)) + "x"]
        if r < 0.7:
            return [" " * i + "- " + self.text(1) for i in range(0, k, 2)]
        if r < 0.85:
            return [self.pick(["[", "![", "*", "_", "~~", "<a>", "`"]) * k + "x" +
                    self.pick(["]", "](/u)", "*", "_", "~~", "</a>", "`"]) * self.n(0, k)]
        return ["  " * i + self.pick(["1. ", "- ", "> "]) + "x" for i in range(k)]

    BLOCKS = [("paragraph", 20), ("atx", 6), ("setext", 4), ("hr", 3), ("fence", 5), ("code", 3),
              ("blockquote", 7), ("list_", 9), ("table", 6), ("refdef", 5), ("html_block", 5),
              ("blank", 7), ("markdoc", 4), ("junk", 2), ("deep", 0.4)]

    def block(self, depth):
        kinds = [(k, w) for k, w in self.BLOCKS if depth < 4 or k not in ("blockquote", "list_", "markdoc")]
        total = sum(w for _, w in kinds)
        x = self.r.random() * total
        for k, w in kinds:
            x -= w
            if x < 0:
                return getattr(self, k)(depth)
        return self.paragraph(depth)

    def blocks(self, depth, k):
        lines = []
        for _ in range(k):
            lines += self.block(depth)
            if self.chance(0.45):
                lines.append(self.pick(["", "", " ", "\t"]))
        return lines

    def mutate(self, s):
        noise = PUNCT + SPACES + ["\n", "\n\n", "\r\n", "\u0000", "    ", "\t", "> ", "- ", "1. ",
                                  "```", "|", "[", "]", "`", "*", "_", "<", "&", "\\"]
        chars = list(s)
        for _ in range(self.n(1, 4)):
            p = self.n(0, len(chars))
            r = self.r.random()
            if r < 0.45:
                chars[p:p] = list(self.pick(noise))
            elif r < 0.7 and chars:
                del chars[p:p + self.n(1, 6)]
            elif r < 0.85 and chars:
                q = self.n(0, len(chars))
                a, b = min(p, q), min(max(p, q), min(p, q) + 40)
                chars[p:p] = chars[a:b]
            elif chars:
                chars[min(p, len(chars) - 1)] = self.pick(noise)
        return "".join(chars)

    def document(self):
        self.labels = []
        lines = self.blocks(0, self.n(1, 9))
        nl = self.pick(["\n"] * 10 + ["\r\n", "\r", "mixed"])
        if nl == "mixed":
            s = "".join(line + self.pick(["\n", "\n", "\r\n", "\r"]) for line in lines)
        else:
            s = nl.join(lines) + (nl if self.chance(0.7) else "")
        if self.chance(0.3):
            s = self.mutate(s)
        return s


def generate(seed, count):
    rng = random.Random(seed)
    g = Gen(rng)
    for i in range(count):
        yield f"f{seed}-{i}", g.document()


# ---------------------------------------------------------------------------------------
# Running both sides

def request(doc_id, src, config):
    req = dict(CONFIGS[config] if config in CONFIGS else EXTRA_CONFIGS[config])
    req.update({"id": doc_id, "src": src})
    return req


def run_batch(markdoc, rust_bin, reqs):
    """Run requests through both programs, streaming; yield (req, node_line, rust_line)."""
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".jsonl", delete=False,
                                     dir=os.environ.get("FUZZ_TMP")) as f:
        for req in reqs:
            f.write(json.dumps(req, ensure_ascii=False) + "\n")
        path = f.name
    try:
        with open(path, "rb") as a, open(path, "rb") as b:
            node = subprocess.Popen(["node", os.path.join(HERE, "oracle_tokens.cjs"), markdoc],
                                    stdin=a, stdout=subprocess.PIPE)
            rust = subprocess.Popen([rust_bin], stdin=b, stdout=subprocess.PIPE)
            for req in reqs:
                yield req, node.stdout.readline(), rust.stdout.readline()
            node.wait()
            rust.wait()
    finally:
        os.remove(path)


def differ(node_line, rust_line):
    """None when both outputs agree (both errors count as agreement), else (path, node, rust)."""
    if node_line == rust_line:
        return None
    if not node_line or not rust_line:
        return ("output", "<missing>" if not node_line else "present",
                "<missing>" if not rust_line else "present")
    a, b = json.loads(node_line), json.loads(rust_line)
    if "error" in a or "error" in b:
        if "error" in a and "error" in b:
            return None
        return ("error", a.get("error", "<tokens>"), b.get("error", "<tokens>"))
    return first_difference({"tokens": a["tokens"], "env": a.get("env")},
                            {"tokens": b["tokens"], "env": b.get("env")}, "")


def cmd_run(args):
    configs = args.configs.split(",")
    total = {c: [0, 0, 0] for c in configs}  # docs, differ, both-error
    out = open(args.failures, "a", encoding="utf-8") if args.failures else None
    done = 0
    docs = generate(args.seed, args.count)
    while done < args.count:
        batch = []
        for _ in range(min(args.batch, args.count - done)):
            batch.append(next(docs))
        done += len(batch)
        reqs = [request(f"{c}:{i}", src, c) for i, src in batch for c in configs]
        for req, nl, rl in run_batch(args.markdoc, args.rust_bin, reqs):
            config = req["id"].split(":", 1)[0]
            total[config][0] += 1
            d = differ(nl, rl)
            if d is None and b'"error":' in nl:  # only an {id, error} line has that key unescaped
                total[config][2] += 1
            if d:
                total[config][1] += 1
                if out:
                    out.write(json.dumps({"config": config, "id": req["id"], "src": req["src"],
                                          "path": d[0], "node": d[1], "rust": d[2]},
                                         ensure_ascii=False) + "\n")
                    out.flush()
        print(f"seed {args.seed}: {done}/{args.count} docs  " +
              "  ".join(f"{c} differ {total[c][1]}" for c in configs), file=sys.stderr, flush=True)
    for c in configs:
        print(f"seed {args.seed} config {c}: docs {total[c][0]}  differ {total[c][1]}  both-error {total[c][2]}")
    return 1 if any(t[1] for t in total.values()) else 0


# ---------------------------------------------------------------------------------------
# Minimising

def fails(args, config, candidates):
    """For each candidate source: do the two sides differ on it?"""
    reqs = [request(f"{config}:{i}", s, config) for i, s in enumerate(candidates)]
    return [differ(nl, rl) is not None for _, nl, rl in run_batch(args.markdoc, args.rust_bin, reqs)]


def ddmin(args, config, items, join):
    """Delta debugging (Zeller): a smallest-found sublist of items on which the sides differ."""
    n = 2
    while len(items) >= 2:
        size = max(1, len(items) // n)
        chunks = [items[i:i + size] for i in range(0, len(items), size)]
        subsets = [c for c in chunks]
        complements = [[x for j, c in enumerate(chunks) if j != i for x in c] for i in range(len(chunks))]
        cands = subsets + complements
        res = fails(args, config, [join(c) for c in cands])
        hit = next((i for i, r in enumerate(res) if r), None)
        if hit is None:
            if n >= len(items):
                break
            n = min(len(items), n * 2)
        elif hit < len(subsets):
            items, n = cands[hit], 2
        else:
            items, n = cands[hit], max(n - 1, 2)
    return items


def cmd_min(args):
    seen = set()
    with open(args.failures, encoding="utf-8") as f, open(args.out, "a", encoding="utf-8") as out:
        for line in f:
            fail = json.loads(line)
            config, src = fail["config"], fail["src"]
            if not fails(args, config, [src])[0]:
                print(f"{fail['id']}: no longer differs", file=sys.stderr)
                continue
            lines = src.splitlines(keepends=True)
            lines = ddmin(args, config, lines, "".join)
            chars = ddmin(args, config, list("".join(lines)), "".join)
            small = "".join(chars)
            if (config, small) in seen:
                continue
            seen.add((config, small))
            out.write(json.dumps({"id": f"min-{fail['id']}", "src": small, "config": config},
                                 ensure_ascii=False) + "\n")
            out.flush()
            print(f"{fail['id']} ({config}): {len(src)} -> {len(small)}: {json.dumps(small, ensure_ascii=False)}",
                  file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("gen")
    g.add_argument("seed", type=int)
    g.add_argument("count", type=int)
    g.add_argument("out")
    r = sub.add_parser("run")
    r.add_argument("--markdoc", required=True)
    r.add_argument("--rust-bin", required=True)
    r.add_argument("--seed", type=int, default=1)
    r.add_argument("--count", type=int, default=10000)
    r.add_argument("--batch", type=int, default=5000)
    r.add_argument("--configs", default="default,markdoc-ai,markdoc-noai")
    r.add_argument("--failures")
    m = sub.add_parser("min")
    m.add_argument("--markdoc", required=True)
    m.add_argument("--rust-bin", required=True)
    m.add_argument("failures")
    m.add_argument("out")
    args = ap.parse_args()
    if args.cmd == "gen":
        with open(args.out, "w", encoding="utf-8") as f:
            for doc_id, src in generate(args.seed, args.count):
                f.write(json.dumps({"id": doc_id, "src": src}, ensure_ascii=False) + "\n")
        return 0
    if args.cmd == "run":
        return cmd_run(args)
    return cmd_min(args)


if __name__ == "__main__":
    sys.exit(main())
