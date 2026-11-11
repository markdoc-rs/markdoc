"""Generate random tag contents for the tag grammar's differential test (compare.py).

Contents are built from tag.pegjs's tokens (tag names, identifiers, `.class`, `#id`,
`name=value`, primary values, `/`, null, booleans, numbers, strings with escapes, arrays,
hashes with identifier and string keys, `$` and `@` variables with `.name`, `[0]` and
`["x"]`, function calls with named and positional arguments, whitespace and newlines), mixed
with near-misses the grammar rejects (other whitespace, `\\u` escapes, raw control
characters, exponents, `.5`, `1.`, unbalanced brackets, non-ASCII and astral characters),
and then most of them are mutated (deletions, insertions, swaps, duplications), so that most
inputs fail and the failures land at every depth of the grammar.

Usage (run python with -I):
  python3 -I tests/grammar/fuzz.py --seed S --count N --out OUT.jsonl [--mode grammar|numbers|objects]

`--mode numbers` writes number literals of every size instead (`x=<literal>` and arrays of
them), for parseFloat and the JSON text of numbers; `--mode objects` writes hash literals and
calls whose keys collide with JavaScript's special properties (`__proto__`, array indices,
`$$mdtype`, inherited names), for the object semantics of `Object.assign` and `[[Set]]`.
"""

import argparse
import decimal
import json
import math
import random
import struct

IDENTS = ["foo", "bar", "if", "else", "x", "a", "b", "id", "class", "primary", "a1", "1a", "_",
          "-", "a-b", "a_b", "0", "1", "007", "42", "4294967294", "4294967295", "null", "true",
          "false", "nullx", "truex", "falsey", "__proto__", "constructor", "toString",
          "hasOwnProperty", "length", "valueOf", "Infinity", "NaN", "partial", "table", "tab",
          "callout", "type", "file", "variables", "equals", "and", "or", "not", "default",
          "debug", "ref", "image", "src", "alt", "title", "width"]
KEYS = ['"$$mdtype"', '"__proto__"', '""', '"a b"', '"1"', '"01"', '"-0"', '"4294967295"',
        '"0"', '"key"', '"é"', '"😀"', '"\\n"', '"a\\"b"', '"$x"', '"%}"']
NUMBERS = ["0", "-0", "1", "-1", "1.5", "-1.5", "007", "0.5", "10", "1000000", "0.1", "0.000001",
           "0.0000001", "123456789012345678901", "1152921504606846976", "9007199254740993",
           "100000000000000000000", "1000000000000000000000", "1" + "0" * 400, "0." + "0" * 30 + "1",
           "1e5", ".5", "1.", "-", "--1", "1.2.3", "0x10", "1_000", "+1", "-.5"]
STRING_PIECES = ["a", "b", "foo", " ", "  ", "é", "ß", "中文", "😀", "𝒳", " ", "﻿",
                 "\xa0", "%}", "{%", "'", "/", "#", ".", "=", "[", "]", "{", "}", "(", ")", ",",
                 ":", "$", "@", "\\\"", "\\\\", "\\n", "\\r", "\\t", "\\u0041", "\\x41", "\\0",
                 "\\/", "\\b", "\\f", "\\", "\x00", "\x01", "\x1f", "\x7f", "\x85", "\t", "\n",
                 "\r", "퟿", "", "￿"]
WS = [" ", " ", " ", "  ", "\n", "\t", " \n ", "\n\n", "\t\t"]
BAD_WS = ["\r", "\r\n", "\xa0", "\x0b", "\x0c", " ", "　", "﻿"]
RANDOM_CHARS = (list(" \n\t\r!\"#$%&'()*+,-./0123456789:;<=>?@AZaz[\\]^_`{|}~") +
                ["é", "😀", "\x00", "\x1f", "\x7f", " ", "﻿", "中", "𝒳", "\xa0"])


class Gen:
    def __init__(self, rng):
        self.r = rng

    def chance(self, p):
        return self.r.random() < p

    def ws(self, allow_empty=True):
        if allow_empty and self.chance(0.45):
            return ""
        if self.chance(0.03):
            return self.r.choice(BAD_WS)
        return self.r.choice(WS)

    def ident(self):
        if self.chance(0.6):
            return self.r.choice(IDENTS)
        alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
        return "".join(self.r.choice(alphabet) for _ in range(self.r.randint(1, 8)))

    def number(self):
        if self.chance(0.5):
            return self.r.choice(NUMBERS)
        s = "-" if self.chance(0.3) else ""
        s += "".join(self.r.choice("0123456789") for _ in range(self.r.randint(1, 22)))
        if self.chance(0.4):
            s += "." + "".join(self.r.choice("0123456789") for _ in range(self.r.randint(1, 20)))
        return s

    def string(self):
        body = "".join(self.r.choice(STRING_PIECES) if self.chance(0.5) else self.r.choice("abcxyz ")
                       for _ in range(self.r.randint(0, 6)))
        if self.chance(0.04):
            return '"' + body  # unterminated
        return '"' + body + '"'

    def variable(self, depth):
        s = ("$" if self.chance(0.8) else "@") + self.ident()
        for _ in range(self.r.randint(0, 3)):
            k = self.r.random()
            if k < 0.45:
                s += "." + self.ident()
            elif k < 0.7:
                s += "[" + self.number() + "]"
            elif k < 0.9:
                s += "[" + self.string() + "]"
            else:
                s += self.r.choice(["[" + self.ident() + "]", "[]", ".", "[1", "[ 1 ]", "[$x]"])
        return s

    def function(self, depth):
        params = []
        for _ in range(self.r.randint(0, 3)):
            v = self.value(depth + 1)
            params.append(self.ident() + "=" + v if self.chance(0.4) else v)
        sep = self.r.choice([",", ", ", " , ", ",\n", ","])
        inner = sep.join(params)
        if self.chance(0.05):
            inner = "," + inner
        if self.chance(0.05):
            inner += ","
        return self.ident() + "(" + self.ws() + inner + self.ws() + ")"

    def array(self, depth):
        items = [self.value(depth + 1) for _ in range(self.r.randint(0, 4))]
        sep = self.r.choice([",", ", ", " ,", ",\n"])
        inner = sep.join(items)
        if items and self.chance(0.2):
            inner += self.ws() + ","
        return "[" + self.ws() + inner + self.ws() + "]"

    def hash(self, depth):
        items = []
        for _ in range(self.r.randint(0, 4)):
            key = self.r.choice(KEYS) if self.chance(0.35) else self.ident()
            items.append(key + ":" + self.ws() + self.value(depth + 1))
        sep = self.r.choice([",", ", ", " , ", ",\n"])
        inner = sep.join(items)
        if items and self.chance(0.2):
            inner += self.ws() + ","
        return "{" + self.ws() + inner + self.ws() + "}"

    def value(self, depth=0):
        k = self.r.random()
        if depth > 3:
            k = k * 0.55
        if k < 0.07:
            return "null"
        if k < 0.14:
            return self.r.choice(["true", "false"])
        if k < 0.3:
            return self.string()
        if k < 0.45:
            return self.number()
        if k < 0.55:
            return self.variable(depth)
        if k < 0.68:
            return self.array(depth)
        if k < 0.82:
            return self.hash(depth)
        if k < 0.95:
            return self.function(depth)
        return self.ident()  # not a value

    def attribute(self):
        k = self.r.random()
        if k < 0.2:
            return "#" + self.ident()
        if k < 0.4:
            return "." + self.ident()
        return self.ident() + "=" + self.value()

    def attributes(self):
        return [self.attribute() for _ in range(self.r.randint(1, 4))]

    def content(self):
        k = self.r.random()
        if k < 0.12:  # variable or function
            s = self.variable(0) if self.chance(0.6) else self.function(0)
        elif k < 0.27:  # annotation
            s = (" " if self.chance(0.9) else self.ws(False)).join(self.attributes()) + self.ws()
        elif k < 0.37:  # closing tag
            s = "/" + self.ident()
        else:  # opening or self-closing tag
            s = self.ident()
            if self.chance(0.5):
                s += self.r.choice([" ", " ", "  ", "\n", "\t"]) + self.value()
            if self.chance(0.6):
                s += self.r.choice([" ", " ", "  ", "\n"]) + " ".join(self.attributes())
            if self.chance(0.3):
                s += self.ws() + "/"
        if self.chance(0.03):
            s = self.ws(False) + s
        if self.chance(0.06):
            s = s + self.ws(False)
        return s

    def mutate(self, s):
        for _ in range(self.r.randint(1, 3)):
            if not s:
                s = self.r.choice(RANDOM_CHARS)
                continue
            i = self.r.randrange(len(s))
            k = self.r.random()
            if k < 0.3:  # delete a span
                s = s[:i] + s[i + self.r.randint(1, 3):]
            elif k < 0.55:  # insert a character or a token
                piece = self.r.choice(RANDOM_CHARS) if self.chance(0.6) else self.r.choice(
                    [self.ident(), self.number(), self.string(), "=", "/", ",", ":", "(", ")"])
                s = s[:i] + piece + s[i:]
            elif k < 0.75:  # swap two neighbours
                if i + 1 < len(s):
                    s = s[:i] + s[i + 1] + s[i] + s[i + 2:]
            elif k < 0.88:  # replace a character
                s = s[:i] + self.r.choice(RANDOM_CHARS) + s[i + 1:]
            else:  # duplicate a span
                j = min(len(s), i + self.r.randint(1, 6))
                s = s[:j] + s[i:j] + s[j:]
        return s

    def item(self):
        s = self.content()
        if self.chance(0.65):
            s = self.mutate(s)
        return s

    def number_item(self):
        k = self.r.random()
        if k < 0.6:
            return "x=" + self.number_literal()
        if k < 0.8:
            return "x=[" + ",".join(self.number_literal() for _ in range(self.r.randint(1, 5))) + "]"
        return "$v[" + self.number_literal() + "]"

    def number_literal(self):
        k = self.r.random()
        sign = "-" if self.chance(0.3) else ""
        if k < 0.25:  # integers of every length
            return sign + str(self.r.randint(0, 9)) + "".join(
                self.r.choice("0123456789") for _ in range(self.r.randint(0, 30)))
        if k < 0.5:  # decimals around every exponent the grammar reaches
            return (sign + "".join(self.r.choice("0123456789") for _ in range(self.r.randint(1, 25)))
                    + "." + "".join(self.r.choice("0123456789") for _ in range(self.r.randint(1, 25))))
        if k < 0.6:  # tiny values: many leading zeros
            return sign + "0." + "0" * self.r.randint(0, 330) + str(self.r.randint(1, 99999))
        if k < 0.7:  # huge values
            return sign + str(self.r.randint(1, 9)) + "0" * self.r.randint(15, 330)
        if k < 0.8:  # a random double, rounded to fixed notation
            x = self.r.uniform(-1, 1) * 10 ** self.r.randint(-20, 25)
            return format(x, "f").rstrip("0").rstrip(".") or "0"
        if k < 0.9:  # a random finite double (any bit pattern), written exactly
            while True:
                x = struct.unpack("<d", struct.pack("<Q", self.r.getrandbits(64)))[0]
                if math.isfinite(x):
                    return exact(x)
        # a double with few fraction bits, written exactly: its shortest form is often
        # exactly halfway between two (JavaScript takes the even one)
        m = self.r.randrange(1 << 52, 1 << 53)
        return sign + exact(math.ldexp(m, self.r.randint(-16, 12)))


OBJECT_KEYS = ["__proto__", '"__proto__"', "a", "b", "0", "1", "01", '"4294967295"', "4294967294",
               "length", "constructor", "toString", '"$$mdtype"', '""', "path", "name"]
OBJECT_VALUES = ["null", "1", "0", '"s"', '""', "true", "false", "[1]", "[]", "{}", "$v", "@v", "f()",
                 "f(a=1)"]


def object_value(g, depth):
    """A hash literal or a call whose keys collide with JavaScript's special properties."""
    k = g.r.random()
    if depth < 3 and k < 0.35:
        items = [g.r.choice(OBJECT_KEYS) + ": " + object_value(g, depth + 1)
                 for _ in range(g.r.randint(1, 6))]
        return "{" + ", ".join(items) + "}"
    if depth < 3 and k < 0.5:
        params = []
        for _ in range(g.r.randint(1, 5)):
            v = object_value(g, depth + 1)
            name = g.r.choice(OBJECT_KEYS)
            params.append(name + "=" + v if g.chance(0.6) and not name.startswith('"') else v)
        return g.r.choice(["f", "fn", "__proto__"]) + "(" + ", ".join(params) + ")"
    if depth < 3 and k < 0.6:
        return "[" + ", ".join(object_value(g, depth + 1) for _ in range(g.r.randint(1, 3))) + "]"
    return g.r.choice(OBJECT_VALUES)


def exact(x):
    """The exact decimal value of a double, in the grammar's -?[0-9]+(.[0-9]+)? form."""
    text = format(decimal.Decimal(x), "f")
    if "." in text:
        text = text.rstrip("0").rstrip(".")
    return text if text not in ("-0", "") else "0"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("--count", type=int, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mode", choices=("grammar", "numbers", "objects"), default="grammar")
    ap.add_argument("--prefix", default=None)
    args = ap.parse_args()
    g = Gen(random.Random(args.seed))
    prefix = args.prefix or f"{dict(grammar='fz', numbers='num', objects='obj')[args.mode]}{args.seed}"
    with open(args.out, "w", encoding="utf-8") as f:
        for i in range(args.count):
            if args.mode == "grammar":
                content = g.item()
            elif args.mode == "numbers":
                content = g.number_item()
            else:
                content = "x=" + object_value(g, 0)
            f.write(json.dumps({"id": f"{prefix}-{i}", "content": content}) + "\n")


if __name__ == "__main__":
    main()
