'use strict';
// Checks what the port's linkify-it (src/markdown_it/linkify_it/rebuilder.rs) assumes of V8's
// case-insensitive (`i`, non-unicode) regexes, in upstream Markdoc 0.5.11's bundle: every
// class linkify-it uses matches the same code units with `i` as without, and each letter of
// its literals matches only its two cases (so ſ, ı, İ, the Kelvin sign and fullwidth letters
// match no ASCII letter). Usage: node linkify_classes.cjs DIST/index.js (prints "ok: ...").
//
// With --table, writes (stdout) what the port's character functions must answer, from V8's
// own regexes (the dist's REBuilder sources, flags `iy`), as runs of equal rows:
// tests/next/tokenizer/linkify_classes.json, which src/markdown_it/linkify_it/tests.rs
// compares with rebuilder.rs's is_sep, is_zcc, zpcc_len, pseudo_letter_len and eq_ci.
//   bmp:    for each code unit c, lone surrogates included: [sep, ZCc, ZPCc, L] at 0 of c,
//           L at 0 of c+'a', of c+LOW, at 1 of 'a'+c, of 'a'+c+LOW (LOW a lone low
//           surrogate, U+DC00), and the units of the port's literals c matches with `i`
//   astral: for each code point above U+FFFF (a surrogate pair): ZPCc and L at 0 of it,
//           L at 0 of it+'a', and at 1 of 'a'+it
// where a class is the length the regex matches there (0: none), L the pseudo-letter.
//   node linkify_classes.cjs DIST/index.js --table > tests/next/tokenizer/linkify_classes.json
const path = require('path');
const Markdoc = require(path.resolve(process.argv[2]));
const md = new (new Markdoc.Tokenizer().parser.constructor)({ linkify: true });
const re = md.linkify.re;

if (process.argv[3] === '--table') {
  const sticky = (src) => new RegExp('(?:' + src + ')', 'iy');
  const sepRe = sticky(re.get_text_separators().source);
  const zccRe = sticky(re.src_ZCc);
  const zpccRe = sticky(re.src_ZPCc);
  const plRe = sticky(re.get_pseudo_letter().source);
  const at = (r, s, i) => { r.lastIndex = i; const m = r.exec(s); return m ? m[0].length : 0; };
  const LOW = String.fromCharCode(0xdc00);
  const pats = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789:/-._@' +
    String.fromCharCode(0x440, 0x444, 0x420, 0x424);
  const patRe = [...pats].map((p) => [p, new RegExp('^' + p.replace(/[.?*+^$[\]\\(){}|/-]/g, '\\$&') + '$', 'i')]);
  const runs = (rows, first) => {
    const out = [];
    rows.forEach((row, i) => {
      const key = JSON.stringify(row);
      const last = out[out.length - 1];
      if (last && last.key === key) last.end = first + i;
      else out.push({ start: first + i, end: first + i, key, row });
    });
    return out.map((r) => [r.start, r.end, ...r.row]);
  };
  const bmp = [];
  for (let c = 0; c <= 0xffff; c++) {
    const u = String.fromCharCode(c);
    const eq = patRe.filter(([, r]) => r.test(u)).map(([p]) => p).join('') || '-';
    bmp.push([at(sepRe, u, 0), at(zccRe, u, 0), at(zpccRe, u, 0), at(plRe, u, 0),
      at(plRe, u + 'a', 0), at(plRe, u + LOW, 0), at(plRe, 'a' + u, 1), at(plRe, 'a' + u + LOW, 1), eq]);
  }
  const astral = [];
  for (let cp = 0x10000; cp <= 0x10ffff; cp++) {
    const s = String.fromCodePoint(cp);
    astral.push([at(zpccRe, s, 0), at(plRe, s, 0), at(plRe, s + 'a', 0), at(plRe, 'a' + s, 1)]);
  }
  const lines = ['{"bmp": ['];
  const b = runs(bmp, 0);
  b.forEach((r, i) => lines.push(' ' + JSON.stringify(r) + (i + 1 < b.length ? ',' : '')));
  lines.push('], "astral": [');
  const a = runs(astral, 0x10000);
  a.forEach((r, i) => lines.push(' ' + JSON.stringify(r) + (i + 1 < a.length ? ',' : '')));
  lines.push(']}');
  process.stdout.write(lines.join('\n') + '\n');
  process.exit(0);
}
const all = [];
for (let c = 0; c < 0x10000; c++) all.push(String.fromCharCode(c));
function setOf(src, flags) {
  const r = new RegExp('^(?:' + src + ')$', flags);
  const out = [];
  for (let c = 0; c < 0x10000; c++) if (r.test(all[c])) out.push(c);
  return out;
}
const same = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
let bad = 0;
function check(name, src) {
  const a = setOf(src, ''), b = setOf(src, 'i');
  if (!same(a, b)) { bad++; console.log('DIFF', name, 'plain', a.length, 'i', b.length, b.filter(x => !a.includes(x)).map(x => x.toString(16)).slice(0, 20)); }
}
check('ZPCc', re.src_ZPCc);
check('ZCc', re.src_ZCc);
check('sep', '[><｜]');
check('Any-bmp', '[\\0-\\uD7FF\\uE000-\\uFFFF]');
check('hex', '[0-9A-Fa-f]');
check('path-class', '[a-zA-Z0-9%/&]');
check('mail', "[-!#$%&'*+/=?^_`{|}~a-zA-Z0-9]");
check('digit', '\\d');
check('punct', '[/?#\\\\:%@&=_~*"\'.,;!?\\[\\](){}$+<=>^`|\\uff5c-]');
// letters: what each matches with i
const letters = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZрфРФ';
for (const l of letters) {
  const b = setOf(l.replace(/[\\^$.*+?()[\]{}|]/g, '\\$&'), 'i');
  const want = [l.toLowerCase().charCodeAt(0), l.toUpperCase().charCodeAt(0)].sort((x, y) => x - y);
  if (!same(b, [...new Set(want)])) { bad++; console.log('LETTER', l, b.map(x => x.toString(16))); }
}
const xn = setOf('[a-z0-9\\-]', 'i');
const xnWant = setOf('[a-zA-Z0-9\\-]', '');
if (!same(xn, xnWant)) { bad++; console.log('xn class differs'); }
// astral: P's surrogate-pair alternatives are case-free
console.log(bad ? `${bad} differences` : 'ok: every class is closed under V8 non-unicode case folding; letters match only their ASCII/Cyrillic pair');
// odd characters people expect to fold
for (const [c, l] of [['ſ', 's'], ['ı', 'i'], ['İ', 'i'], ['K', 'k'], ['ｈ', 'h'], ['ẞ', 's']]) {
  console.log(JSON.stringify(c), 'matches', l, 'with i:', new RegExp(l, 'i').test(c));
}
