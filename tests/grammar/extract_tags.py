"""Extract the content of every Markdoc tag ({% ... %}) from documents, as grammar requests.

A tag ends at the first `%}` outside a double-quoted string, with backslash escapes inside
the string, exactly as upstream's findTagEnd (src/utils.ts) decides; the scan moves on as
upstream's parseTags does. Each tag yields its content as written (between `{%` and `%}`)
and, when different, the content after JavaScript's String.prototype.trim(), which is what
Markdoc's tokenizer hands the parser.

Usage (run python with -I):
  python3 -I tests/grammar/extract_tags.py --out OUT.jsonl --prefix P PATH...
      PATH: a file or a directory (walked; node_modules and .git skipped; files that are not
            UTF-8 are skipped)
  python3 -I tests/grammar/extract_tags.py --out OUT.jsonl --prefix P --yaml tests.yaml
      every string anywhere in each case of a marktest tests.yaml
  python3 -I tests/grammar/extract_tags.py --out OUT.jsonl --prefix P --tag-test tag.test.ts
      the string literal of every parse(...) call, and of the `examples` arrays, in
      upstream's tag.test.ts

Requests are {id, content}, one per distinct content; a tally goes to stderr.
"""

import argparse
import json
import os
import re
import sys

OPEN, CLOSE = "{%", "%}"

# String.prototype.trim(): WhiteSpace and LineTerminator (ECMA-262 §12.2, §12.3).
JS_WHITESPACE = "\t\n\x0b\x0c\r \xa0        " \
                "        　﻿"


def js_trim(s):
    return s.strip(JS_WHITESPACE)


def find_tag_end(content, start=0):
    """findTagEnd from upstream src/utils.ts."""
    state = 0  # 0 normal, 1 string, 2 escape
    pos = start
    while pos < len(content):
        char = content[pos]
        if state == 1:
            if char == '"':
                state = 0
            elif char == "\\":
                state = 2
        elif state == 2:
            state = 1
        else:
            if char == '"':
                state = 1
            elif content.startswith(CLOSE, pos):
                return pos
        pos += 1
    return None


def tags(content):
    """The content of each tag, scanning as upstream's parseTags does."""
    out = []
    pos = 0
    while pos < len(content):
        if not content.startswith(OPEN, pos):
            pos += 1
            continue
        end = find_tag_end(content, pos)
        if end is None:
            pos = pos + len(OPEN) + 1
            continue
        out.append(content[pos + len(OPEN):end])
        pos = end + len(CLOSE)
    return out


def walk(paths):
    for path in paths:
        if os.path.isfile(path):
            yield path
            continue
        for root, dirs, files in os.walk(path):
            dirs[:] = sorted(d for d in dirs if d not in ("node_modules", ".git"))
            for name in sorted(files):
                yield os.path.join(root, name)


def strings_in(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from strings_in(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings_in(v)


JS_ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "b": "\b", "f": "\f", "v": "\v", "0": "\0"}


def js_unquote(body):
    """The value of a single-quoted JavaScript string literal's body."""
    out, i = [], 0
    while i < len(body):
        c = body[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        n = body[i + 1]
        if n == "x":
            out.append(chr(int(body[i + 2:i + 4], 16)))
            i += 4
        elif n == "u":
            out.append(chr(int(body[i + 2:i + 6], 16)))
            i += 6
        else:
            out.append(JS_ESCAPES.get(n, n))
            i += 2
    return "".join(out)


LITERAL = r"'((?:[^'\\\n]|\\.)*)'"


def tag_test_literals(text):
    found = [js_unquote(m.group(1)) for m in re.finditer(r"\bparse\(\s*" + LITERAL, text)]
    for m in re.finditer(r"\bexamples\s*=\s*\[([^\]]*)\]", text):
        found += [js_unquote(x.group(1)) for x in re.finditer(LITERAL, m.group(1))]
    return found


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("paths", nargs="*")
    ap.add_argument("--out", required=True)
    ap.add_argument("--prefix", required=True)
    ap.add_argument("--yaml", help="a marktest tests.yaml")
    ap.add_argument("--tag-test", help="upstream's src/grammar/tag.test.ts")
    args = ap.parse_args()

    contents = []  # (source, content) in order of appearance
    files = 0
    if args.yaml:
        import yaml  # PyYAML
        with open(args.yaml, encoding="utf-8") as f:
            cases = yaml.safe_load(f)
        for n, case in enumerate(cases):
            files += 1
            for s in strings_in(case):
                contents += [(f"case{n}", t) for t in tags(s)]
    elif args.tag_test:
        with open(args.tag_test, encoding="utf-8") as f:
            contents = [("tag.test.ts", c) for c in tag_test_literals(f.read())]
        files = 1
    else:
        for path in walk(args.paths):
            try:
                with open(path, encoding="utf-8") as f:
                    text = f.read()
            except (UnicodeDecodeError, OSError):
                continue
            if OPEN not in text:
                continue
            files += 1
            contents += [(path, t) for t in tags(text)]

    seen = set()
    requests = []
    occurrences = len(contents)
    for _, content in contents:
        variants = [content] if args.tag_test else [js_trim(content), content]
        for c in variants:
            if c not in seen:
                seen.add(c)
                requests.append(c)
    with open(args.out, "w", encoding="utf-8") as f:
        for i, c in enumerate(requests):
            f.write(json.dumps({"id": f"{args.prefix}-{i}", "content": c}, ensure_ascii=False) + "\n")
    print(f"{args.prefix}: {files} sources, {occurrences} tags, {len(requests)} distinct requests",
          file=sys.stderr)


if __name__ == "__main__":
    main()
