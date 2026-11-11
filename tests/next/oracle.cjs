// Upstream Markdoc as an oracle for the next layer (tests/next/compare.py runs it).
// task5's oracle/bin/oracle.cjs, extended: a request it answered without a throw is answered
// byte for byte the same; when a stage throws, see "threw" below.
//
// Usage: node oracle.cjs <path to @markdoc/markdoc 0.5.11's dist/index.js> < requests.jsonl
//
// One JSON request per stdin line:
//   {"id", "code", "config"?, "allowIndentation"?, "allowComments"?,
//    "slots"?, "file"?, "partials"?: {name: source}}
// and, added:
//   "outputs"?: [...]   what to answer: any of tokens, ast, errors, transformed, html,
//                       partials. Default: ast, errors, transformed, html (as before).
//   "tokenizer"?: {}    more Tokenizer options, over allowIndentation/allowComments (both
//                       still default true): allowLinkValidation, linkValidationOptions, and
//                       markdown-it's own (html, typographer, breaks, quotes, ...).
//   "lossyTokens"?: true  token strings' lone surrogates U+FFFD at the end of tokenizing (see
//                       tokenizerFor).
//   "parse"?: {}        more ParserArgs, over {file, slots, location: true}: location,
//                       conditionalTags.
//   "tokens"?: [...]    parse these tokens (the "tokens" output's form, or a shorter one)
//                       instead of tokenizing `code`. A field left out reads back as
//                       markdown-it's default ('' for content, markup, info and tag), $$mdtype
//                       objects in meta as Variable and Function: what parser.ts reads is the
//                       original's.
//   "ast"?: {...}       validate and transform this AST (the "ast" output's form, revived by
//                       Markdoc.Ast.fromJSON) instead of parsing.
// and, for the api stages (formatted, reactStatic, react; see apiStages below):
//   "formatOptions"?: {} Markdoc.format's options (allowIndentation, maxTagOpeningWidth: a
//                       number, or "Infinity" (JSON has no Infinity; `x > "Infinity"` is
//                       `x > Infinity`), orderedListMode, indent).
//   "components"?: {} | "$function"   renderers.react's components: an object (each value
//                       what the fake React records as the element's type), or "$function" for
//                       `(name) => ({$component: name})`.
//   "renderTree"?: any  render this tree (the "transformed" output's form, or any JSON) for
//                       html, reactStatic and react instead of transforming the document
//                       (a {"$$mdtype": "Tag"} object stays a plain object: the renderers read
//                       it as a tag, as they read a Tag).
// In "config", an object {"$regex": source, "flags"?: flags} is the RegExp (for `matches`),
// and an object {"$type": name} is a constructor (String, Number, Boolean, Object, Array) or
// one of upstream's classes (Class, Id, ConditionalAttributeType, PartialFile), for a `type`
// or `returns`; everything else is the JSON as given ("type": "String" is upstream's own
// string form), as JSON.parse makes it (a "__proto__" key is an own property).
//
// One JSON answer per line: {id, ...the outputs asked for}. Each output is what
// JSON.stringify writes of upstream's value, whole, keys in its order:
//   tokens       the token stream: markdown-it's tokens with every field (map, level, children,
//                content, markup, info, block, hidden, type, tag, attrs, nesting, meta, and
//                errors where a plugin set them), parseTags' objects in a fence's children
//                with theirs ({type, start, end, content}; {map, position, start, end, info,
//                type, nesting?, meta})
//   ast          the AST: every field JSON.stringify writes (Variable and Function as
//                {"$$mdtype": ...} objects), lines and location included
//   errors       validate(ast, config): [{type, lines, location?, error: {id, level, message,
//                location?}}]
//   transformed  transform(ast, {...config, partials}), the partials parsed: the render tree
//   html         renderers.html(transformed)
//   partials     the partials' ASTs, {name: ast}
//   formatted    Markdoc.format(ast, formatOptions)
//   reactStatic  Markdoc.renderers.reactStatic(transformed, {resolveTagName}): the JS source.
//                resolveTagName is the dist's own default with its name restored: esbuild
//                renamed the default `tagName` to `tagName2` in the bundle, and reactStatic
//                throws "resolveTagName must be named tagName" for any other name, so the dist
//                cannot run its own default (see STATIC_TAG_NAME).
//   react        Markdoc.renderers.react(transformed, React, {components}) with a fake React
//                that records each call: {calls: [{type, props, children}], result}, in the
//                order createElement was called; an element is {"$element": i} (the call that
//                made it), React.Fragment {"$fragment": true}, and a value JSON cannot hold
//                {"$undefined": true}, {"$number": "NaN" | "Infinity" | "-Infinity" | "-0"},
//                {"$function": name}, {"$hole": true}.
// The stages are tokens, ast, errors, partials (parsing the partials), transformed and html.
// When a stage throws: "threw" (its message; as before, the first), "threwName" (that error's
// name: TypeError, RangeError, ...), "threwAt" (that stage) and "failed" ({stage: {name,
// message}} for each stage that threw). A stage that needs a failed one is left out; errors
// and transformed do not need each other.
// When upstream made a value but JSON.stringify cannot write it (an AST nested deeper than
// V8's stack allows the writer, a sparse array too long to write), the output is left out
// and "unwritable" holds {output: {name, message}}: the stage did not throw, the oracle just
// cannot show what it made. Anything else that goes wrong for one request (a config too deep
// to revive) is {id, harness: message}; the other requests are answered as usual.
'use strict';
const path = require('path');
const readline = require('readline');
const Markdoc = require(path.resolve(process.argv[2]));

const DEFAULT_OUTPUTS = ['ast', 'errors', 'transformed', 'html'];

function strip(value) {
  // Plain JSON with the AST's methods and circular parents left behind.
  return JSON.parse(JSON.stringify(value, (k, v) => (k === '$$lines' ? undefined : v)));
}

const TYPES = {
  String, Number, Boolean, Object, Array,
  Class: Markdoc.globalAttributes.class.type,
  Id: Markdoc.globalAttributes.id.type,
  ConditionalAttributeType: Markdoc.tags.if.attributes.primary.type,
  PartialFile: Markdoc.tags.partial.attributes.file.type,
};

function reviveConfig(value) {
  if (Array.isArray(value)) return value.map(reviveConfig);
  if (value && typeof value === 'object') {
    if (typeof value.$regex === 'string') return new RegExp(value.$regex, value.flags || '');
    if (typeof value.$type === 'string') return TYPES[value.$type];
    // As JSON.parse made it: every key an own property, `__proto__` included (an assignment
    // would call Object.prototype's setter instead).
    const out = {};
    for (const [k, v] of Object.entries(value))
      Object.defineProperty(out, k, {
        value: reviveConfig(v), writable: true, enumerable: true, configurable: true,
      });
    return out;
  }
  return value;
}

function reviveTokens(tokens) {
  const revived = Markdoc.Ast.fromJSON(JSON.stringify(tokens));
  const fill = (t) => {
    for (const f of ['content', 'markup', 'info', 'tag']) if (t[f] === undefined) t[f] = '';
    if (Array.isArray(t.children)) t.children.forEach(fill);
  };
  revived.forEach(fill);
  return revived;
}

// What a throw was: an Error's name and message (anything else thrown: its String()).
function thrown(e) {
  if (e && typeof e === 'object' && 'message' in e)
    return { name: String(e.name), message: String(e.message) };
  return { name: '', message: String(e) };
}

const tokenizers = new Map();
function tokenizerFor(req) {
  const options = {
    allowIndentation: req.allowIndentation !== false,
    allowComments: req.allowComments !== false,
    ...(req.tokenizer || {}),
  };
  const key = JSON.stringify(options) + (req.lossyTokens ? ' lossy' : '');
  if (!tokenizers.has(key)) {
    const tokenizer = new Markdoc.Tokenizer(options);
    // `lossyTokens`: every lone surrogate in the tokens' strings is U+FFFD once tokenizing
    // ends (a last core rule), as the port's token strings are (Rust `String`s): what
    // upstream gives with the port's boundary, to tell a difference that boundary makes
    // from any other (work/utf16).
    if (req.lossyTokens) {
      const lone = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g;
      const fix = (s) => (typeof s === 'string' ? s.replace(lone, '\ufffd') : s);
      const walk = (tokens) => {
        for (const t of tokens || []) {
          t.content = fix(t.content);
          t.markup = fix(t.markup);
          t.info = fix(t.info);
          if (Array.isArray(t.attrs)) t.attrs = t.attrs.map(([k, v]) => [fix(k), fix(v)]);
          walk(t.children);
        }
      };
      tokenizer.parser.core.ruler.push('lossy_tokens', (state) => walk(state.tokens));
    }
    tokenizers.set(key, tokenizer);
  }
  return tokenizers.get(key);
}

function answer(req) {
  const outputs = new Set(req.outputs || DEFAULT_OUTPUTS);
  const out = { id: req.id };
  const failed = {};
  const unwritable = {};
  const stage = (name, fn) => {
    try {
      return { ok: true, value: fn() };
    } catch (e) {
      const error = thrown(e);
      if (out.threw === undefined) {
        out.threw = error.message;
        out.threwName = error.name;
        out.threwAt = name;
      }
      failed[name] = error;
      return { ok: false };
    }
  };
  // `out[name] = write()`, unless JSON.stringify cannot write the value.
  const write = (name, fn) => {
    try {
      out[name] = fn();
    } catch (e) {
      unwritable[name] = thrown(e);
    }
  };
  const given = 'renderTree' in req;
  const needAst = ['ast', 'errors', 'transformed'].some((o) => outputs.has(o)) ||
    outputs.has('formatted') || (TREE_OUTPUTS.some((o) => outputs.has(o)) && !given);
  // Inside the stages that use it: `new Tokenizer(options)` throwing is the tokens stage's.
  const tokenizer = () => tokenizerFor(req);
  const parseArgs = (file) => ({ file, slots: !!req.slots, location: true, ...(req.parse || {}) });

  let tokens = { ok: false };
  if (outputs.has('tokens') || (needAst && !req.ast)) {
    tokens = stage('tokens', () =>
      req.tokens ? reviveTokens(req.tokens) : tokenizer().tokenize(req.code || ''));
    if (tokens.ok && outputs.has('tokens'))
      write('tokens', () => JSON.parse(JSON.stringify(tokens.value)));
  }

  let ast = { ok: false };
  if (needAst) {
    if (req.ast) ast = stage('ast', () => Markdoc.Ast.fromJSON(JSON.stringify(req.ast)));
    else if (tokens.ok) ast = stage('ast', () => Markdoc.parse(tokens.value, parseArgs(req.file)));
    if (ast.ok && outputs.has('ast')) write('ast', () => strip(ast.value));
  }

  const config = reviveConfig(req.config || {});
  let tree = { ok: false };
  if (ast.ok && outputs.has('errors')) {
    const errors = stage('errors', () => Markdoc.validate(ast.value, config));
    if (errors.ok) write('errors', () => JSON.parse(JSON.stringify(errors.value)));
  }

  const wantsTree = outputs.has('transformed') ||
    (TREE_OUTPUTS.some((o) => outputs.has(o)) && !given);
  if ((ast.ok && wantsTree) || outputs.has('partials')) {
    const partials = stage('partials', () => {
      const parsed = {};
      for (const [file, src] of Object.entries(req.partials || config.partials || {}))
        parsed[file] = Markdoc.parse(tokenizer().tokenize(src), parseArgs(file));
      return parsed;
    });
    if (partials.ok && outputs.has('partials')) write('partials', () => strip(partials.value));
    if (ast.ok && partials.ok && wantsTree) {
      const transformed = stage('transformed', () =>
        Markdoc.transform(ast.value, { ...config, partials: partials.value }));
      if (transformed.ok) {
        if (outputs.has('transformed')) write('transformed', () => strip(transformed.value));
        if (outputs.has('html') && !given) {
          const html = stage('html', () => Markdoc.renderers.html(transformed.value));
          if (html.ok) out.html = html.value;
        }
        tree = transformed;
      }
    }
  }
  apiStages(req, outputs, ast, given ? { ok: true, value: req.renderTree } : tree, stage, write, out);
  if (Object.keys(failed).length) out.failed = failed;
  if (Object.keys(unwritable).length) out.unwritable = unwritable;
  return out;
}

// --- the api stages: format, reactStatic, react -----------------------------------------

const API_OUTPUTS = ['formatted', 'reactStatic', 'react'];
const TREE_OUTPUTS = ['html', 'reactStatic', 'react'];

// reactStatic's default resolveTagName as upstream's source has it: the dist's own function
// (`tagName2` in the bundle, its text read from the dist itself) under its source name, so
// that `resolveTagName.name === 'tagName'` holds and the generated code says `function
// tagName(...)`.
const STATIC_TAG_NAME = (() => {
  const dist = require('fs').readFileSync(path.resolve(process.argv[2]), 'utf8');
  const at = dist.indexOf('// src/renderers/react/static.ts');
  const match = /function tagName2\(name, components\) \{\n[^\n]*\n\}/.exec(dist.slice(at));
  if (at < 0 || !match) throw new Error("the dist's reactStatic tagName2 was not found");
  return new Function('return ' + match[0].replace('function tagName2', 'function tagName'))();
})();

function formatOptions(req) {
  return { ...(req.formatOptions || {}) };
}

function componentsOf(req) {
  if (req.components === '$function') return (name) => ({ $component: name });
  return req.components || {};
}

// A fake React that records its calls (see the header).
function recordingReact() {
  const calls = [];
  const FRAGMENT = Object.freeze({ fragment: true });
  const elements = new Map();
  function enc(v) {
    // Iterative, so a deep value is written at any depth.
    const root = { v: undefined };
    const stack = [[v, root, 'v']];
    while (stack.length) {
      const [x, holder, key] = stack.pop();
      if (x === undefined) holder[key] = { $undefined: true };
      else if (x === null || typeof x === 'boolean' || typeof x === 'string') holder[key] = x;
      else if (typeof x === 'number') {
        if (Number.isNaN(x)) holder[key] = { $number: 'NaN' };
        else if (x === Infinity) holder[key] = { $number: 'Infinity' };
        else if (x === -Infinity) holder[key] = { $number: '-Infinity' };
        else if (Object.is(x, -0)) holder[key] = { $number: '-0' };
        else holder[key] = x;
      } else if (typeof x === 'function') holder[key] = { $function: x.name };
      else if (typeof x !== 'object') holder[key] = { $other: String(typeof x) };
      else if (x === FRAGMENT) holder[key] = { $fragment: true };
      else if (elements.has(x)) holder[key] = { $element: elements.get(x) };
      else if (Array.isArray(x)) {
        const out = new Array(x.length);
        holder[key] = out;
        for (let i = x.length - 1; i >= 0; i--) {
          if (i in x) stack.push([x[i], out, i]);
          else out[i] = { $hole: true };
        }
      } else {
        // a prototype-less object, so that an own `__proto__` key is written in its place
        // (on `{}`, `out.__proto__ = null` would set the prototype instead)
        const out = Object.create(null);
        holder[key] = out;
        const entries = Object.entries(x);
        for (const [k] of entries) out[k] = null;
        for (let i = entries.length - 1; i >= 0; i--) stack.push([entries[i][1], out, entries[i][0]]);
      }
    }
    return root.v;
  }
  const React = {
    Fragment: FRAGMENT,
    createElement(type, props, ...children) {
      const element = {};
      calls.push({ type: enc(type), props: enc(props), children: enc(children) });
      elements.set(element, calls.length - 1);
      return element;
    },
  };
  return { React, calls, enc };
}

function apiStages(req, outputs, ast, tree, stage, write, out) {
  if (ast.ok && outputs.has('formatted')) {
    const formatted = stage('formatted', () => Markdoc.format(ast.value, formatOptions(req)));
    if (formatted.ok) out.formatted = formatted.value;
  }
  if (!tree.ok) return;
  if ('renderTree' in req && outputs.has('html')) {
    const html = stage('html', () => Markdoc.renderers.html(tree.value));
    if (html.ok) out.html = html.value;
  }
  if (outputs.has('reactStatic')) {
    const code = stage('reactStatic', () =>
      Markdoc.renderers.reactStatic(tree.value, { resolveTagName: STATIC_TAG_NAME }));
    if (code.ok) out.reactStatic = code.value;
  }
  if (outputs.has('react')) {
    const rec = recordingReact();
    const rendered = stage('react', () =>
      Markdoc.renderers.react(tree.value, rec.React, { components: componentsOf(req) }));
    if (rendered.ok) write('react', () => ({ calls: rec.calls, result: rec.enc(rendered.value) }));
  }
}

// One line per request, whatever happens to it.
function respond(req) {
  try {
    return JSON.stringify(answer(req));
  } catch (e) {
    const { name, message } = thrown(e);
    return JSON.stringify({ id: req.id, harness: `${name}: ${message}` });
  }
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  if (!line.trim()) return;
  let req;
  try { req = JSON.parse(line); } catch (e) { process.stdout.write(JSON.stringify({ harness: 'bad request: ' + e.message }) + '\n'); return; }
  process.stdout.write(respond(req) + '\n');
});
