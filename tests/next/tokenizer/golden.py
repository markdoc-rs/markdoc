"""Write tests/next/tokenizer/golden.jsonl: upstream's tokens for the tokenizer's regression set.

The set is the probes below (each quirk of upstream's tokenizer plugins the port reproduces,
under the configurations where it shows) plus the documents minimise.py reduced the
fuzzer's differences to (`--minimised FILE`, the minimised.jsonl beside this script). For
each case node runs upstream's Tokenizer (oracle_tokens.cjs) and the line written is
{id, code, tokenizer, tokens} or {id, code, tokenizer, threw: {name, message}}. A lone
surrogate in upstream's strings (an error message quoting half an emoji) is written as
U+FFFD, as the port holds it, and the line marked "lossy": true. tests/next_tokenizer.rs
reads the file and needs no node.

Usage (python3 -I, from anywhere):
  python3 -I tests/next/tokenizer/golden.py --markdoc DIST/index.js \
      [--minimised tests/next/tokenizer/minimised.jsonl] [--out tests/next/tokenizer/golden.jsonl]
"""

import argparse
import json
import os
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))

TT = {"allowIndentation": True, "allowComments": True}
TF = {"allowIndentation": True, "allowComments": False}
FT = {"allowIndentation": False, "allowComments": True}
FF = {}
LV = {"allowLinkValidation": True}
LVC = {"allowLinkValidation": True, "linkValidationOptions": {"validatedProtocols": ["vscode", "h"]}}
ALL4 = [TT, TF, FT, FF]

DEEP = lambda n: "[" * (n - 1) + "1" + "]" * (n - 1)  # noqa: E731  n values deep

# (id, code, configurations)
PROBES = [
    # block rule: a tag must end its line; the trim of the whole source before it (leading
    # blank lines count) lets text after the tag through, and drops it
    ("block-trailing-text", "{% a %} xyz\n", [FF]),
    ("block-leading-blank-lines-drop-text", "\n\n\n\n\n{% a %} xyz", [FF]),
    ("block-leading-blank-lines-short", "\n\n\n{% a %} xyz", [FF]),
    ("block-multiline-tag", "{% a\n  b=1\n  c=\"x\" %}\ntext\n{% /a %}\n", ALL4),
    ("block-tag-spans-past-blockquote", "> {% a\nb=1 %}\n> c\n", [TT, FF]),
    ("block-tag-in-blockquote-markers-stripped", "> {% a b=\"x\"\n>  c=1 %}\n> d\n", [TT]),
    ("block-variable-is-inline", "{% $v %}\n", [FF]),
    ("block-function-is-a-block-token", "{% fn(1) %}\n", [FF]),
    ("block-error-location", "para\n\n  {% a = %}\n", [FF]),
    ("block-empty-and-overlapping", "{%}\n\n{%%}\n\n{% %}\n", [FF]),
    ("block-unclosed", "{% a\n\ntext %\n", [FF]),
    ("block-close-at-top-level", "{% /b %}\n\npara\n", [FF]),
    ("block-interrupts-paragraph", "para\n{% a /%}\nmore\n", [FF]),
    ("block-interrupts-blockquote", "> q\n{% a /%}\n", [FF]),
    ("block-in-list-indented", "- a\n\n      {% b %}\n      c\n      {% /b %}\n", [TT, FF]),
    ("block-maxnesting-reached", "{% a %}\n" * 101 + "x\n", [FF]),
    ("block-percent-close-in-string", "{% a b=\"%}\" c=\"{%\" %}\n", [FF]),
    ("block-js-whitespace-trim", " {% a %}　\n", [FF]),
    ("block-nul-in-tag", "{% a b=\"\u0000\" %}\n", [FF]),
    # inline rule
    ("inline-tags-and-markdown", "a {% b %}*c*{% /b %} {% #id .c %} {% $v.w[0] %}\n", [FF]),
    ("inline-close-without-open", "a {% /b %} *c*\n", [FF]),
    ("inline-error-has-no-location", "a {% = %} b\n", [FF]),
    ("inline-emoji-error-lossy", "a {% 😀 %} b\n", [FF]),
    ("inline-tag-in-code-span", "`{% a %}` {% b %}\n", [FF]),
    ("inline-tag-in-link-text-and-destination", "[{% a %}](u) [x]({% b %}) <https://x/{% c %}>\n", [FF]),
    ("inline-tag-end-past-label", "[a {% b ](u) %} c\n", [FF]),
    ("inline-escaped-open", "\\{% a %} {\\% b %}\n", [FF]),
    ("inline-tag-in-heading-and-table", "# h {% #i %}\n\n| a {% b /%} | {% .c %} |\n|---|---|\n| {% d %} | e |\n", [FF]),
    ("inline-unbalanced-in-link-restored", "[a {% /b %}](u) {% x /%} *c*\n", [FF]),
    ("inline-unbalanced-under-open-tag", "{% a %}[x {% /b %}](u) *c*\n", [FF]),
    # fences: info string and content
    ("fence-info-tag", "```js {% a b=1 .c %}\nx\n```\n", [FF]),
    ("fence-info-annotation-and-variable", "``` {% #i .c %}\nx\n```\n\n``` {% $v %}\ny\n```\n", [FF]),
    ("fence-info-error", "```js {% a = %}\nx\n```\n", [FF]),
    ("fence-info-unclosed", "```js {% a b=1\nx\n```\n", [FF]),
    ("fence-process-false", "``` {% process=false %}\n{% a %}\n```\n", [FF]),
    ("fence-process-falsy-values", "".join(
        f"``` {{% p process={v} %}}\n{{% a %}}\n```\n\n"
        for v in ["0", "\"\"", "null", "true", "1", "$x", "[]", "\"no\"", "-0"]), [FF]),
    ("fence-content-whitespace-rule", "```\n  {% a %}  \nfoo {% b %}\n  {% c %}  \n```\n", [FF]),
    ("fence-first-line-and-no-trailing-newline", "```\n{% a %}", [FF]),
    ("fence-multiline-tag-lines", "```\n{% a\nb=1 %}\nline {% c %}\n```\n", [FF]),
    ("fence-unclosed-open-skips-newline", "```\n{%\n{% a %}\nx\n```\n", [FF]),
    ("fence-errors-and-positions", "```\nx {% = %} y\n  {% 😀 %}\n{%}\n```\n", [FF]),
    ("fence-in-list-and-blockquote", "- ```\n  {% a %}\n  ```\n\n> ```\n> {% b %}\n> ```\n", [FF]),
    ("fence-indented", "    ```\n    {% a %}\n    ```\n", [TT, FF]),
    # frontmatter
    ("frontmatter", "---\ntitle: x\n---\n# h\n", [FF]),
    ("frontmatter-crlf", "---\r\na: 1\r\n---\r\n\r\ntext\r\n", [FF]),
    ("frontmatter-unterminated", "---\na: 1\n\n# h\n", [FF]),
    ("frontmatter-spaced-fences", "  ---  \na: 1\n ---\t\nx\n", [FF]),
    ("frontmatter-empty", "---\n---\n", [FF]),
    ("frontmatter-four-dashes", "----\na\n----\n", [FF]),
    ("frontmatter-in-blockquote-at-line-0", "> ---\n> a: 1\n> ---\n> b\n", [FF]),
    ("frontmatter-not-at-start", "x\n\n---\na\n---\n", [FF]),
    ("frontmatter-with-tags", "---\n{% a %}\n---\n{% b /%}\n", [FF]),
    # comments
    ("comment-block-and-inline", "<!-- a -->\n\nx <!-- b --> y\n\n<!--\nc\n-->\n", [TT, FF]),
    ("comment-block-trailing-text-dropped", "<!-- a --> after\nnext\n", [TT, FF]),
    ("comment-overlapping-close", "<!-->\n\n<!---->\n\n<!--->\n", [TT, FF]),
    ("comment-unclosed", "<!-- a\n\nb\n", [TT, FF]),
    ("comment-interrupts-paragraph", "para\n<!-- c -->\nmore\n", [TT, FF]),
    ("comment-spans-past-blockquote", "> <!-- a\nb -->\nc\n", [TT, FF]),
    ("comment-in-list-and-table", "- <!-- a -->\n\n| <!-- b --> |\n|---|\n", [TT, FF]),
    ("comment-in-link-label-throws", "a [<!-- x -->](y) b\n", [TT, FF]),
    ("comment-after-unmatched-bracket-throws", "see [1 <!-- note -->\n", [TT, FF]),
    # link validation
    ("link-validation", "https://example.com/{% a %}\n\n[x](https://{% $v %})\n\n[y]({% b %})\n\n"
     "The link https://e.com. {% c /%}\n\n{% d %} https://x{% e %}\n\nvscode://{% f %}\n", [LV, LVC]),
    ("link-validation-first-closed-tag-only", "{% x \"https://a{% y %}\n\nh://{% z %}\n", [LV, LVC]),
    # what upstream throws
    ("throws-type-error-length", "[a {% /b %}](u)\n", [FF]),
    ("throws-type-error-push-emphasis", "[a {% /b %}](u) *c*\n", [FF]),
    ("throws-type-error-push-strikethrough", "[a {% /b %}](u) ~~c~~\n", [FF]),
    ("throws-type-error-single-tilde", "[a {% /b %}](u) ~c\n", [FF]),
    # nested past V8's stack (node 22: about 2,200 to 4,700 levels) and the port's 300; between
    # the two node parses and the port throws, the one known difference (see MAX_NESTING)
    ("throws-range-error-block", "{% a x=" + DEEP(10000) + " %}\n", [FF]),
    ("throws-range-error-fence-info", "``` {% x=" + DEEP(10000) + " %}\nx\n```\n", [FF]),
    # 300 levels, the port's limit: parsed through (it fails at the innermost value, so the
    # error token stays shallow: serde_json reads JSON only 128 levels deep)
    ("parses-at-the-limit", "{% a x=" + DEEP(300).replace("1", "x") + " %}\n", [FF]),
]


def cases(args):
    out = []
    for id_, code, configs in PROBES:
        for i, config in enumerate(configs):
            out.append({"id": id_ + (f"#{i}" if len(configs) > 1 else ""), "code": code,
                        "tokenizer": config})
    for path in args.minimised or []:
        with open(path, encoding="utf-8") as f:
            for line in f:
                if line.strip():
                    m = json.loads(line)
                    out.append({"id": "fuzz-" + m["id"], "code": m["code"],
                                "tokenizer": m["tokenizer"]})
    return out


def lossy(value):
    if isinstance(value, str):
        return value.encode("utf-16", "surrogatepass").decode("utf-16", "replace")
    if isinstance(value, list):
        return [lossy(v) for v in value]
    if isinstance(value, dict):
        return {lossy(k): lossy(v) for k, v in value.items()}
    return value


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"), required=False)
    ap.add_argument("--minimised", action="append")
    ap.add_argument("--out", default=os.path.join(HERE, "golden.jsonl"))
    ap.add_argument("--node", default="node")
    args = ap.parse_args()
    todo = cases(args)
    text = "".join(json.dumps(c, ensure_ascii=False) + "\n" for c in todo)
    done = subprocess.run([args.node, os.path.join(HERE, "oracle_tokens.cjs"), args.markdoc],
                          input=text.encode("utf-8"), capture_output=True, check=True)
    answers = [json.loads(l) for l in done.stdout.decode("utf-8", "surrogatepass").split("\n")
               if l.strip()]
    assert len(answers) == len(todo)
    with open(args.out, "w", encoding="utf-8") as f:
        for case, answer in zip(todo, answers):
            line = dict(case)
            result = {k: v for k, v in answer.items() if k != "id"}
            fixed = lossy(result)
            line.update(fixed)
            if fixed != result:
                line["lossy"] = True
            f.write(json.dumps(line, ensure_ascii=False, separators=(",", ":")) + "\n")
    threw = sum(1 for a in answers if "threw" in a)
    print(f"wrote {len(todo)} cases ({threw} where upstream throws) to {args.out}")


if __name__ == "__main__":
    main()
