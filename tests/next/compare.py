"""Compare the next layer's port with upstream Markdoc 0.5.11, stage by stage.

Runs tests/next/oracle.cjs (node, upstream's dist) and examples/next_compare (the Rust port)
over the same inputs, compares both answers for each stage whole, and reports each document's
first difference, then the totals. Exits 1 unless every document agrees.

Usage (run python with -I, and under bin/capped):
  python3 -I tests/next/compare.py --stage STAGE [INPUTS.jsonl ...] [--conformance]
      --markdoc DIST/index.js [--from-oracle] [--template JSON | --templates JSON-or-FILE]
      [--rust-bin PATH] [--node PATH] [--id-prefix P] [--limit N] [--show N] [--keep DIR]
      [--batch N] [--jobs N] [--timeout SECONDS]
  python3 -I tests/next/compare.py --roundtrip [INPUTS.jsonl ...] [--conformance] --markdoc ...
  python3 -I tests/next/compare.py --golden OUT.jsonl [INPUTS.jsonl ...] [--conformance] --markdoc ...

STAGE is tokens, ast, errors, transform, html, or all (the five from one run of each side);
the api stages are format (Markdoc.format of the AST, with the line's formatOptions),
reactStatic (renderers.reactStatic of the render tree) and react (renderers.react with a fake
React that records its calls, with the line's components), or api (the three) and every (all
eight). A line with a `renderTree` (a render tree, as the transform stage writes one, or any
JSON) has html, reactStatic and react render that tree instead of the document's; one with
`tokens` (plain() tokens) or `ast` (Node JSON, revived by Ast.fromJSON) has both sides parse
those tokens, or validate, transform and format that AST, instead of the source (upstream's
own tests make such inputs by hand: tests/next/suites/).
Each input line is {id, src or code, ...} with any of the oracle's request keys (config,
slots, file, partials, allowIndentation, allowComments, tokenizer, parse); --template is
merged under every line (the line's keys win), and --templates runs the inputs once per
template of a JSON list (given inline or as a file). --conformance adds upstream's 105
conformance cases (examples/next_compare --conformance). allowIndentation and allowComments
default to true, as the oracle's and the conformance runner's do; upstream's own Tokenizer
defaults both to false, so say false to test that.

End to end is the default: both sides start from the source, so the port's tokens, AST,
validation errors, render tree and HTML are its own tokenizer's, parser's and semantics'.
--from-oracle feeds the port upstream's output for the stage before instead, so a stage can
be checked alone: ast parses the oracle's tokens; transform and errors run on the oracle's AST
(and its partials' ASTs); html renders the oracle's tree.

--golden writes, per input, the request and every oracle output (tokens, ast, errors,
transformed, html, partials) to OUT.jsonl: tests/next/golden/conformance.jsonl is
`--golden tests/next/golden/conformance.jsonl --conformance`, read by tests/next_compare.rs.

--roundtrip checks the harness itself: the oracle's tokens, AST, partials and render tree go
through the Rust converters and back (compare::tokens_from_json + tokens_json,
node_from_json + node_json, render_from_json + render_json), and must come back equal.

What is compared, per stage: what JSON.stringify writes of upstream's value, whole, against
the port's JSON of its own (nothing is left out or filled in on either side):
  tokens     the token stream: markdown-it's tokens with every field, parseTags' objects
             with theirs (a field an object lacks is absent, not 0 or false).
  ast        upstream's Node JSON, `lines` and `location` included, errors whole.
  errors     validate()'s [{type, lines, location, error}].
  transform  the render tree.
  html       the string.
Objects are compared key by key in order (key order is output); numbers as numbers.

Thrown errors are compared too, at every stage: upstream's name and message (`TypeError:
Cannot ...`, JS's String(error)) against what the port says it throws. A stage's error is the
first one thrown on its way: tokens, ast, then partials and transformed (transform, html) or
errors. A document where both sides threw the same agrees; one where they threw differently,
or only one side threw, does not. A port that panics or crashes never agrees.

A render tree that JSON.stringify cannot write (a `toJSON` in it that throws) is
"unwritable" on either side: the transform did not throw; both unwritable with the same error
agree ("both unwritable"), anything else does not.

Not compared, and counted apart: "oracle unwritable" (upstream made the value but
JSON.stringify cannot write it for V8's stack, e.g. an AST nested past it), "skipped" (with
--from-oracle, upstream threw before the stage's input), "harness" (a side could not read the
request).

Strings: inputs reach both sides exactly, lone UTF-16 surrogates included (a request that
holds one is written as ASCII JSON, its `\\udxxx` escapes read exactly by JSON.parse and by
next_compare for the config's `variables`). The render tree ("transformed") is compared
exactly: values hold JS strings. Every other answer field is what the port keeps as Rust
`String`s by the user's decision (tokens, the AST, messages, the HTML), where a lone surrogate
is U+FFFD, what node itself writes for one when it encodes UTF-8: there it becomes U+FFFD on
both sides before comparing (EXACT_FIELDS, loads). The totals say how many documents held
one. Answers are read as JSONL split at "\\n" alone: JSON leaves
U+2028, U+2029 and U+0085 raw inside strings. Every walk over a value is iterative, so any
depth either side can write is compared.

The documents go through both sides in batches (--batch, default 400; --jobs batches at once,
default 2), so memory stays flat whatever the inputs. A port process that dies (a stack
overflow, an abort, an allocation past --port-memory, --timeout) is restarted after the
document it died on, which counts as "port crashed" (with --stage all, that document's stages
are then run one at a time, so the ones before the stage that kills it are still compared);
the oracle likewise (node's heap capped by --oracle-memory).
"""

import argparse
import collections
import json
import os
import re
import resource
import shlex
import subprocess
import sys
import tempfile
import threading
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(os.path.dirname(HERE))
REQUEST_KEYS = ("config", "slots", "file", "partials", "tokenizer", "parse") + \
    ("formatOptions", "components", "renderTree", "tokens", "ast", "lossyTokens")
STAGES = ("tokens", "ast", "errors", "transform", "html")
# The api stages: the formatter and the React renderers.
API_STAGES = ("format", "reactStatic", "react")
# The stages that render a tree (with --from-oracle, upstream's, given as the request's tree).
TREE_STAGES = ("html", "reactStatic", "react")
# The oracle's output a stage compares, and the one its --from-oracle input comes from.
OUTPUT = {"tokens": "tokens", "ast": "ast", "errors": "errors", "transform": "transformed",
          "html": "html", "format": "formatted", "reactStatic": "reactStatic", "react": "react"}
INPUT = {"ast": "tokens", "transform": "ast", "errors": "ast", "html": "transformed",
         "format": "ast", "reactStatic": "transformed", "react": "transformed"}
# The steps each stage's output needs, in the order they run (both sides' `failed` keys).
CHAIN = {"tokens": ("tokens",), "ast": ("tokens", "ast"), "errors": ("tokens", "ast", "errors"),
         "transform": ("tokens", "ast", "partials", "transformed"),
         "html": ("tokens", "ast", "partials", "transformed", "html"),
         "format": ("tokens", "ast", "formatted"),
         "reactStatic": ("tokens", "ast", "partials", "transformed", "reactStatic"),
         "react": ("tokens", "ast", "partials", "transformed", "react")}
TALLY = ("equal", "both threw", "different", "threw differently", "port threw",
         "oracle threw", "port crashed", "oracle unwritable", "both unwritable",
         "unwritable differently", "port unwritable", "skipped", "harness")
# What makes a run fail.
BAD = ("different", "threw differently", "port threw", "oracle threw", "port crashed",
       "unwritable differently", "port unwritable", "harness")
# What V8 throws where its stack runs out (writing a value nested past it: the depth band).
STACK_OVERFLOW = "RangeError: Maximum call stack size exceeded"
# A lone UTF-16 surrogate, as json.loads leaves one in a str (a valid pair it joins).
LONE_SURROGATE = re.compile("[\ud800-\udfff]")
# A surrogate escape in JSON text: every lone surrogate in a string json.loads reads comes
# from one (JSON text in UTF-8 holds no raw surrogate).
SURROGATE_ESCAPE = re.compile(r"\\u[dD][89a-fA-F]")


# --- values, walked iteratively -------------------------------------------------------------

def fffd(value):
    """`value` with each lone surrogate in its strings (keys too) U+FFFD, as node writes one
    in UTF-8, changed in place where it is a list or a dict; and whether there was any."""
    if isinstance(value, str):
        mapped = LONE_SURROGATE.sub("�", value)
        return mapped, mapped != value
    changed = False
    stack = [value] if isinstance(value, (list, dict)) else []
    while stack:
        container = stack.pop()
        if isinstance(container, dict):
            if any(LONE_SURROGATE.search(k) for k in container):
                items = list(container.items())
                container.clear()
                for k, v in items:
                    container[LONE_SURROGATE.sub("�", k)] = v
                changed = True
            entries = container.items()
        else:
            entries = enumerate(container)
        for k, v in list(entries):
            if isinstance(v, str):
                mapped = LONE_SURROGATE.sub("�", v)
                if mapped != v:
                    container[k] = mapped
                    changed = True
            elif isinstance(v, (list, dict)):
                stack.append(v)
    return value, changed


# An answer's fields that are exact in the port: the render tree holds values, whose strings
# are JS strings (lone surrogates included). Every other field holds what the port keeps as
# Rust `String`s by the user's decision (tokens, the AST, messages, the HTML), where a lone
# surrogate is U+FFFD: those are compared with each lone surrogate U+FFFD on both sides.
EXACT_FIELDS = ("transformed",)


def loads(line):
    """An answer line, lone surrogates made U+FFFD in every field but EXACT_FIELDS; and
    whether there were any outside them."""
    value = json.loads(line)
    if SURROGATE_ESCAPE.search(line) is None:
        return value, False
    if not isinstance(value, dict):
        return fffd(value)
    changed = False
    for k in list(value):
        if k not in EXACT_FIELDS:
            value[k], c = fffd(value[k])
            changed = changed or c
    return value, changed


def dumps_request(request, **kwargs):
    """A request line: UTF-8 JSON, or all-ASCII JSON where a string holds a lone surrogate
    (its `\\udxxx` escape, which both sides read exactly: JSON.parse, and next_compare's
    reader for the config's variables). Any JSON this script writes goes through it: UTF-8
    cannot encode a lone surrogate."""
    text = json.dumps(request, ensure_ascii=False, **kwargs)
    if LONE_SURROGATE.search(text):
        text = json.dumps(request, **kwargs)
    return text


def same_scalar(a, b):
    """JSON scalar equality: 1 == 1.0, but true != 1 and "1" != 1."""
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return float(a) == float(b)
    return type(a) is type(b) and a == b


def short(value):
    try:
        s = dumps_request(value)
    except (RecursionError, ValueError):
        s = repr(value)
    return s if len(s) <= 200 else s[:200] + "..."


def first_difference(a, b):
    """The first (path, a, b) where two JSON values differ, in document order; object keys in
    order."""
    try:
        if json.dumps(a) == json.dumps(b):
            return None
    except (RecursionError, ValueError):
        pass
    stack = [("value", a, b, "")]
    while stack:
        kind, a, b, path = stack.pop()
        if kind == "length":
            if a != b:
                return (f"{path}.length", str(a), str(b))
            continue
        if isinstance(a, dict) and isinstance(b, dict):
            ka, kb = list(a), list(b)
            if ka != kb:
                i = 0
                while i < min(len(ka), len(kb)) and ka[i] == kb[i]:
                    i += 1
                return (f"{path}{{keys from #{i}}}", short(ka[i:]), short(kb[i:]))
            for k in reversed(ka):
                stack.append(("value", a[k], b[k], f"{path}.{k}"))
        elif isinstance(a, list) and isinstance(b, list):
            stack.append(("length", len(a), len(b), path))
            for i in reversed(range(min(len(a), len(b)))):
                stack.append(("value", a[i], b[i], f"{path}[{i}]"))
        elif not same_scalar(a, b):
            return (path or "(value)", short(a), short(b))
    return None


# --- what each side threw -------------------------------------------------------------------

def error_text(error):
    """An error as JS's String(error) writes it: `Name: message`."""
    if isinstance(error, dict):
        name, message = error.get("name") or "", error.get("message") or ""
        if not name:
            return message
        return f"{name}: {message}" if message else name
    return str(error)


def stage_error(answer, stage):
    """What `answer` says the stage threw (the first step on its way that threw), or None."""
    failed = answer.get("failed") or {}
    for step in CHAIN[stage]:
        if step in failed:
            return error_text(failed[step])
    if "threw" in answer:
        if "threwName" in answer:
            return error_text({"name": answer["threwName"], "message": answer["threw"]})
        return error_text(answer["threw"])
    return None


# --- running both sides ---------------------------------------------------------------------

class Crash(Exception):
    pass


def limit_memory(megabytes):
    """A preexec_fn: the child's address space capped, so that a runaway allocation kills it
    alone (an abort, counted as a crash), not the capped scope compare.py runs in."""
    if not megabytes:
        return None

    def limit():
        size = megabytes * 1024 * 1024
        resource.setrlimit(resource.RLIMIT_AS, (size, size))
    return limit


def run_once(command, requests, timeout, memory=None):
    """Answers to `requests`, in order, as far as `command` got: (answers, None) or (answers
    so far, why it stopped)."""
    text = "".join(dumps_request(r) + "\n" for r in requests)
    with tempfile.TemporaryFile("w+b") as stdin:
        stdin.write(text.encode("utf-8"))
        stdin.seek(0)
        try:
            done = subprocess.run(command, stdin=stdin, capture_output=True, cwd=CRATE,
                                  timeout=timeout, preexec_fn=limit_memory(memory))
            out, code, err = done.stdout, done.returncode, done.stderr
        except subprocess.TimeoutExpired as e:
            out, code, err = e.stdout or b"", "timeout", e.stderr or b""
    lines = out.decode("utf-8").split("\n")
    # A line cut off by the crash is not an answer.
    complete = lines[:-1] if code != 0 else lines
    answers = []
    for line in complete:
        if line.strip():
            answers.append(loads(line))
    if code == 0 and len(answers) == len(requests):
        return answers, None
    why = f"exit {code}" if code != "timeout" else f"timed out after {timeout} s"
    tail = err.decode("utf-8", "replace").strip().splitlines()[-1:] if err else []
    return answers, why + (f" ({tail[0][:200]})" if tail else "")


def run(command, requests, timeout, prefix, memory=None):
    """Every request's answer, as `(answer, surrogates)`; a request the process died on gets
    `({"id", "crashed": why}, False)`. `prefix`: the process writes each answer before it
    reads the next request, so the one it died on is the first unanswered (else halves are
    run until the one is found). `memory`: the process's address space in MB."""
    out = []
    todo = list(requests)
    while todo:
        answers, why = run_once(command, todo, timeout, memory)
        if why is None:
            out += answers
            break
        if prefix or len(todo) == 1:
            out += answers
            if len(answers) < len(todo):
                out.append(({"id": todo[len(answers)]["id"], "crashed": why}, False))
            todo = todo[len(answers) + 1:]
            continue
        half = len(todo) // 2
        out += run(command, todo[:half], timeout, prefix, memory)
        todo = todo[half:]
    for r, (a, _) in zip(requests, out):
        if a.get("id") != r["id"] and "harness" not in a:
            raise Crash(f"{shlex.join(command)}: answer {a.get('id')!r} for request {r['id']!r}")
    return out


def rust_command(args):
    if args.rust_bin:
        return [args.rust_bin]
    return ["cargo", "run", "--release", "--quiet", "--example", "next_compare", "--"]


def oracle_command(args):
    # A heap V8 gives up on (an abort the batch survives) before the capped scope does.
    return [args.node, f"--max-old-space-size={args.oracle_memory}",
            os.path.join(HERE, "oracle.cjs"), args.markdoc]


# --- inputs ---------------------------------------------------------------------------------

def input_sets(args):
    """[(name, iterator of raw lines)]: each input file, and the conformance cases."""
    sets = []
    for path in args.inputs:
        def lines(path=path):
            with open(path, encoding="utf-8", newline="") as f:
                for line in f:
                    yield line
        sets.append((os.path.basename(path).removesuffix(".jsonl"), lines))
    if args.conformance:
        def cases():
            done = subprocess.run(rust_command(args) + ["--conformance"], capture_output=True,
                                  stdin=subprocess.DEVNULL, cwd=CRATE, check=True)
            yield from done.stdout.decode("utf-8").split("\n")
        sets.append(("conformance", cases))
    return sets


def documents(args, lines, template):
    """The documents of one set, each `(doc, held lone surrogates)`."""
    n = 0
    for line in lines():
        if not line.strip():
            continue
        # read exactly: a lone surrogate in an input reaches both sides as it is
        line_doc = json.loads(line)
        surrogates = SURROGATE_ESCAPE.search(line) is not None
        doc = {**template, **line_doc}
        if "code" not in doc:
            doc["code"] = doc.pop("src", "")
        doc.pop("src", None)
        if args.id_prefix and not str(doc["id"]).startswith(args.id_prefix):
            continue
        n += 1
        if args.limit and n > args.limit:
            return
        yield doc, surrogates


def batches(items, size):
    batch = []
    for item in items:
        batch.append(item)
        if len(batch) == size:
            yield batch
            batch = []
    if batch:
        yield batch


def tokenizer_options(doc):
    return {"allowIndentation": doc.get("allowIndentation", True) is not False,
            "allowComments": doc.get("allowComments", True) is not False,
            **doc.get("tokenizer", {})}


def oracle_request(doc, outputs):
    req = {"id": doc["id"], "code": doc["code"], "tokenizer": tokenizer_options(doc),
           "outputs": outputs}
    for key in REQUEST_KEYS:
        if key in doc and key != "tokenizer":
            req[key] = doc[key]
    return req


def rust_request(doc, stages):
    req = {"id": doc["id"], "stages": list(stages), "code": doc["code"],
           "tokenizer": tokenizer_options(doc)}
    # A document's own `tokens` (a hand-made token stream) are parsed instead of `code`'s,
    # and its own `ast` is transformed instead of the parsed one.
    for key in ("config", "slots", "file", "parse", "formatOptions", "components", "renderTree",
                "tokens", "ast"):
        if key in doc:
            req[key] = doc[key]
    partials = doc.get("partials") or (doc.get("config") or {}).get("partials")
    if partials:
        req["partials"] = partials
    return req


# --- comparing ------------------------------------------------------------------------------

def compare_one(stage, o, p):
    """(tally key, where, oracle's, port's) for one stage of one document."""
    key = OUTPUT[stage]
    if "harness" in o or "harness" in p:
        return ("harness", "request", o.get("harness", ""), p.get("harness", ""))
    if "crashed" in p:
        return ("port crashed", "process", "", p["crashed"])
    if "crashed" in o:
        return ("harness", "oracle process", o["crashed"], "")
    o_unw = (o.get("unwritable") or {}).get(key)
    p_unw = (p.get("unwritable") or {}).get(key)
    if o_unw is not None and p_unw is not None:
        # Both made the value and JSON.stringify threw writing it (a `toJSON` that throws, a
        # value past the stack): the same error agrees.
        a, b = error_text(o_unw), error_text(p_unw)
        return ("both unwritable" if a == b else "unwritable differently", key, a, b)
    if o_unw is not None:
        if error_text(o_unw) == STACK_OVERFLOW:
            return ("oracle unwritable", key, error_text(o_unw), "")
        return ("unwritable differently", key, error_text(o_unw), "(written)")
    if p_unw is not None and key in o:
        return ("port unwritable", key, "(written)", error_text(p_unw))
    o_has, p_has = key in o, key in p
    o_err = None if o_has else stage_error(o, stage)
    p_err = None if p_has else stage_error(p, stage)
    if not o_has and not p_has:
        if o_err == p_err:
            return ("both threw", "", o_err, p_err)
        return ("threw differently", "threw", o_err, p_err)
    if not p_has:
        return ("port threw", "threw", "(no throw)", p_err)
    if not o_has:
        return ("oracle threw", "threw", o_err, "(no throw)")
    d = first_difference(o[key], p[key])
    if d:
        return ("different",) + d
    return ("equal", "", "", "")


class Run:
    """Totals and differences of one compare.py invocation."""

    def __init__(self, args):
        self.args = args
        self.lock = threading.Lock()
        self.shown = 0
        self.keep = None
        if args.keep:
            os.makedirs(args.keep, exist_ok=True)
            self.keep = open(os.path.join(args.keep, "differences.jsonl"), "w",
                             encoding="utf-8")
        self.grand = collections.defaultdict(lambda: dict.fromkeys(TALLY, 0))
        self.surrogates = 0

    def report(self, label, doc, stage, result):
        kind, where, a, b = result
        with self.lock:
            if self.keep:
                self.keep.write(dumps_request({"set": label, "stage": stage, "id": doc["id"],
                                               "kind": kind, "where": where, "oracle": a,
                                               "port": b, "doc": doc}) + "\n")
            if self.shown < self.args.show:
                self.shown += 1
                print(f"  {label} {doc['id']}: {stage}: {kind} {where}: oracle {a} | port {b}",
                      flush=True)

    def close(self):
        if self.keep:
            self.keep.close()


def compare_batch(args, stages, docs):
    """[(doc, {stage: result})] and how many oracle answers held lone surrogates."""
    from_oracle = args.from_oracle
    outputs = [OUTPUT[s] for s in stages]
    if from_oracle:
        stage = stages[0]
        outputs.append(INPUT[stage])
        if stage == "transform":
            outputs.append("partials")
    oracle = run(oracle_command(args), [oracle_request(d, outputs) for d in docs],
                 args.timeout, prefix=False)
    surrogates = sum(1 for _, s in oracle if s)
    oracle = [o for o, _ in oracle]
    requests, skipped, index = [], {}, {}
    for i, (doc, o) in enumerate(zip(docs, oracle)):
        req = rust_request(doc, stages)
        if from_oracle:
            source = INPUT[stages[0]]
            if source not in o:
                skipped[i] = stage_error(o, INPUT_STAGE[stages[0]]) or \
                    o.get("harness", "no " + source)
                continue
            req["tree" if stages[0] in TREE_STAGES else source] = o[source]
            if stages[0] == "transform" and "partials" in o:
                req["partialAsts"] = o["partials"]
        index[i] = len(requests)
        requests.append(req)
    # Answers in request order (an unreadable request's answer may have no id).
    port = [a for a, _ in run(rust_command(args), requests, args.timeout, prefix=True,
                              memory=args.port_memory)]
    # A document the port died on, asked for several stages: each stage alone, so the ones
    # before the one that kills it are still compared.
    alone = {}
    if len(stages) > 1:
        for j, req in enumerate(requests):
            if "crashed" in port[j]:
                for stage in stages:
                    one = dict(req, stages=[stage])
                    [(answer, _)] = run(rust_command(args), [one], args.timeout,
                                        prefix=True, memory=args.port_memory)
                    alone[(j, stage)] = answer
    results = []
    for i, (doc, o) in enumerate(zip(docs, oracle)):
        if i in skipped:
            results.append((doc, {s: ("skipped", "input", skipped[i], "") for s in stages}))
            continue
        j = index[i]
        results.append((doc, {s: compare_one(s, o, alone.get((j, s), port[j]))
                              for s in stages}))
    return results, surrogates


# The stage whose output an --from-oracle input is.
INPUT_STAGE = {"ast": "tokens", "transform": "ast", "errors": "ast", "html": "transform",
               "format": "ast", "reactStatic": "transform", "react": "transform"}


def compare_set(args, stages, run_state, label, docs):
    """One input set under one template: every document through both sides, in batches."""
    tally = collections.defaultdict(lambda: dict.fromkeys(TALLY, 0))
    count = surrogates_in = surrogates_out = 0

    def batch_docs():
        nonlocal count, surrogates_in
        for doc, s in docs:
            count += 1
            surrogates_in += s
            yield doc

    def work(batch):
        return compare_batch(args, stages, batch)

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        pending = collections.deque()
        source = batches(batch_docs(), args.batch)
        for batch in source:
            pending.append(pool.submit(work, batch))
            # Keep at most two batches per job in flight: memory stays flat.
            while len(pending) >= 2 * args.jobs:
                surrogates_out += collect(pending.popleft().result(), stages, tally,
                                          run_state, label)
        while pending:
            surrogates_out += collect(pending.popleft().result(), stages, tally, run_state,
                                      label)
    for stage in stages:
        t = tally[stage]
        for k, v in t.items():
            run_state.grand[stage][k] += v
        title = f"stage {stage}" + (f", from the oracle's {INPUT[stage]}"
                                    if args.from_oracle else "")
        print(f"{label}: {title}: {count} documents: "
              + ", ".join(f"{v} {k}" for k, v in t.items() if v or k in ("equal",))
              + (f"; lone surrogates in {surrogates_in} inputs (sent exactly), made U+FFFD "
                 f"outside the render tree in {surrogates_out} oracle answers"
                 if surrogates_in or surrogates_out else ""),
              flush=True)
    run_state.surrogates += surrogates_out
    return count


def collect(batch_result, stages, tally, run_state, label):
    results, surrogates = batch_result
    for doc, per_stage in results:
        for stage in stages:
            result = per_stage[stage]
            tally[stage][result[0]] += 1
            if result[0] in BAD or result[0] in ("oracle unwritable",):
                run_state.report(label, doc, stage, result)
    return surrogates


def compare_roundtrip(args, docs):
    outputs = ["tokens", "ast", "transformed", "partials"]
    tally = {"equal": 0, "different": 0, "port threw": 0}
    counted = {k: 0 for k in outputs}
    report = []
    for batch in batches(docs, args.batch):
        oracle = [o for o, _ in run(oracle_command(args),
                                    [oracle_request(d, outputs) for d, _ in batch],
                                    args.timeout, prefix=False)]
        requests = [{"id": o["id"], "stage": "roundtrip",
                     **{k: o[k] for k in outputs if k in o}} for o in oracle]
        port = [p for p, _ in run(rust_command(args), requests, args.timeout, prefix=True,
                                  memory=args.port_memory)]
        for o, p in zip(oracle, port):
            if "threw" in p or "crashed" in p:
                tally["port threw"] += 1
                report.append((o["id"], "port threw", p.get("threw", p.get("crashed")), ""))
                continue
            diff = None
            for k in outputs:
                if k not in o:
                    continue
                counted[k] += 1
                diff = diff or first_difference(o[k], p.get(k))
                if diff and not diff[0].startswith(k):
                    diff = (k + diff[0],) + diff[1:]
            if diff:
                tally["different"] += 1
                report.append((o["id"],) + diff)
            else:
                tally["equal"] += 1
    print("roundtrip (" + ", ".join(f"{counted[k]} {k}" for k in outputs) + "): "
          + ", ".join(f"{v} {k}" for k, v in tally.items()))
    for id_, where, a, b in report[: args.show]:
        print(f"  {id_}: {where}: " + (f"oracle {a} | port {b}" if b else a))
    return tally["equal"] == sum(tally.values())


def write_golden(args, docs):
    outputs = ["tokens", "ast", "errors", "transformed", "html", "partials"]
    n = surrogates = 0
    with open(args.golden, "w", encoding="utf-8") as f:
        for batch in batches(docs, args.batch):
            requests = [oracle_request(d, outputs) for d, _ in batch]
            answers = run(oracle_command(args), requests, args.timeout, prefix=False)
            for req, (answer, s) in zip(requests, answers):
                surrogates += s
                line = {k: v for k, v in req.items() if k != "outputs"}
                line.update({k: v for k, v in answer.items() if k != "id"})
                f.write(dumps_request(line, separators=(",", ":")) + "\n")
                n += 1
    print(f"wrote {n} documents to {args.golden}"
          + (f" ({surrogates} with lone surrogates made U+FFFD outside the render tree)" if surrogates else ""))


def templates(args):
    if args.templates:
        text = args.templates
        if os.path.exists(text):
            with open(text, encoding="utf-8") as f:
                text = f.read()
        return json.loads(text)
    return [json.loads(args.template) if args.template else {}]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("inputs", nargs="*")
    ap.add_argument("--stage", choices=STAGES + API_STAGES + ("all", "api", "every"))
    ap.add_argument("--roundtrip", action="store_true")
    ap.add_argument("--golden", metavar="OUT")
    ap.add_argument("--conformance", action="store_true")
    ap.add_argument("--from-oracle", action="store_true")
    ap.add_argument("--markdoc", default=os.environ.get("MARKDOC_DIST"),
                    help="upstream's dist/index.js (default: $MARKDOC_DIST)")
    ap.add_argument("--template")
    ap.add_argument("--templates")
    ap.add_argument("--rust-bin")
    ap.add_argument("--node", default="node")
    ap.add_argument("--id-prefix")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--show", type=int, default=10)
    ap.add_argument("--keep")
    ap.add_argument("--batch", type=int, default=400)
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--port-memory", type=int, default=1024,
                    help="the port's address space, MB (0: unlimited)")
    ap.add_argument("--oracle-memory", type=int, default=1024,
                    help="node's --max-old-space-size, MB")
    args = ap.parse_args()
    if not args.markdoc:
        ap.error("--markdoc DIST/index.js (or $MARKDOC_DIST) is required")
    if [bool(args.stage), args.roundtrip, bool(args.golden)].count(True) != 1:
        ap.error("give one of --stage STAGE, --roundtrip, --golden OUT")
    if args.from_oracle and args.stage in ("tokens", "all", "api", "every"):
        ap.error("--from-oracle: give one stage after tokens (ast, errors, transform, html, "
                 "format, reactStatic, react)")
    sets = input_sets(args)
    if not sets:
        ap.error("no inputs (give INPUTS.jsonl files or --conformance)")
    if args.golden or args.roundtrip:
        docs = (d for _, lines in sets for d in documents(args, lines, templates(args)[0]))
        if args.golden:
            return write_golden(args, docs)
        sys.exit(0 if compare_roundtrip(args, docs) else 1)
    stages = {"all": STAGES, "api": API_STAGES, "every": STAGES + API_STAGES}.get(
        args.stage, (args.stage,))
    state = Run(args)
    total = 0
    try:
        for template in templates(args):
            for name, lines in sets:
                label = (json.dumps(template, ensure_ascii=False) + " " if template else "") + name
                total += compare_set(args, stages, state, label, documents(args, lines, template))
    finally:
        state.close()
    print(f"total: {total} documents")
    bad = 0
    for stage in stages:
        t = state.grand[stage]
        bad += sum(t[k] for k in BAD)
        print(f"  {stage}: " + ", ".join(f"{v} {k}" for k, v in t.items() if v))
    if state.surrogates:
        print(f"  {state.surrogates} oracle answers held lone surrogates, made U+FFFD outside the render tree")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
