'use strict';
// Engine-only oracle for the markdown-it port: the patched markdown-it 15.0.1 that
// upstream Markdoc 0.5.11 bundles, without Markdoc's plugins.
// `new Markdoc.Tokenizer().parser` is that bundle's MarkdownIt instance, and its
// `.constructor` is the patched MarkdownIt class.
//
// Usage: node oracle_tokens.cjs <@markdoc/markdoc dist/index.js> < requests.jsonl > tokens.jsonl
//   request: {id, src, preset?: "default"|"commonmark"|"zero", options?: {markdown-it options,
//             e.g. allowIndentation, typographer, html}, enable?: [rule names],
//             disable?: [rule names], mode?: "parse"|"parseInline",
//             env?: the env to parse with, e.g. {references: {LABEL: {href, title}}}}
//            (`new MarkdownIt(preset, options)`, then `enable(enable)`, then `disable(disable)`)
//   output:  one line per request: {id, tokens, env?} (tokens in plain() form below; env only
//            when the parse left keys in it), or {id, error} when markdown-it throws.
// The Rust side is examples/mdit_tokens.rs; tests/markdown_it/compare.py runs both.
const path = require('path');
const readline = require('readline');

const Markdoc = require(path.resolve(process.argv[2]));
const MarkdownIt = new Markdoc.Tokenizer().parser.constructor;

// The token fields a port must match, in this order (as task5's node_tokens.cjs).
const FIELDS = ['type', 'tag', 'attrs', 'map', 'nesting', 'level', 'content', 'markup', 'info', 'meta', 'block', 'hidden', 'errors', 'position'];
function plain(t) {
  const o = {};
  for (const f of FIELDS) {
    const v = t[f];
    if (v === null || v === undefined || v === '' || (f === 'hidden' && v === false)) continue;
    o[f] = v;
  }
  if (Array.isArray(t.children)) o.children = t.children.map(plain);
  return o;
}

const cache = new Map();
function parser(req) {
  const key = JSON.stringify([req.preset ?? null, req.options ?? null, req.enable ?? null, req.disable ?? null]);
  if (!cache.has(key)) {
    const md = req.preset ? new MarkdownIt(req.preset, req.options) : new MarkdownIt(req.options);
    if (req.enable) md.enable(req.enable);
    if (req.disable) md.disable(req.disable);
    cache.set(key, md);
  }
  return cache.get(key);
}

function run(req) {
  const md = parser(req);
  const env = req.env || {};
  const tokens = req.mode === 'parseInline' ? md.parseInline(req.src, env) : md.parse(req.src, env);
  // JSON round trip first, as node_tokens.cjs does (drops undefined, class prototypes)
  const out = { id: req.id, tokens: JSON.parse(JSON.stringify(tokens)).map(plain) };
  const envOut = JSON.parse(JSON.stringify(env));
  if (Object.keys(envOut).length) out.env = envOut;
  return out;
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on('line', (l) => {
  if (!l.trim()) return;
  const req = JSON.parse(l);
  let out;
  try {
    out = run(req);
  } catch (e) {
    out = { id: req.id, error: String((e && e.message) || e) };
  }
  process.stdout.write(JSON.stringify(out) + '\n');
});
