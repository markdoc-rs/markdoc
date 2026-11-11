'use strict';
// Values for the unit tests of the markdown-it port's mdurl, punycode, link normalization and
// core rules (replacements, smartquotes, text_join, strip_references), taken from the
// markdown-it, mdurl and punycode.js that upstream Markdoc 0.5.11 bundles.
//
// Usage: node core_values.cjs <@markdoc/markdoc dist/index.js> MODE
//   --write      regenerate core_cases.json: the generators' pieces, and node's digest of
//                every function's output over the generated URLs and token sequences
//   --check      recompute the digests from core_cases.json; exit 1 when one differs
//   --dump FILE  write each generated URL and every function's output symbols (JSONL); the
//                Rust test writes the same with MDIT_MDURL_DUMP=FILE2, to diff the two
//                (--dump-tokens FILE and MDIT_TOKENS_DUMP do the same for token sequences)
//   --examples   print the values the hand-written unit tests assert
//
// The generators (xorshift32 over the pieces in core_cases.json) are mirrored by
// `gen_cases` in src/markdown_it/mdurl/mod.rs and `gen_tokens` in
// src/markdown_it/rules_core/smartquotes.rs; the "inputs" digests check that both agree.
const fs = require('fs');
const path = require('path');

const bundlePath = path.resolve(process.argv[2]);
const mode = process.argv[3];
const Markdoc = require(bundlePath);
const MarkdownIt = new Markdoc.Tokenizer().parser.constructor;
const md = new MarkdownIt();
const mdurl = md.utils.lib.mdurl;
// punycode.js exactly as bundled (markdown-it does not export it): evaluate its section alone
const bundle = fs.readFileSync(bundlePath, 'utf8');
const pa = bundle.indexOf('// node_modules/punycode.js/punycode.es6.js');
const pb = bundle.indexOf('// node_modules/', pa + 10);
const punycode = new Function(bundle.slice(pa, pb) + '\nreturn punycode;')();

const CASES = path.join(__dirname, 'core_cases.json');

// ---- the generator's pieces (only read by --write; the JSON is what both sides use) ----
const units = (s) => Array.from({ length: s.length }, (_, i) => s.charCodeAt(i));
const PIECES = [
  ['ws', ['', ' ', '\t', '\n', '\r\n', '\u00a0', '\u3000', '\ufeff', '\u2028', '\u180e', '\u0085', '\u000b']],
  ['scheme', ['', 'http:', 'https:', 'HTTP:', 'Https:', 'mailto:', 'MAILTO:', 'ftp:', 'file:', 'gopher:',
    'javascript:', 'JavaScript:', 'data:', 'vbscript:', 'skype:', 'a.b+c-d:', 'xn:', '1:', '+:', 'http',
    'ftp;', 'h ttp:', '\u00e4:']],
  ['slashes', ['', '', '//', '//', '/', '///', '\\\\', '////', '/\\']],
  ['auth', ['', '', '', 'user@', 'u:p@', 'a@b@', '\u00fcser@', '@', ':@', 'a%20b@', 'a b@', 'x?y@', 'a#b@']],
  ['label', ['example', 'com', 'a', 'www', 'localhost', '127', '0', 'xn--exmple-cua', 'xn--n3h', 'XN--N3H',
    'xn--', 'xn--a', 'xn--a-', 'xn--99999999999', 'xn--zz', 'xn--mnchen-3ya', 'xn--MNCHEN-3YA', 'xn--ls8h',
    'xn--\u00e4', '\u00e4-xn--a', 'xn--a-\u00e4', 'm\u00fcnchen', '\u00f1', '\u4f8b\u3048', '\u30c6\u30b9\u30c8',
    '\u{1F600}', '\u30c9\u30e1\u30a4\u30f3\u540d\u4f8b', 'b\u00e4r', '\u0130stanbul', '\u03a3\u0391\u03a3',
    '\u01c5', '\u216b', '\ufb00', '\ud800', '\udc00x', 'a_b', 'a+b', '-x-', 'ab!', 'a$b', 'a(b)', 'a,b', 'a=b',
    'a~b', 'a*b', 'a&b', 'a\u2028b', 'a!\u2029b', 'a!b\u00e9', '[::1]', '[2001:db8::1]', '[v1.x]', '[', ']',
    'a'.repeat(63), 'b'.repeat(64), 'c'.repeat(70) + '\u00e9', '\u00e9'.repeat(70), 'd'.repeat(130), '']],
  ['sep', ['.', '.', '.', '.', '\u3002', '\uff0e', '\uff61', '..', ':', '@']],
  ['port', ['', '', '', ':80', ':8080', ':', ':abc', ':1:2', ':\u0663', ':0', ':80:']],
  ['path', ['', '', '/', '/a/b', '/\u00e4', '/%41%42', '/%zz', '/%', '/%4', '/%E2%82%AC', '/%C3%A4', '/%c3%a4',
    '/%C3', '/%C3%28', '/%ED%A0%80', '/%F0%9F%98%80', '/%F4%90%80%80', '/%C0%80', '/%E0%80%80', '/%F0%80%80%80',
    '/%80', '/%FF%FE', '/%3B%3b%25', '/%2F%2f', '/a b', '/a\tb', "/'", '/"<>`{}|\\^', '/\u{1F600}', '/\ud83d',
    '/\ude00x', '/\udbff\u{10FC00}', ':foo', '/a:b@c', '/~user', '/a;b=c', '/\u00a0x\u3000']],
  ['query', ['', '', '?', '?a=b', '?a=b&c=d', '?q=\u00e4', '?%3F', '?a b', '?a\u3000b', '?x#']],
  ['frag', ['', '', '#', '#frag', '#\u00e4', '#%23', '#a#b', '#a b', '#\u{1F600}', '#\udbff']],
];
const UNITS = [];
for (let c = 0; c < 128; c++) UNITS.push(c);
UNITS.push(0x80, 0x85, 0xA0, 0xC0, 0xE9, 0xFF, 0x100, 0x130, 0x131, 0x17F, 0x3A3, 0x1680, 0x2000, 0x200B,
  0x2028, 0x2029, 0x202F, 0x3000, 0x3002, 0xFF0E, 0xFF61, 0x212A, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xD83D,
  0xDE00, 0xFEFF, 0xFFFD, 0xFFFF);
// systematic hosts: every label under each prefix and before each suffix, and bracketed
// (IPv6-like) hosts in several shapes
const SYSTEMATIC = [
  ['host_prefix', ['http://', 'https://', 'HTTP://', '//', 'mailto:u@', 'skype:', 'ftp://', 'http://u:p@']],
  ['host_suffix', ['', '/', ':8080/p?q=1#f', '/\u00e4', '.com', '.xn--ls8h:80']],
  ['ipv6', ['::1', '2001:db8::1', '::ffff:1.2.3.4', 'fe80::1%25eth0', 'v1.x', '', '::', '\u00e4', 'a:b',
    '1:2:3:4:5:6:7:8', 'xn--a', ':::']],
  ['ipv6_open', ['[', '[', '[', '', '[', '[[']],
  ['ipv6_close', [']', ']:80', '', ']', ']/p', ']]']],
  ['ipv6_prefix', ['http://', '//', 'mailto:x@', 'skype:']],
];
const SEED = 0x9E3779B9;
const RANDOM_CASES = 16000;

// ---- token sequences for the core rules ----
const TOKEN_PIECES = [
  ['text', ['a', 'b', 'word', "it's", "'", '"', '""', "''", '"a"', "'a'", '1"', "2'", '"1"', 'x"y', "x'y", ' ', '  ',
    '\u00a0', '\u3000', '(', ')', '-', '--', '---', '----', '...', '..', '!!!!', '???', '?....', '+-', ',,', '(c)',
    '(TM)', '(r)', 'ab"', '"ab', '\u{1F600}', '\u{1F600}"', '"\u{1F600}', '\u2018', '\u201c', '\u00ab', '\u00e9"',
    '"\u00e9', '.', '?', '!', '*', '_', '[', ']', '<', '>', '&', '#', '1', '9', '0"', "'''", '"\'"', '\'"\'', '\t',
    'a\u2028b', '\u2014', '\u2028', '\n', 'x--y', 'a -- b', '\u20ac"', '"\u20ac', '$', '\u00bf"']],
  ['code', ['x', '"', "'", 'a b', '1', '--', '\u00a0']],
  ['html', ['<a href="x">', '</a>', '<A>', '</a >', '<b>', "<i class='x'>", '<!-- " -->']],
  ['kind', ['text', 'text', 'text', 'text', 'text', 'text', 'softbreak', 'hardbreak', 'code_inline', 'html_inline',
    'em_open', 'em_close', 'strong_open', 'strong_close', 'link_open', 'link_close', 'text_special', 'image']],
  ['block', ['paragraph_open', 'paragraph_close', 'reference_definition', 'reference_definition', 'heading_open',
    'text', 'inline']],
];
const TOKEN_CASES = 12000;

// ---- the generator ----
function generate(spec) {
  const P = new Map(spec.pieces);
  const names = spec.pieces.map((p) => p[0]);
  const U = spec.units;
  let s = spec.seed >>> 0;
  const rnd = (n) => {
    s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0;
    return s % n;
  };
  const pick = (name) => { const list = P.get(name); return list[rnd(list.length)]; };
  const cat = (...parts) => parts.flatMap((p) => (typeof p === 'number' ? [p] : typeof p === 'string' ? units(p) : p));
  const host = () => {
    const n = rnd(4);
    let out = [];
    for (let i = 0; i < n; i++) {
      if (i > 0) out = out.concat(pick('sep'));
      out = out.concat(pick('label'));
    }
    return out;
  };
  const structured = () => {
    const parts = [];
    parts.push(pick('ws'), pick('scheme'), pick('slashes'), pick('auth'), host(), pick('port'),
      pick('path'), pick('query'), pick('frag'), pick('ws'));
    return parts.flat();
  };
  const soup = () => {
    const len = rnd(24);
    let out = [];
    for (let i = 0; i < len; i++) {
      if (rnd(4) === 0) {
        const list = P.get(names[rnd(names.length)]);
        out = out.concat(list[rnd(list.length)]);
      } else {
        out.push(U[rnd(U.length)]);
      }
    }
    return out;
  };
  const HEX = (b) => b.toString(16).toUpperCase().padStart(2, '0');
  const cases = [];
  // every ASCII character in path, query and fragment, in a host, alone and after '%'
  for (let c = 0; c < 128; c++) {
    cases.push(cat('http://h/a', c, 'b?q', c, '#f', c));
    cases.push(cat(c));
    cases.push(cat('%', c, 'A'));
    cases.push(cat('//h', c, 'x/'));
    cases.push(cat('mailto:a', c, '@b', c, '.c'));
  }
  const S = new Map(spec.systematic);
  for (const label of P.get('label')) {
    for (const prefix of S.get('host_prefix')) {
      for (const suffix of S.get('host_suffix')) cases.push(cat(prefix, label, suffix));
    }
  }
  for (const ip of S.get('ipv6')) {
    S.get('ipv6_open').forEach((open, k) => {
      for (const prefix of S.get('ipv6_prefix')) cases.push(cat(prefix, open, ip, S.get('ipv6_close')[k]));
    });
  }
  // every byte as an escape, upper and lower case, then as a lead byte of longer sequences
  for (let b = 0; b < 256; b++) {
    cases.push(cat('%', HEX(b)));
    cases.push(cat('%', HEX(b).toLowerCase()));
    cases.push(cat('/%', HEX(b), '%80'));
    cases.push(cat('%', HEX(b), '%BF%BF'));
    cases.push(cat('%', HEX(b), '%80%80%80'));
    cases.push(cat('%', HEX(b), '%41'));
    cases.push(cat('%', HEX(b), '%', HEX(b)));
  }
  for (let k = 0; k < spec.random_cases; k++) {
    const m = rnd(10);
    let u;
    if (m < 6) {
      u = structured();
    } else if (m < 8) {
      u = soup();
    } else if (m < 9) {
      u = host();
    } else {
      u = structured();
      const n = rnd(4) + 1;
      for (let i = 0; i < n; i++) {
        const at = rnd(u.length + 1);
        u = u.slice(0, at).concat([U[rnd(U.length)]], u.slice(at));
      }
    }
    cases.push(u);
  }
  return cases;
}

// A token sequence: the children of one inline token ({type, content, level, info, children})
// and the block tokens around it. Mirrored by `gen_tokens` in smartquotes.rs.
function generateTokens(spec) {
  const P = new Map(spec.pieces);
  let s = spec.seed >>> 0;
  const rnd = (n) => {
    s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0;
    return s % n;
  };
  const pick = (name) => { const list = P.get(name); return list[rnd(list.length)]; };
  const text = () => {
    const k = rnd(3) + 1;
    let out = [];
    for (let j = 0; j < k; j++) out = out.concat(pick('text'));
    return out;
  };
  const cases = [];
  for (let c = 0; c < spec.cases; c++) {
    const n = rnd(9) + 1;
    let level = 0;
    const children = [];
    for (let i = 0; i < n; i++) {
      const type = str(pick('kind'));
      const t = { type, content: [], level, info: '', children: null };
      if (type === 'text' || type === 'text_special') {
        t.content = text();
      } else if (type === 'code_inline') {
        t.content = pick('code');
      } else if (type === 'html_inline') {
        t.content = pick('html');
      } else if (type.endsWith('_open')) {
        t.level = level++;
        if (type === 'link_open' && rnd(2) === 0) t.info = 'auto';
      } else if (type.endsWith('_close')) {
        t.level = --level;
        if (type === 'link_close' && rnd(2) === 0) t.info = 'auto';
      } else if (type === 'image') {
        const m = rnd(4);
        t.children = [];
        for (let j = 0; j < m; j++) {
          const kind = rnd(3) === 0 ? 'text_special' : 'text';
          t.children.push({ type: kind, content: text(), level: 0, info: '', children: null });
        }
      }
      children.push(t);
    }
    const before = [];
    const after = [];
    for (let k = rnd(3); k > 0; k--) before.push(str(pick('block')));
    for (let k = rnd(3); k > 0; k--) after.push(str(pick('block')));
    cases.push({ children, before, after });
  }
  return cases;
}

// ---- output symbols (u32), folded into a digest per function ----
const str = (a) => String.fromCharCode(...a);
const NULL = 0, TRUE = 0x7FFFFFF1, FALSE = 0x7FFFFFF0, ERROR = 0xFFFFFFFF;
function strSyms(out, s) {
  out.push(s.length + 1);
  for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i));
}
function valSyms(out, v) {
  if (v === null || v === undefined) out.push(NULL);
  else if (v === true) out.push(TRUE);
  else if (v === false) out.push(FALSE);
  else strSyms(out, v);
}
function tryStr(out, f) {
  let v;
  try { v = f(); } catch (e) { out.push(ERROR); strSyms(out, e.message); return null; }
  strSyms(out, v);
  return v;
}
const URL_FIELDS = ['protocol', 'slashes', 'auth', 'port', 'hostname', 'hash', 'search', 'pathname'];
function urlSyms(out, u) { for (const f of URL_FIELDS) valSyms(out, u[f]); }

const FUNCTIONS = [
  ['parse_true', (o, s) => urlSyms(o, mdurl.parse(s, true))],
  ['parse_false', (o, s) => urlSyms(o, mdurl.parse(s, false))],
  ['format_true', (o, s) => strSyms(o, mdurl.format(mdurl.parse(s, true)))],
  ['format_false', (o, s) => strSyms(o, mdurl.format(mdurl.parse(s, false)))],
  ['encode', (o, s) => strSyms(o, mdurl.encode(s))],
  ['encode_component', (o, s) => strSyms(o, mdurl.encode(s, mdurl.encode.componentChars))],
  ['encode_empty', (o, s) => strSyms(o, mdurl.encode(s, ''))],
  ['encode_not_escaped', (o, s) => strSyms(o, mdurl.encode(s, false))],
  ['encode_component_not_escaped', (o, s) => strSyms(o, mdurl.encode(s, mdurl.encode.componentChars, false))],
  ['encode_proto', (o, s) => strSyms(o, mdurl.encode(s, 'toString'))],
  ['decode', (o, s) => strSyms(o, mdurl.decode(s))],
  ['decode_percent', (o, s) => strSyms(o, mdurl.decode(s, mdurl.decode.defaultChars + '%'))],
  ['decode_component', (o, s) => strSyms(o, mdurl.decode(s, mdurl.decode.componentChars))],
  ['decode_proto', (o, s) => strSyms(o, mdurl.decode(s, '__proto__'))],
  ['decode_encode', (o, s) => strSyms(o, mdurl.decode(mdurl.encode(s)))],
  ['puny_to_ascii', (o, s) => tryStr(o, () => punycode.toASCII(s))],
  ['puny_to_unicode', (o, s) => tryStr(o, () => punycode.toUnicode(s))],
  ['puny_encode', (o, s) => tryStr(o, () => punycode.encode(s))],
  ['puny_decode', (o, s) => tryStr(o, () => punycode.decode(s))],
  ['puny_roundtrip', (o, s) => tryStr(o, () => punycode.decode(punycode.encode(s)))],
  ['puny_ucs2', (o, s) => {
    const cps = punycode.ucs2.decode(s);
    o.push(cps.length);
    for (const c of cps) o.push(c);
    tryStr(o, () => punycode.ucs2.encode(cps));
  }],
  ['puny_host', (o, s) => {
    const h = mdurl.parse(s, true).hostname;
    if (h === null) { o.push(NULL); return; }
    const a = tryStr(o, () => punycode.toASCII(h));
    tryStr(o, () => punycode.toUnicode(h));
    if (a !== null) tryStr(o, () => punycode.toUnicode(a));
  }],
  ['normalize_link', (o, s) => strSyms(o, md.normalizeLink(s))],
  ['normalize_link_text', (o, s) => strSyms(o, md.normalizeLinkText(s))],
  ['validate_link', (o, s) => valSyms(o, md.validateLink(s))],
];

// FNV-1a over the little-endian bytes of each symbol
function fold(h, sym) {
  for (let k = 0; k < 4; k++) {
    h = Math.imul(h ^ ((sym >>> (8 * k)) & 0xff), 16777619) >>> 0;
  }
  return h;
}
const FNV_OFFSET = 2166136261;

function digests(spec, dumpPath) {
  const cases = generate(spec);
  const result = { inputs: FNV_OFFSET };
  for (const [name] of FUNCTIONS) result[name] = FNV_OFFSET;
  const dump = dumpPath ? fs.openSync(dumpPath, 'w') : null;
  cases.forEach((u, i) => {
    const inSyms = [];
    strSyms(inSyms, str(u));
    for (const x of inSyms) result.inputs = fold(result.inputs, x);
    const s = str(u);
    const outs = {};
    for (const [name, f] of FUNCTIONS) {
      const o = [];
      f(o, s);
      for (const x of o) result[name] = fold(result[name], x);
      if (dump) outs[name] = o;
    }
    if (dump) fs.writeSync(dump, JSON.stringify({ i, input: u, outputs: outs }) + '\n');
  });
  if (dump) fs.closeSync(dump);
  return { count: cases.length, digests: result };
}

const TOKEN_CONFIGS = [
  ['default', { typographer: true }],
  ['guillemets', { typographer: true, quotes: '\u00ab\u00bb\u201e\u201c' }],
  ['array', { typographer: true, quotes: ['<<', '>>', '<', '>'] }],
  ['short', { typographer: true, quotes: 'ab' }],
];
function tokenDigests(spec, dumpPath) {
  const cases = generateTokens(spec);
  const mds = TOKEN_CONFIGS.map(([, o]) => new MarkdownIt(o));
  const rule = (name) => md.core.ruler.__rules__.find((r) => r.name === name).fn;
  const T = new md.core.State('', md, {}).Token;
  const build = (t) => {
    const x = new T(t.type, '', 0);
    x.content = str(t.content);
    x.level = t.level;
    x.info = t.info;
    if (t.children) x.children = t.children.map(build);
    return x;
  };
  // the block tokens around one inline token, whose content is its children's text
  const block = (c) => {
    const inline = new T('inline', '', 0);
    inline.children = c.children.map(build);
    inline.content = inline.children.map((x) => x.content).join('');
    const plainTok = (type) => { const x = new T(type, '', 0); if (type === 'inline') x.children = []; return x; };
    return [...c.before.map(plainTok), inline, ...c.after.map(plainTok)];
  };
  const run = (m, names, c) => {
    const st = new m.core.State('', m, {});
    st.tokens = block(c);
    for (const n of names) rule(n)(st);
    return st.tokens;
  };
  const tokSyms = (o, ts) => {
    o.push(ts.length);
    for (const x of ts) {
      strSyms(o, x.type);
      strSyms(o, x.content);
      o.push(x.level + 1000);
      if (x.children) tokSyms(o, x.children); else o.push(NULL);
    }
  };
  const FUNCS = [
    ['replacements', (o, c) => tokSyms(o, run(mds[0], ['replacements'], c))],
    ...TOKEN_CONFIGS.map(([name], k) => ['smartquotes_' + name, (o, c) => tokSyms(o, run(mds[k], ['smartquotes'], c))]),
    ['text_join', (o, c) => tokSyms(o, run(md, ['text_join'], c))],
    ['strip_references', (o, c) => tokSyms(o, run(md, ['strip_references'], c))],
    ['chain', (o, c) => tokSyms(o, run(mds[0], ['strip_references', 'replacements', 'smartquotes', 'text_join'], c))],
  ];
  const result = { inputs: FNV_OFFSET };
  for (const [name] of FUNCS) result[name] = FNV_OFFSET;
  const dump = dumpPath ? fs.openSync(dumpPath, 'w') : null;
  cases.forEach((c, i) => {
    const inSyms = [];
    tokSyms(inSyms, block(c));
    for (const x of inSyms) result.inputs = fold(result.inputs, x);
    const outs = {};
    for (const [name, f] of FUNCS) {
      const o = [];
      f(o, c);
      for (const x of o) result[name] = fold(result[name], x);
      if (dump) outs[name] = o;
    }
    if (dump) fs.writeSync(dump, JSON.stringify({ i, input: inSyms, outputs: outs }) + '\n');
  });
  if (dump) fs.closeSync(dump);
  return { count: cases.length, digests: result };
}

function examples() {
  const out = {};
  const t = (k, v) => { out[k] = v; };
  const hex = (s) => Array.from({ length: s.length }, (_, i) => s.charCodeAt(i).toString(16));
  t('encode', [mdurl.encode('a b%20c%zz%'), mdurl.encode('%41%4', false), mdurl.encode('\u00e4\u20ac\u{1F600}'),
    mdurl.encode('/?#[]', mdurl.encode.componentChars), mdurl.encode("-_.!~*'()", ''),
    mdurl.encode('\ud800A\udc00\udbff'), mdurl.encode('a%41\u00e4', 'toString'), mdurl.encode('ab', '__proto__')]);
  t('decode', [mdurl.decode('%41%2f%2F%25%zz'), mdurl.decode('%41%2f%25', ';/?:@&=+$,#%'),
    mdurl.decode('%C3%A4%E2%82%AC%F0%9F%98%80'), mdurl.decode('%C3%28 %C0%80 %ED%A0%80 %F4%90%80%80 %80 %C3'),
    mdurl.decode('%e2%82 %F0%9F%98 %FF%FE%FD'), mdurl.decode('%41%C3%A4', 'valueOf'), mdurl.decode('%%41%4%')]);
  const P = (u) => { const r = mdurl.parse(u, true); return URL_FIELDS.map((f) => r[f]); };
  t('parse', ['  HTTP://u:p@WWW.Example.com:8080/p/a?q=1#f  ', 'http://a@b@c/', 'http://a@b?@c', 'mailto:user@host',
    'javascript://x', '//h/p', 'http://[::1]:80/x', 'http://ex ample.com/', 'skype:name', 'http://a.b!c.d/e',
    'http://' + 'a'.repeat(64) + '.com', 'http://\u00e4\u00f6.de:abc/', 'file:', 'http://x.y:', 'a:b'].map(P));
  t('parseNoSlashes', ['/a/b?c', '//a/b', '///a', '/a b', '/a?b c', '#x', 'a/b'].map((u) => {
    const r = mdurl.parse(u, false); return URL_FIELDS.map((f) => r[f]);
  }));
  const tryP = (f) => { try { return f(); } catch (e) { return 'RangeError: ' + e.message; } };
  t('punycode', [tryP(() => punycode.toASCII('m\u00fcnchen.de')), tryP(() => punycode.toASCII('user@\u00fc.\u3002x')),
    tryP(() => punycode.toASCII('a@b@\u00e4')), tryP(() => punycode.toUnicode('xn--mnchen-3ya.de')),
    tryP(() => punycode.toUnicode('XN--mnchen-3ya.xn--MNCHEN-3YA')), tryP(() => punycode.decode('99999999999')),
    tryP(() => punycode.decode('\u00e4-a')), tryP(() => punycode.decode('a-')), tryP(() => punycode.decode('!')),
    tryP(() => punycode.encode('\u{1F600}\ud800')), tryP(() => punycode.decode('ls8h')),
    tryP(() => punycode.toUnicode('xn--!.xn--99999999999')), tryP(() => punycode.decode('hzzzzz')),
    tryP(() => punycode.ucs2.encode([0x41, 0x110000])), punycode.ucs2.decode('a\u{1F600}\ud800\u{10FC00}')]);
  t('normalizeLink', ['http://ex\u00e4mple.com/\u00e4?q=\u00e4#\u00e4', 'HTTP://\u00e4.com', 'mailto:u@\u00e4.de',
    '//\u00e4/x', 'skype:\u00e4', 'http://xn--!/', 'http://[::1]/', '%zz%41'].map((u) => md.normalizeLink(u)));
  t('normalizeLinkText', ['http://xn--exmple-cua.com/%C3%A4?q=%25#%2F', 'http://xn--99999999999/', 'mailto:u@xn--tda.de',
    '%3B%2f%25', 'http://%C3%A4@x/'].map((u) => md.normalizeLinkText(u)));

  // core rules on hand-built tokens, through the bundle's own rule functions
  const rule = (name) => md.core.ruler.__rules__.find((r) => r.name === name).fn;
  const T = md.core.State.prototype.Token || new md.core.State('', md, {}).Token;
  const tok = (type, content, level, extra) => {
    const x = new T(type, '', 0); x.content = content; x.level = level || 0; Object.assign(x, extra || {}); return x;
  };
  const typo = new MarkdownIt({ typographer: true });
  const runCore = (m, name, tokens) => { const st = new m.core.State('', m, {}); st.tokens = tokens; rule(name)(st); return st.tokens; };
  const inline = (content, children) => tok('inline', content, 0, { children });
  t('rare', ['a+-b +-+- ++-', 'x.. y... z.....', '?... !.. ?. ?..', '?!?!? !!!! ??? a!!!!!b', 'a,,b,,,,c,d',
    'a---b ---- c --- d', '---\n---x', 'a -- b --c-- d--e', '--\u2028--', 'x---y---z', 'a--b--c', '-- -- --',
    'a\u3000--\u3000b', '(c) (C) (tm) (TM) (tM) (r) (R) (x) ((c)) (c', '(\u212a)'].map((s) =>
    runCore(typo, 'replacements', [inline(s, [tok('text', s)])])[0].children[0].content));
  t('rareAutolink', runCore(typo, 'replacements', [inline('a--b (c)', [tok('text', 'a--b (c)'),
    tok('link_open', '', 0, { info: 'auto' }), tok('text', 'x--y (c)'), tok('link_close', '', 0, { info: 'auto' }),
    tok('link_close', '', 0, { info: 'auto' }), tok('text', 'p--q (r)')])])[0].children.map((x) => x.content));
  const sq = (m, children) => runCore(m, 'smartquotes', [inline('\'"', children)])[0].children.map((x) => x.content);
  t('smartquotes', [
    sq(typo, [tok('text', '"a'), tok('code_inline', 'x'), tok('text', 'b" it\'s')]),
    sq(typo, [tok('text', "'a "), tok('em_open', ''), tok('text', "b'", 1), tok('em_close', ''), tok('text', " c'")]),
    sq(typo, [tok('text', 'x'), tok('softbreak', ''), tok('text', '"y"')]),
    sq(typo, [tok('text', '1"" "a"')]),
    sq(new MarkdownIt({ typographer: true, quotes: '\u00ab\u00bb' }), [tok('text', '"a" \'b\'')]),
    sq(new MarkdownIt({ typographer: true, quotes: ['<<', '>>', '<', '>'] }), [tok('text', '"a" \'b\'')]),
    sq(typo, [tok('text', '"\u{1F600}" \'\u00a0x\' "\u2018"')]),
  ]);
  const summary = (ts) => ts.map((x) => x.type + ':' + x.content + (x.children ? JSON.stringify(summary(x.children)) : ''));
  t('textJoin', [summary(runCore(md, 'text_join', [tok('paragraph_open', ''), inline('', [tok('text', 'a'),
    tok('text_special', '\\*'), tok('text', 'b'), tok('softbreak', ''), tok('text_special', 'c'),
    tok('text', 'd', 3), tok('code_inline', 'x'), tok('text', 'e')]), tok('text', 'p'), tok('text', 'q')])),
  summary(runCore(md, 'text_join', [inline('', [tok('image', '', 0, { children: [tok('text_special', '&'),
    tok('text', 'x'), tok('em_open', ''), tok('text', 'y'), tok('text', 'z')] }), tok('text', 't', 0, { children: [] }),
  tok('text', 'u')])]))]);
  t('stripReferences', runCore(md, 'strip_references', ['reference_definition', 'a', 'reference_definition',
    'reference_definition', 'b', 'c', 'reference_definition'].map((k, i) => tok(k, String(i)))).map((x) => x.type + ':' + x.content));
  return out;
}

if (mode === '--write') {
  const urls = {
    seed: SEED, random_cases: RANDOM_CASES, units: UNITS,
    pieces: PIECES.map(([name, list]) => [name, list.map(units)]),
    systematic: SYSTEMATIC.map(([name, list]) => [name, list.map(units)]),
  };
  Object.assign(urls, digests(urls));
  const tokens = {
    seed: 0x5BD1E995, cases: TOKEN_CASES,
    pieces: TOKEN_PIECES.map(([name, list]) => [name, list.map(units)]),
  };
  Object.assign(tokens, tokenDigests(tokens));
  const spec = { note: 'written by tests/markdown_it/core_values.cjs --write; read by tests under src/markdown_it', urls, tokens };
  fs.writeFileSync(CASES, JSON.stringify(spec) + '\n');
  console.log(`wrote ${CASES}: ${urls.count} URLs, ${tokens.count} token sequences`);
} else if (mode === '--check' || mode === '--dump' || mode === '--dump-tokens') {
  const spec = JSON.parse(fs.readFileSync(CASES, 'utf8'));
  let bad = false;
  for (const [key, f, dump] of [['urls', digests, mode === '--dump'], ['tokens', tokenDigests, mode === '--dump-tokens']]) {
    const r = f(spec[key], dump ? process.argv[4] : null);
    if (r.count !== spec[key].count) bad = true;
    for (const [k, v] of Object.entries(r.digests)) {
      if (spec[key].digests[k] !== v) { console.log(`${key}.${k}: stored ${spec[key].digests[k]}, node ${v}`); bad = true; }
    }
    console.log(`${key}: ${r.count} cases, ${Object.keys(r.digests).length} digests`);
  }
  console.log(bad ? 'MISMATCH' : 'ok');
  process.exit(bad ? 1 : 0);
} else if (mode === '--examples') {
  console.log(JSON.stringify(examples(), null, 1));
} else {
  console.error('mode: --write | --check | --dump FILE | --dump-tokens FILE | --examples');
  process.exit(2);
}
