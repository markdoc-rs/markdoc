'use strict';
// Generates src/next/realm/table.rs: the built-in objects of a JavaScript realm that a value
// upstream Markdoc handles can reach by reading string-keyed properties, as node holds them.
//
// A value a variable, an attribute or a function's result can hold is a primitive, a plain
// object, an array, a Tag, a function or prototype read from one of those, or (through the
// harness's config convention {"$type": name}) a constructor or one of Markdoc's classes, or
// (through {"$regex": source}) a RegExp. Reading a property walks the value's prototype chain,
// so the table starts from the constructors and prototypes of those kinds (Object, Function,
// Array, String, Number, Boolean, RegExp, and Markdoc's Tag, Class, Id,
// ConditionalAttributeType and PartialFile from the dist) and adds, to a fixpoint, every
// object a string-keyed property of a table object holds and every table object's prototype
// (what `__proto__` reads). For each object it records:
//
//   - its name (the first path that reaches it), whether it is callable and a constructor,
//     its prototype, what `Object.prototype.toString` calls its class, whether it is an
//     array (Array.prototype is), the primitive a wrapper prototype holds ([[StringData]] '',
//     [[NumberData]] 0, [[BooleanData]] false), and for a function the text
//     `Function.prototype.toString` gives (a class's or a method's source from the dist);
//   - its own string-keyed properties in [[OwnPropertyKeys]] order, each a data property
//     (its value: a primitive or another table object) or an accessor (what its getter does,
//     one of the getters below, and whether it has a setter), with its attributes;
//   - what node gives for it (test expectations): typeof, String(o), JSON.stringify(o),
//     Object.keys(o).
//
// Every accessor's getter must be one this script knows (Object.prototype.__proto__,
// Function.prototype's restricted caller/arguments, RegExp.prototype's flag, source and
// flags getters, RegExp's legacy static properties); an unknown one stops the script, so a
// node that adds one is noticed.
//
// Usage: node tools/gen_realm.cjs <@markdoc/markdoc 0.5.11 dist/index.js> [--check]
//   --check   compares instead of writing; exits 1 when the checked-in table differs (the
//             node it runs on must be the version the table records).
// The dist is only required, never modified.

const fs = require('fs');
const path = require('path');

// Discovery: every object of the table, from the roots to a fixpoint (see the header).
// Returns {names: Map(object -> name), order: [object...], getters, constantGetters}.
function discover(Markdoc) {
  // The roots: each constructor (or class) and, named with it, its prototype.
  const roots = [
    ['Object', Object],
    ['Function', Function],
    ['Array', Array],
    ['String', String],
    ['Number', Number],
    ['Boolean', Boolean],
    ['RegExp', RegExp],
    ['Tag', Markdoc.Tag],
    ['Class', Markdoc.globalAttributes.class.type],
    ['Id', Markdoc.globalAttributes.id.type],
    ['ConditionalAttributeType', Markdoc.tags.if.attributes.primary.type],
    ['PartialFile', Markdoc.tags.partial.attributes.file.type],
    // Upstream's AST classes: a value can be an instance (a resolved Node in an attribute of a
    // hand-made AST, ...), whose prototype chain then starts at their prototypes.
    ['Node', Markdoc.Ast.Node],
    ['Variable', Markdoc.Ast.Variable],
    ['AstFunction', Markdoc.Ast.Function],
  ];

  // Objects reached by long paths that have a well-known name of their own (the intrinsics
  // behind a generator method: Node.prototype.walk is one).
  const GeneratorFunction = Object.getPrototypeOf(function* () {}).constructor;
  const wellKnown = new Map([
    [GeneratorFunction, 'GeneratorFunction'],
    [GeneratorFunction.prototype, 'GeneratorFunction.prototype'],
    [GeneratorFunction.prototype.prototype, 'Generator.prototype'],
    [Iterator, 'Iterator'],
    [Iterator.prototype, 'Iterator.prototype'],
  ]);

  // The getters the table may hold, by identity.
  const getters = new Map();
  const accessor = (o, k) => Object.getOwnPropertyDescriptor(o, k);
  getters.set(accessor(Object.prototype, '__proto__').get, 'Getter::Proto');
  for (const k of ['caller', 'arguments']) getters.set(accessor(Function.prototype, k).get, 'Getter::Restricted');
  const flagGetters = { hasIndices: 'd', global: 'g', ignoreCase: 'i', multiline: 'm', dotAll: 's', unicode: 'u', unicodeSets: 'v', sticky: 'y' };
  for (const [k, f] of Object.entries(flagGetters)) getters.set(accessor(RegExp.prototype, k).get, `Getter::RegExpFlag(b'${f}')`);
  getters.set(accessor(RegExp.prototype, 'source').get, 'Getter::RegExpSource');
  getters.set(accessor(RegExp.prototype, 'flags').get, 'Getter::RegExpFlags');
  const legacy = {
    input: 'Input', $_: 'Input', lastMatch: 'LastMatch', '$&': 'LastMatch', lastParen: 'LastParen', '$+': 'LastParen',
    leftContext: 'LeftContext', '$`': 'LeftContext', rightContext: 'RightContext', "$'": 'RightContext',
  };
  for (const [k, v] of Object.entries(legacy)) getters.set(accessor(RegExp, k).get, `Getter::RegExpLegacy(Legacy::${v})`);
  for (let i = 1; i <= 9; i++) getters.set(accessor(RegExp, '$' + i).get, `Getter::RegExpLegacy(Legacy::Paren(${i}))`);
  // Iterator.prototype.constructor: a getter that returns Iterator whatever `this` is.
  const constantGetters = new Map([[accessor(Iterator.prototype, 'constructor').get, Iterator]]);

  // Discovery.
  const names = new Map();
  const order = [];
  const queue = [];
  const isObject = (v) => v !== null && (typeof v === 'object' || typeof v === 'function');
  function add(name, o) {
    if (!isObject(o) || names.has(o)) return;
    names.set(o, wellKnown.get(o) ?? name);
    order.push(o);
    queue.push(o);
  }
  for (const [name, c] of roots) {
    add(name, c);
    if (isObject(c.prototype)) add(name + '.prototype', c.prototype);
  }
  const stringKeys = (o) => Reflect.ownKeys(o).filter((k) => typeof k === 'string');
  while (queue.length) {
    const o = queue.shift();
    const name = names.get(o);
    for (const k of stringKeys(o)) {
      const d = Object.getOwnPropertyDescriptor(o, k);
      if ('value' in d) {
        if (typeof d.value === 'symbol' || typeof d.value === 'bigint') {
          console.error(`${name}.${k} holds a ${typeof d.value}, which the table cannot`);
          process.exit(1);
        }
        add(`${name}.${k}`, d.value);
      } else if (constantGetters.has(d.get)) {
        add(`${name}.${k}`, constantGetters.get(d.get));
      } else if (!getters.has(d.get) && d.get !== undefined) {
        console.error(`${name}.${k} is an accessor whose getter this script does not know`);
        process.exit(1);
      }
    }
    add(`${name}.__proto__`, Object.getPrototypeOf(o));
  }
  return { names, order, getters, constantGetters };
}

if (require.main !== module) {
  module.exports = { discover };
  return;
}

const args = process.argv.slice(2);
const check = args.includes('--check');
const dist = args.find((a) => !a.startsWith('--'));
if (!dist) {
  console.error('usage: node tools/gen_realm.cjs <markdoc dist/index.js> [--check]');
  process.exit(2);
}
const Markdoc = require(path.resolve(dist));
const markdocVersion = (() => {
  try {
    return JSON.parse(fs.readFileSync(path.join(path.dirname(path.resolve(dist)), '..', 'package.json'), 'utf8')).version;
  } catch {
    return 'unknown';
  }
})();
const outFile = path.join(__dirname, '..', 'src', 'next', 'realm', 'table.rs');

const { names, order, getters, constantGetters } = discover(Markdoc);
const stringKeys = (o) => Reflect.ownKeys(o).filter((k) => typeof k === 'string');
const index = new Map(order.map((o, i) => [o, i]));

// Facts.
function isConstructor(f) {
  if (typeof f !== 'function') return false;
  try {
    Reflect.construct(function () {}, [], f);
    return true;
  } catch {
    return false;
  }
}
// Does V8 run the function without a frame of its own that stack walks see (a C++ or
// assembly builtin), so that a call it makes of something that is not a function names the
// call site of the script below it (`JSON.stringify is not a function` when JSON.stringify
// called it as a toJSON), or with one (a CSA or Torque builtin, a JS function: `object is not
// a function`)? Probed: the function as a toJSON of an object whose conversions all call
// Function.prototype.apply with a non-callable `this`.
function isFrameless(f) {
  const apply = Function.prototype.apply;
  const convert = { toString: apply, valueOf: apply };
  const holder = { ...convert, length: { ...convert }, 0: { ...convert, toLocaleString: apply }, toJSON: f };
  try {
    JSON.stringify({ k: holder });
  } catch (e) {
    return e.message === 'JSON.stringify is not a function';
  }
  return false;
}

function primitiveData(o) {
  const tries = [
    ['String', String.prototype.valueOf],
    ['Number', Number.prototype.valueOf],
    ['Boolean', Boolean.prototype.valueOf],
  ];
  for (const [kind, valueOf] of tries) {
    try {
      return [kind, valueOf.call(o)];
    } catch {}
  }
  return null;
}
function thrown(e) {
  return `${e.name}: ${e.message}`;
}
function attempt(f) {
  try {
    return { ok: f() };
  } catch (e) {
    return { err: thrown(e) };
  }
}

// Rust text.
function rustStr(s) {
  let out = '"';
  for (let i = 0; i < s.length; i++) {
    const cu = s.charCodeAt(i);
    const ch = s[i];
    if (cu >= 0xd800 && cu <= 0xdfff) {
      console.error(`a lone or paired surrogate in ${JSON.stringify(s)}: not expected in the table`);
      process.exit(1);
    }
    if (ch === '"' || ch === '\\') out += '\\' + ch;
    else if (cu >= 0x20 && cu < 0x7f) out += ch;
    else if (cu === 0x0a) out += '\\n';
    else out += `\\u{${cu.toString(16)}}`;
  }
  return out + '"';
}
function rustNum(n) {
  if (Number.isNaN(n)) return 'f64::NAN';
  if (n === Infinity) return 'f64::INFINITY';
  if (n === -Infinity) return 'f64::NEG_INFINITY';
  if (Object.is(n, -0)) return '-0.0';
  const s = String(n);
  return /[.e]/.test(s) ? s.replace(/^(-?\d+)e/, '$1.0e') : s + '.0';
}
function data(v) {
  if (v === undefined) return 'Data::Undefined';
  if (v === null) return 'Data::Null';
  if (typeof v === 'boolean') return `Data::Boolean(${v})`;
  if (typeof v === 'number') return `Data::Number(${rustNum(v)})`;
  if (typeof v === 'string') return `Data::String(${rustStr(v)})`;
  return `Data::Object(Builtin(${index.get(v)}))`;
}
function constName(name) {
  return name
    .split('.')
    .map((seg) =>
      seg
        .replace(/^_+|_+$/g, '')
        .replace('RegExp', 'Regexp')
        .replace('NaN', 'Nan')
        .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
        .toUpperCase(),
    )
    .join('_');
}

const consts = new Map();
for (const o of order) {
  const c = constName(names.get(o));
  if (consts.has(c)) {
    console.error(`two objects are named ${c}: ${consts.get(c)} and ${names.get(o)}`);
    process.exit(1);
  }
  consts.set(c, names.get(o));
}

const lines = [];
lines.push(`// @generated by tools/gen_realm.cjs from node ${process.version} and @markdoc/markdoc ${markdocVersion}. Do not edit.`);
lines.push('//');
lines.push('// The built-in objects a value can reach through string-keyed property reads (see');
lines.push('// tools/gen_realm.cjs and the realm module\'s documentation).');
lines.push('');
lines.push('use super::{Builtin, Data, Getter, Legacy, ObjectData, Prop, Slot, Wrapped};');
lines.push('use super::{CALLABLE, CLASS, CONFIGURABLE, CONSTRUCTOR, ENUMERABLE, FRAMELESS, IS_ARRAY, WRITABLE};');
lines.push('');
lines.push(`/// The node whose realm the table describes (\`process.version\`).`);
lines.push(`pub const NODE_VERSION: &str = ${rustStr(process.version)};`);
lines.push(`/// The \`@markdoc/markdoc\` whose classes the table holds.`);
lines.push(`pub const MARKDOC_VERSION: &str = ${rustStr(markdocVersion)};`);
lines.push('');
lines.push('impl Builtin {');
for (const o of order) {
  const name = names.get(o);
  lines.push(`    /// \`${name}\`.`);
  lines.push(`    pub const ${constName(name)}: Builtin = Builtin(${index.get(o)});`);
}
lines.push('}');
lines.push('');
lines.push(`/// Every object of the table, by [\`Builtin\`] index.`);
lines.push(`pub(super) static OBJECTS: [ObjectData; ${order.length}] = [`);
for (const o of order) {
  const name = names.get(o);
  const flags = [];
  if (typeof o === 'function') flags.push('CALLABLE');
  if (isConstructor(o)) flags.push('CONSTRUCTOR');
  const source = typeof o === 'function' ? Function.prototype.toString.call(o) : null;
  if (source !== null && source.startsWith('class')) flags.push('CLASS');
  if (Array.isArray(o)) flags.push('IS_ARRAY');
  if (typeof o === 'function' && isFrameless(o)) flags.push('FRAMELESS');
  const proto = Object.getPrototypeOf(o);
  const wrapped = primitiveData(o);
  const tag = Object.prototype.toString.call(o).slice(8, -1);
  lines.push('    ObjectData {');
  lines.push(`        name: ${rustStr(name)},`);
  lines.push(`        flags: ${flags.length ? flags.join(' | ') : '0'},`);
  lines.push(`        proto: ${proto === null ? 'None' : `Some(Builtin(${index.get(proto)}))`},`);
  lines.push(`        class: ${rustStr(tag)},`);
  lines.push(
    `        wrapped: ${
      wrapped === null
        ? 'None'
        : wrapped[0] === 'String'
          ? `Some(Wrapped::String(${rustStr(wrapped[1])}))`
          : wrapped[0] === 'Number'
            ? `Some(Wrapped::Number(${rustNum(wrapped[1])}))`
            : `Some(Wrapped::Boolean(${wrapped[1]}))`
    },`,
  );
  lines.push(`        source: ${source === null ? 'None' : `Some(${rustStr(source)})`},`);
  lines.push('        props: &[');
  for (const k of stringKeys(o)) {
    const d = Object.getOwnPropertyDescriptor(o, k);
    const attrs = [];
    if (d.writable) attrs.push('WRITABLE');
    if (d.enumerable) attrs.push('ENUMERABLE');
    if (d.configurable) attrs.push('CONFIGURABLE');
    const getter = (g) =>
      constantGetters.has(g) ? `Getter::Constant(Builtin(${index.get(constantGetters.get(g))}))` : getters.get(g);
    const slot =
      'value' in d
        ? `Slot::Data(${data(d.value)})`
        : `Slot::Accessor { get: ${d.get === undefined ? 'None' : `Some(${getter(d.get)})`}, set: ${d.set !== undefined} }`;
    lines.push(`            Prop { key: ${rustStr(k)}, slot: ${slot}, attrs: ${attrs.length ? attrs.join(' | ') : '0'} },`);
  }
  lines.push('        ],');
  lines.push('    },');
}
lines.push('];');
lines.push('');
lines.push('/// What node gives for each object (by index): `typeof`, `String(o)`, `JSON.stringify(o)`');
lines.push('/// (`None` for `undefined`), `Object.keys(o)`; a thrown error as `Err("Name: message")`.');
lines.push('#[cfg(test)]');
lines.push('#[allow(clippy::type_complexity)]');
lines.push(`pub(super) static EXPECTED: [(&str, Result<&str, &str>, Result<Option<&str>, &str>, &[&str]); ${order.length}] = [`);
for (const o of order) {
  const str = attempt(() => String(o));
  const json = attempt(() => JSON.stringify(o));
  const r = (a, f) => (a.err !== undefined ? `Err(${rustStr(a.err)})` : `Ok(${f(a.ok)})`);
  lines.push(
    `    (${rustStr(typeof o)}, ${r(str, rustStr)}, ${r(json, (j) => (j === undefined ? 'None' : `Some(${rustStr(j)})`))}, &[${Object.keys(o)
      .map(rustStr)
      .join(', ')}]),`,
  );
}
lines.push('];');
lines.push('');

// ICU's root collation of ASCII (what `localeCompare` gives node, whose default locale
// collates as the root): each character's primary weight (0: ignorable, as the C0 controls
// but tab, line feed, vertical tab, form feed and carriage return are) and its tertiary
// weight (lower case before upper case). ASCII has no contractions, expansions or secondary
// differences, so two ASCII strings compare by their primary weights, then their tertiary.
{
  const chars = [];
  for (let c = 0; c < 128; c++) chars.push(String.fromCharCode(c));
  const base = new Intl.Collator(undefined, { sensitivity: 'base' });
  const ignorable = (c) => c.localeCompare('') === 0 && ('a' + c).localeCompare('a') === 0;
  const kept = chars.filter((c) => !ignorable(c)).sort((a, b) => a.localeCompare(b));
  const primary = new Array(128).fill(0);
  const tertiary = new Array(128).fill(0);
  let p = 0;
  let t = 0;
  let prev = null;
  for (const c of kept) {
    if (prev === null || base.compare(prev, c) !== 0) {
      p++;
      t = 0;
    }
    t++;
    primary[c.charCodeAt(0)] = p;
    tertiary[c.charCodeAt(0)] = t;
    prev = c;
  }
  lines.push('/// ICU\'s root collation of ASCII: each character\'s primary weight (0 for an ignorable one).');
  lines.push(`pub(super) static ASCII_PRIMARY: [u8; 128] = [${primary.join(', ')}];`);
  lines.push('/// ... and its tertiary weight within its primary (lower case first).');
  lines.push(`pub(super) static ASCII_TERTIARY: [u8; 128] = [${tertiary.join(', ')}];`);
  lines.push('');
}

const text = lines.join('\n');
if (check) {
  const current = fs.existsSync(outFile) ? fs.readFileSync(outFile, 'utf8') : '';
  if (current !== text) {
    console.error(`${path.relative(process.cwd(), outFile)} differs from what node ${process.version} gives`);
    process.exit(1);
  }
  console.log(`${path.relative(process.cwd(), outFile)}: ${order.length} objects, as node ${process.version} gives them`);
} else {
  fs.mkdirSync(path.dirname(outFile), { recursive: true });
  fs.writeFileSync(outFile, text);
  console.log(`wrote ${path.relative(process.cwd(), outFile)}: ${order.length} objects`);
}
