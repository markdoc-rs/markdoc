'use strict';
// Writes tests/next/realm/conversions.jsonl: what node gives when upstream's conversions call
// each built-in function of the realm's table (src/next/realm/table.rs), for the unit tests
// in src/next/realm/tests.rs. Upstream calls a built-in only where it converts a value:
// String(value) calls the `toString` (else the `valueOf`) the value's chain finds with no
// argument, and JSON.stringify calls a `toJSON` with the member's key. So each function is
// called both ways, from an object that holds it and is shaped as a document can shape one
// (plain, or like an array), and every built-in object is converted on its own.
//
// Left out: what the port documents as a divergence (src/next/realm/mod.rs): a body compiled
// by Function or GeneratorFunction (a SyntaxError for most keys), a RegExp source V8 rejects
// (its SyntaxError's message is the regex engine's), a locale V8 would format with locale
// data. Each of those is pinned by a test of its own.
//
// Usage: node tools/gen_realm_conversions.cjs <@markdoc/markdoc 0.5.11 dist/index.js> [--check]
//   --check   compares instead of writing; exits 1 when the checked-in file differs.
// Each line: {kind: "string"|"json", value: <desc>, key?: string, out: <desc>|{"t": "Name:
// message"}}; descs as work/realm/convfuzz.cjs writes them (null, booleans, finite numbers,
// well-formed strings; {"u":1} undefined; {"n": "NaN"|...}; {"b": name} a built-in;
// {"a": [...]} an array; {"o": [[key, desc]...]} a plain object).

const fs = require('fs');
const path = require('path');

const args = process.argv.slice(2);
const check = args.includes('--check');
const dist = args.find((a) => !a.startsWith('--'));
if (!dist) {
  console.error('usage: node tools/gen_realm_conversions.cjs <markdoc dist/index.js> [--check]');
  process.exit(2);
}
const Markdoc = require(path.resolve(dist));
const { discover } = require('./gen_realm.cjs');
const { names, order } = discover(Markdoc);
const outFile = path.join(__dirname, '..', 'tests', 'next', 'realm', 'conversions.jsonl');
process.on('unhandledRejection', () => {});

function enc(v) {
  if (v === undefined) return { u: 1 };
  if (v === null || typeof v === 'boolean') return v;
  if (typeof v === 'number') {
    if (Object.is(v, -0)) return { n: '-0' };
    return Number.isFinite(v) ? v : { n: String(v) };
  }
  if (typeof v === 'string') {
    if (v.isWellFormed()) return v;
    return { s16: Array.from({ length: v.length }, (_, i) => v.charCodeAt(i)) };
  }
  if (names.has(v)) return { b: names.get(v) };
  if (Array.isArray(v)) return { a: v.map(enc) };
  return { o: Object.keys(v).map((k) => [k, enc(v[k])]) };
}

const divergent = (out) =>
  typeof out.t === 'string' &&
  (out.t.startsWith('SyntaxError') || out.t.includes('Incorrect locale information'));

const lines = [];
function emit(kind, value, key) {
  // Before the conversion: a call can set properties of the value (Array.prototype.pop).
  const before = enc(value);
  let out;
  try {
    if (kind === 'string') out = enc(String(value));
    else {
      const s = JSON.stringify(key === undefined ? value : { [key]: value });
      out = s === undefined ? { u: 1 } : enc(s);
    }
  } catch (e) {
    out = { t: `${e.name}: ${e.message}` };
  }
  if (divergent(out)) return;
  const line = { kind, value: before };
  if (key !== undefined) line.key = key;
  line.out = out;
  lines.push(JSON.stringify(line));
}

const fnsNotCompiling = new Set([Function, Object.getPrototypeOf(function* () {}).constructor, RegExp]);
for (const o of order) {
  emit('string', o);
  emit('json', o);
  if (typeof o !== 'function') continue;
  // A key every function takes as it is: a body that compiles, a pattern V8 accepts.
  const key = fnsNotCompiling.has(o) ? 'a' : 'k';
  emit('string', { toString: o });
  emit('string', { toString: o, length: 2, 0: 'a', 1: 3 });
  emit('string', { toString: null, valueOf: o, 0: 'x', length: 1 });
  emit('json', { toJSON: o }, key);
  emit('json', { toJSON: o, length: 1, 0: 3.5 }, key);
  emit('json', { toJSON: o, length: '2', 0: 'é', 1: null });
  emit('json', [{ toJSON: o }]);
}

const text = lines.join('\n') + '\n';
if (check) {
  const current = fs.existsSync(outFile) ? fs.readFileSync(outFile, 'utf8') : '';
  if (current !== text) {
    console.error(`${path.relative(process.cwd(), outFile)} differs from what node ${process.version} gives`);
    process.exit(1);
  }
  console.log(`${path.relative(process.cwd(), outFile)}: ${lines.length} conversions, as node ${process.version} gives them`);
} else {
  fs.mkdirSync(path.dirname(outFile), { recursive: true });
  fs.writeFileSync(outFile, text);
  console.log(`wrote ${path.relative(process.cwd(), outFile)}: ${lines.length} conversions`);
}
