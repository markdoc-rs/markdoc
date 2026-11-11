"""Generate configs and tag-heavy documents for the transform, errors and html stages.

Each line is a compare.py input: {id, code, config, slots?, partials?}. The configs are
upstream-shaped JSON (see tests/next/oracle.cjs): tag and node schemas with attributes of
every type (names, constructors and classes as {"$type": name}, lists of types), required
attributes, defaults, render names (strings, true, false and other loosely-false values),
`matches` as arrays and as RegExps ({"$regex", "flags"}, `g` and `y` included), error
levels, inline/block placement, child rules, self-closing tags and slots; default tags and
nodes overridden or switched off; functions with `returns` and `parameters`; variables of
every JSON shape; partials; `validation.validateFunctions`. The documents use those tags,
block and inline, with every kind of attribute value (strings, numbers, booleans, null,
arrays, objects, variables, function calls, classes, ids), conditionals with `else`,
partials with variables, slots, interpolated variables and function calls, and the
Markdown around them.

Usage (run python with -I):
  python3 -I tests/next/gen_semantics.py --seed S --count N [--profile focused] > cases.jsonl
  python3 -I tests/next/compare.py --stage transform --from-oracle cases.jsonl --markdoc DIST

The `focused` profile leans on what the default one reaches least: slots rendered into
attributes, function parameters checked against their schemas (`validateFunctions`), names
every JS object inherits (`toString` as a tag, attribute, slot, function or parameter name),
`@` arrays, `if` without a condition, non-ASCII render names, `__proto__` as an attribute's or
a slot's render name (an assignment to the prototype setter), and `u`/`v` RegExps whose
`lastIndex` lands inside a surrogate pair (astral text, `g` and `y`).

Left out on purpose, as the from-oracle html stage cannot compare them: objects that say they
are tags (`$$mdtype`; the harness reads a render tree's tags strictly), members that are
`undefined` (gone from the oracle's tree JSON, though its HTML saw them), `__proto__` keys in
object literals, and the RegExp features where regress and V8 differ (see
src/next/schema/regexp.rs).

Schema `validate` and `transform` functions cannot be written in JSON and are not generated.
"""

import argparse
import json
import random

TAG_NAMES = ["box", "note", "card", "x-tag", "item2", "grid", "cell", "btn", "tab", "tabs",
             "media", "b1", "span", "toString", "constructor"]
ATTR_NAMES = ["a", "b", "title", "kind", "size", "data", "items", "opts", "flag", "count",
              "x-y", "0", "2", "primary", "level", "href", "variables", "file", "class", "id"]
NODE_TYPES = ["paragraph", "tag", "list", "item", "inline", "text", "heading", "fence", "hr",
              "image", "table", "em", "strong", "link", "code", "softbreak", "hardbreak",
              "blockquote", "comment", "s"]
RENDER_TAGS = ["div", "span", "section", "p", "br", "img", "hr", "x-el", "Foo", "aside"]
TYPE_NAMES = ["String", "Number", "Boolean", "Object", "Array"]
CLASSES = ["Class", "Id", "ConditionalAttributeType", "PartialFile"]
ODD_TYPE_NAMES = ["Foo", "constructor", "string", "__proto__"]
SLOT_NAMES = ["s1", "header", "footer", "0"]
FUNCTIONS = ["equals", "and", "or", "not", "default", "debug"]
CUSTOM_FUNCTIONS = ["fmt", "upper", "pick", "nope"]
REGEXES = ["^[a-z]+$", "^\\d+$", "foo|bar", "^(a|b)c?", "[A-Z]", "^$", ".*", "x/y",
           "^\\w+\\s\\w+$", "^.{2,4}$", "\\bok\\b", "^[^<>]*$", "(?:ab)+", "a.b", "^1",
           "true", "\\[object", "^-?\\d+(\\.\\d+)?$", "[\\u00e9]", "^\\S+$"]
FLAGS = ["", "", "", "i", "g", "y", "gi", "m", "s", "u", "iu", "gy", "dgimsuy"]
# Code point patterns for astral text (focused): consuming, so V8's empty matches inside a
# surrogate pair never arise; an atom (one astral character) included.
REGEXES_ASTRAL = ["^.", ".$", "^..$", "^.$", "[^a]", "\\u{1F600}", "\U0001F600", "\\p{L}", "x"]
FLAGS_ASTRAL = ["gu", "yu", "gv", "yv", "u", "gmu", "gsv"]
ASTRAL_STRINGS = ["\U0001F600", "x\U0001F600", "\U0001F600a", "a\U0001F600b\U0001F600"]
LEVELS = ["debug", "info", "warning", "error", "critical"]
STRINGS = ["", "a", "foo", "bar", "Hello world", "x y z", "1", "0", "true", "<b>&\"'",
           "ab", "abc", "ok", "#id", "été", "a\\b", "line\nbreak", "π", "A1"]
VAR_KEYS = ["v", "s", "b", "n", "zero", "empty", "arr", "obj", "deep", "t", "f", "neg",
            "big", "list", "Upper", "0"]


class Gen:
    def __init__(self, seed, focused=False):
        self.r = random.Random(seed)
        self.focused = focused

    def chance(self, p):
        return self.r.random() < p

    def pick(self, items):
        return self.r.choice(items)

    def sample(self, items, k):
        return self.r.sample(items, min(k, len(items)))

    # --- JSON values (config) -------------------------------------------------------------

    def json_value(self, depth=0):
        r = self.r.random()
        if r < 0.22:
            return self.pick(STRINGS)
        if r < 0.40:
            return self.pick([0, 1, 2, -3, 1.5, 42, 1e21, 0.1, 100, 7])
        if r < 0.50:
            return self.pick([True, False])
        if r < 0.56:
            return None
        if r < 0.78 and depth < 3:
            return [self.json_value(depth + 1) for _ in range(self.r.randint(0, 4))]
        if depth < 3:
            return {self.pick(VAR_KEYS + ATTR_NAMES): self.json_value(depth + 1)
                    for _ in range(self.r.randint(0, 4))}
        return self.pick(STRINGS)

    def variables(self):
        if self.chance(0.1):
            return None  # no `variables` at all
        v = {
            "v": self.pick([1, "one", True, 0, "x"]),
            "s": self.pick(STRINGS),
            "b": self.pick([True, False]),
            "n": None,
            "zero": 0,
            "empty": "",
            "arr": [self.json_value(2) for _ in range(self.r.randint(0, 4))],
            "obj": {"a": {"b": self.json_value(2)}, "k": [True, 2], "c": self.pick(STRINGS)},
            "deep": {"x": {"y": {"z": self.json_value(1)}}},
            "t": "<i>tag</i>",
            "f": False,
            "neg": -2,
            "big": 1e21,
            "list": [{"name": "a"}, {"name": "b"}],
            "Upper": "U",
            "0": "zero-key",
        }
        if self.chance(0.3):
            for k in self.sample(list(v), self.r.randint(1, 4)):
                del v[k]
        return v

    # --- types and attribute schemas --------------------------------------------------------

    def type_spec(self, allow_list=True):
        r = self.r.random()
        if r < 0.35:
            return self.pick(TYPE_NAMES)
        if r < 0.55:
            return {"$type": self.pick(TYPE_NAMES)}
        if r < 0.65:
            return {"$type": self.pick(CLASSES)}
        if r < 0.72:
            if self.focused and self.chance(0.02):
                return "toString"  # `new Object.prototype.toString()` throws
            return self.pick(ODD_TYPE_NAMES)
        if r < 0.78:
            return self.pick([None, "", False, 0])
        if allow_list:
            return [self.type_spec(False) for _ in range(self.r.randint(0, 3))]
        return self.pick(TYPE_NAMES)

    def render_value(self):
        names = [True, False, True, "data-x", "", "0", " ", 0, None, "title",
                 "Data-Upper", "class", "2", "b", [], "x"]
        if self.focused:
            names += ["DATA-\u00c9", "\u00c4b", "x", "__proto__"]
        return self.pick(names)

    def regex(self):
        if self.focused and self.chance(0.3):
            return {"$regex": self.pick(REGEXES_ASTRAL), "flags": self.pick(FLAGS_ASTRAL)}
        return {"$regex": self.pick(REGEXES), "flags": self.pick(FLAGS)}

    def matches(self):
        r = self.r.random()
        if r < 0.45:
            items = [self.pick(STRINGS + [1, 2, 0, True, None, 1.5])
                     for _ in range(self.r.randint(0, 11))]
            if self.chance(0.1):
                items.append([1])
            return items
        if r < 0.85:
            return self.regex()
        return self.pick([None, "foo", 3, {"a": 1}])

    def attr_schema(self):
        a = {}
        if self.chance(0.8):
            a["type"] = self.type_spec()
        if self.chance(0.5):
            a["render"] = self.render_value()
        if self.chance(0.3):
            a["default"] = self.json_value(1)
        if self.chance(0.3):
            a["required"] = self.pick([True, False, 1, 0, "yes"])
        if self.chance(0.3):
            a["matches"] = self.matches()
        if self.chance(0.2):
            a["errorLevel"] = self.pick(LEVELS + [None, ""])
        if self.chance(0.05):
            a["description"] = "described"
        return a

    def slot_schemas(self):
        out = {}
        for name in self.sample(SLOT_NAMES, self.r.randint(1, 3)):
            s = {}
            if self.chance(0.5):
                s["render"] = self.pick([True, False, "named", "", 0, None]
                                        + (["__proto__"] if self.focused else []))
            if self.chance(0.4):
                s["required"] = self.pick([True, False])
            out[name] = s
        return out

    def tag_schema(self):
        if self.chance(0.04):
            return self.pick([None, True, 1, "x", [], False])
        s = {}
        if self.chance(0.75):
            s["render"] = self.pick(RENDER_TAGS + ["", None, False])
        if self.chance(0.8):
            s["attributes"] = {name: self.attr_schema()
                               for name in self.sample(ATTR_NAMES, self.r.randint(0, 6))}
        if self.chance(0.3):
            s["children"] = self.sample(NODE_TYPES, self.r.randint(0, 6))
        if self.chance(0.3):
            s["inline"] = self.pick([True, False, None])
        if self.chance(0.25):
            s["selfClosing"] = self.pick([True, False, 1, 0, ""])
        if self.chance(0.7 if self.focused else 0.3):
            s["slots"] = self.slot_schemas()
        return s

    def node_overrides(self):
        out = {}
        for t in self.sample(["paragraph", "heading", "link", "fence", "list", "item", "text",
                              "em", "strong", "code", "image", "inline", "document",
                              "softbreak", "hr", "blockquote"], self.r.randint(0, 3)):
            if self.chance(0.2):
                out[t] = None
            else:
                out[t] = self.tag_schema()
        return out

    def functions(self):
        out = {}
        for name in self.sample(CUSTOM_FUNCTIONS + FUNCTIONS, self.r.randint(0, 3)):
            if name in FUNCTIONS and self.chance(0.7):
                continue  # keep most built-ins
            f = {}
            if self.chance(0.6):
                f["returns"] = self.pick([self.type_spec(), [self.type_spec(False),
                                                             self.type_spec(False)]])
            if self.chance(0.9 if self.focused else 0.5):
                f["parameters"] = {p: self.attr_schema()
                                   for p in self.sample(["0", "1", "x", "key"],
                                                        self.r.randint(0, 3))}
            out[name] = self.pick([f, f, f, None]) if self.chance(0.1) else f
        return out

    # --- document values --------------------------------------------------------------------

    def doc_string(self):
        s = self.pick(STRINGS + (ASTRAL_STRINGS if self.focused else []))
        return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'

    def variable(self):
        head = self.pick(VAR_KEYS + ["missing", "u"])
        path = ("@" if self.focused and self.chance(0.05) else "$") + head
        # A path on through `null` throws (upstream's TypeError): keep those rare.
        steps = 0 if head == "n" and not self.chance(0.05) else self.r.randint(0, 3)
        for _ in range(steps):
            r = self.r.random()
            if r < 0.5:
                path += "." + self.pick(["a", "b", "x", "y", "z", "k", "c", "length", "name",
                                         "missing"])
            elif r < 0.8:
                path += "[" + str(self.r.randint(0, 3)) + "]"
            else:
                path += '["' + self.pick(["a", "k", "0", "x y"]) + '"]'
        return path

    def function_call(self, depth):
        names = FUNCTIONS + FUNCTIONS + CUSTOM_FUNCTIONS + ["unknown"]
        if self.focused:
            names += CUSTOM_FUNCTIONS + ["toString"]
        name = self.pick(names)
        args = []
        for i in range(self.r.randint(0, 3)):
            v = self.doc_value(depth + 1)
            if self.chance(0.2):
                keys = ["x", "key", "0", "a"] + (["toString", "1"] if self.focused else [])
                args.append(self.pick(keys) + "=" + v)
            else:
                args.append(v)
        return name + "(" + ", ".join(args) + ")"

    def doc_value(self, depth=0):
        r = self.r.random()
        if r < 0.2:
            return self.doc_string()
        if r < 0.32:
            return self.pick(["0", "1", "-1", "2.5", "42", "007", "-0", "1000000"])
        if r < 0.40:
            return self.pick(["true", "false", "null"])
        if r < 0.62:
            return self.variable()
        if r < 0.78 and depth < 3:
            return self.function_call(depth)
        if r < 0.89 and depth < 3:
            return "[" + ", ".join(self.doc_value(depth + 1)
                                   for _ in range(self.r.randint(0, 3))) + "]"
        if depth < 3:
            items = []
            for _ in range(self.r.randint(0, 3)):
                key = self.pick(["a", "b", "k", "x-y", '"q w"', "0", "1"])
                items.append(key + ": " + self.doc_value(depth + 1))
            return "{" + ", ".join(items) + "}"
        return self.doc_string()

    def attrs(self, names):
        parts = []
        if self.chance(0.25):
            parts.append(self.doc_value())  # primary
        for _ in range(self.r.randint(0, 4)):
            r = self.r.random()
            if r < 0.12:
                parts.append("." + self.pick(["cls", "a", "b-c", "x1"]))
            elif r < 0.2:
                parts.append("#" + self.pick(["anchor", "x", "a1", "1a"]))
            elif self.focused and r < 0.24:
                parts.append(self.pick(["toString", "valueOf"]) + "=" + self.doc_value())
            else:
                parts.append(self.pick(names) + "=" + self.doc_value())
        return " ".join(parts)

    # --- documents ----------------------------------------------------------------------------

    def inline_text(self, tags, depth=0):
        out = []
        for _ in range(self.r.randint(1, 5)):
            r = self.r.random()
            if r < 0.25:
                out.append(self.pick(["plain text", "more words", "a & b < c", "x"]))
            elif r < 0.37:
                out.append("{% " + self.variable() + " %}")
            elif r < 0.45:
                out.append("{% " + self.function_call(1) + " %}")
            elif r < 0.62 and depth < 2:
                t = self.pick(tags)
                out.append("{% " + (t + " " + self.attrs(ATTR_NAMES)).strip() + " %}"
                           + self.inline_text(tags, depth + 1) + "{% /" + t + " %}")
            elif r < 0.7:
                t = self.pick(tags)
                out.append("{% " + (t + " " + self.attrs(ATTR_NAMES)).strip() + " /%}")
            elif r < 0.76:
                out.append("*em* **strong** `code`")
            elif r < 0.81:
                out.append("[link](/url \"title\")")
            elif r < 0.84:
                out.append("![img](/src.png)")
            elif r < 0.9 and depth < 2:
                out.append("{% if " + self.doc_value() + " %}yes{% else /%}no{% /if %}")
            else:
                out.append("~~gone~~")
        return " ".join(out)

    def block(self, tags, depth, slots):
        r = self.r.random()
        if r < 0.18:
            return self.inline_text(tags)
        if r < 0.28:
            ann = " {% " + self.attrs(ATTR_NAMES) + " %}" if self.chance(0.5) else ""
            return "#" * self.r.randint(1, 3) + " Heading " + self.inline_text(tags, 2) + ann
        if r < 0.5 and depth < 3:
            t = self.pick(tags)
            inner = "\n\n".join(self.block(tags, depth + 1, slots)
                                for _ in range(self.r.randint(0, 2)))
            if slots and self.chance(0.8 if self.focused else 0.4):
                name = self.pick(SLOT_NAMES + ["bad"] + (["toString"] if self.focused else []))
                inner = ("{% slot " + self.pick(['"' + name + '"', "", "1", self.variable()])
                         + " %}\nslot text " + self.inline_text(tags, 2) + "\n{% /slot %}\n\n"
                         + inner)
            return ("{% " + (t + " " + self.attrs(ATTR_NAMES)).strip() + " %}\n" + inner
                    + "\n{% /" + t + " %}")
        if r < 0.58:
            t = self.pick(tags)
            return "{% " + (t + " " + self.attrs(ATTR_NAMES)).strip() + " /%}"
        if r < 0.68 and depth < 3:
            cond = "" if self.focused and self.chance(0.1) else " " + self.doc_value()
            parts = ["{% if" + cond + " %}", self.block(tags, depth + 1, slots)]
            for _ in range(self.r.randint(0, 2)):
                cond = self.doc_value() if self.chance(0.6) else ""
                parts.append(("{% else " + cond).rstrip() + " /%}")
                parts.append(self.block(tags, depth + 1, slots))
            parts.append("{% /if %}")
            return "\n".join(parts)
        if r < 0.74:
            f = self.pick(["p1.md", "p2.md", "empty.md", "missing.md"]
                          + (["toString"] if self.chance(0.1) else []))
            v = " variables=" + self.doc_value() if self.chance(0.5) else ""
            return '{% partial file="' + f + '"' + v + " /%}"
        if r < 0.8:
            return "\n".join("- " + self.inline_text(tags, 2) for _ in range(self.r.randint(1, 3)))
        if r < 0.85:
            info = self.pick(["js", "", "py {% process=false %}", "md {% .cls title=\"t\" %}"])
            return "```" + info + "\ncode {% " + self.variable() + " %} here\n```"
        if r < 0.9:
            return "| h1 | h2 |\n|----|----|\n| " + self.inline_text(tags, 2) + " | c |"
        if r < 0.95:
            return "> " + self.inline_text(tags, 2)
        return self.pick(["---", "text  \nwith break", "<!-- comment -->", "{% $obj %}",
                          "{% debug($obj) %}", "{% $arr %}"])

    def case(self, i):
        tags = self.sample(TAG_NAMES, self.r.randint(1, 5))
        config = {}
        if self.chance(0.95):
            config["tags"] = {t: self.tag_schema() for t in tags}
            if self.chance(0.1):
                config["tags"][self.pick(["if", "else", "partial", "slot", "table"])] = \
                    self.pick([None, {"render": "div"}, {}])
        if self.chance(0.3):
            config["nodes"] = self.node_overrides()
        variables = self.variables()
        if variables is not None:
            config["variables"] = variables
        if self.chance(0.4):
            config["functions"] = self.functions()
        if self.chance(0.5):
            config["validation"] = {"validateFunctions": self.chance(0.6)}
        partials = {
            "p1.md": "Partial {% $v %} and {% " + self.pick(tags) + " /%}\n\n"
                     + "{% if $flag %}flagged{% /if %} {% $file %}",
            "p2.md": "- item {% $s %}\n- {% equals($v, 1) %}",
            "empty.md": "",
        }
        slots = self.chance(0.7 if self.focused else 0.3)
        doc_tags = tags + ["if"]
        if self.chance(0.2):
            doc_tags.append(self.pick(TAG_NAMES))  # perhaps one the config does not define
        doc = "\n\n".join(self.block(doc_tags, 0, slots)
                          for _ in range(self.r.randint(1, 6)))
        out = {"id": f"gen-{i}", "code": doc, "config": config}
        if slots:
            out["slots"] = True
        if self.chance(0.7):
            config["partials"] = partials
        elif self.chance(0.5):
            out["partials"] = partials
        return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--count", type=int, default=1000)
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--profile", choices=["default", "focused"], default="default")
    args = ap.parse_args()
    gen = Gen(args.seed, focused=args.profile == "focused")
    for i in range(args.start, args.start + args.count):
        print(json.dumps(gen.case(i), ensure_ascii=False))


if __name__ == "__main__":
    main()
